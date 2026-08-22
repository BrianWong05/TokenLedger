use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::Value;

use crate::adapters::{
    absolute_project, normalize_epoch, remember_file_states, sqlite_file_states, unchanged,
};
use crate::db::{insert_events_superseding, source_session_ids, upsert_source_sessions};
use crate::types::{SourceScanResult, SourceSessionMeta, UsageEvent};

const SOURCE: &str = "opencode";
const PARSER_VERSION: i64 = 1;

/// OpenCode's Antigravity plugin exposes Google Antigravity's models under
/// `antigravity-…` prefixed names — internal routing aliases that match no
/// catalog row (CONTEXT.md §Model: an alias names no model and would price
/// against nothing). Translate to the underlying real Model so price
/// resolution succeeds, mirroring Qoder's `qmodel_38max` → `qwen3.8-max`
/// step in `adapters::qoder`.
///
/// Two shapes appear in the wild:
/// - `-thinking-{level}` for the plugin's thinking-budget Claude Models
///   (e.g. `antigravity-claude-sonnet-4-5-thinking-high`).
/// - bare variant suffixes for the Gemini picker: `-minimal/-low/-medium/-high`
///   on Gemini 3 Flash and `-low/-high` on Gemini 3 Pro
///   (e.g. `antigravity-gemini-3-flash-medium`).
///
/// All variants resolve to the same base catalog row today — Antigravity's
/// pricing is a flat per-model figure, not per-variant. Unknown aliases pass
/// through untouched so a renamed Model surfaces in the Pricing tab as
/// Unpriced rather than silently mis-pricing against a guessed catalog row
/// (the rename signal defined in `adapters::antigravity::resolve_model`).
///
/// Target names are bare Model ids (not `gemini/`-prefixed): the same way
/// Qoder books `qwen3.8-max`, so OpenCode's booking joins Gemini CLI's and
/// Antigravity's for the same Model, and a user Override keyed on
/// the bare name matches (the Override tier matches exact raw names).
/// Resolution still lands on the direct-API publisher rate: the `gemini/`
/// catalog key normalizes to this bare name and outranks the Vertex spelling
/// under ADR-0009, so the merged row carries the Gemini rate.
///
/// Known divergence: `adapters::antigravity::resolve_model` resolves its own
/// `gemini-3-flash-a/-b` wire aliases to `gemini-3.5-flash` (the Antigravity
/// IDE's "3-flash" line is really 3.5 Flash), while this OpenCode surface —
/// a different plugin and API path — maps to `gemini-3-flash-preview` per the
/// plugin's documented transformation. The user approved the latter; the
/// divergence is deliberate and re-checked if Antigravity renames the line.
fn translate_antigravity(raw: &str) -> String {
    let Some(stripped) = raw.strip_prefix("antigravity-") else {
        return raw.to_string();
    };
    match split_antigravity_variant(stripped) {
        "gemini-3-pro" | "gemini-3-pro-preview" => "gemini-3-pro-preview",
        "gemini-3-flash" | "gemini-3-flash-preview" => "gemini-3-flash-preview",
        "claude-sonnet-4-5" => "claude-sonnet-4-5",
        "claude-opus-4-5" => "claude-opus-4-5",
        // Unknown alias — return the RAW name so it stays visible in the
        // Pricing tab as Unpriced (the rename signal).
        _ => return raw.to_string(),
    }
    .to_string()
}

/// Strip the variant suffix off an `antigravity-…` body, returning the base
/// Model name. Prefers the `-thinking-{level}` form before any plain
/// `{level}` suffix so `claude-sonnet-4-5-thinking-low` reads as a thinking
/// variant of sonnet rather than a bare `-low` Gemini variant. The plain
/// suffixes are mutually exclusive, so their order does not change the
/// result — the array is sorted longest-first only to keep the check
/// obvious.
fn split_antigravity_variant(stripped: &str) -> &str {
    if let Some(idx) = stripped.find("-thinking-") {
        return &stripped[..idx];
    }
    if let Some(base) = stripped.strip_suffix("-thinking") {
        return base;
    }
    for level in ["-minimal", "-medium", "-high", "-max", "-low"] {
        if let Some(base) = stripped.strip_suffix(level) {
            return base;
        }
    }
    stripped
}

#[derive(Default)]
struct OpencodeScan {
    events: Vec<UsageEvent>,
    session_ids: HashSet<String>,
    source_sessions: Vec<SourceSessionMeta>,
    superseded: HashSet<String>,
    seen_keys: HashSet<String>,
    lines_skipped: u64,
}

/// Per-model totals for one Session. OpenCode stores usage per assistant
/// Message with the Model that produced it, so a Session that switched Models
/// (e.g. sub-agents on a different Model) splits into one group per Model;
/// Messages without a proven Model fall into the None group.
#[derive(Default)]
struct SessionTotals {
    groups: BTreeMap<Option<String>, ModelTotals>,
}

#[derive(Default)]
struct ModelTotals {
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    reasoning: i64,
    reasoning_seen: bool,
    reasoning_incomplete: bool,
    latest_message_ms: Option<i64>,
}

#[derive(Clone)]
struct UsageSnapshot {
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    reasoning: Option<i64>,
    model: Option<String>,
}

enum ParsedMessage {
    NotUsage,
    Zero,
    Usage(UsageSnapshot),
    Invalid,
}

/// Scan OpenCode's current SQLite database and legacy JSON storage.
pub fn scan_opencode(
    conn: &mut Connection,
    data_root: &Path,
    legacy_root: &Path,
    database_override: Option<&Path>,
) -> SourceScanResult {
    let (database_paths, mut errors) = discover_databases(data_root, database_override);
    let message_root = legacy_message_root(legacy_root);
    let legacy_sessions = discover_legacy_sessions(legacy_root);
    let mut harvest = OpencodeScan::default();
    let mut lines_skipped = 0;
    let mut parsed_states = Vec::new();
    let mut skipped_database = false;
    let mut scan_legacy = true;

    for path in database_paths {
        let states = sqlite_file_states(&path, PARSER_VERSION);
        if states.iter().all(|(path, state)| unchanged(conn, path, state)) {
            skipped_database = true;
            continue;
        }
        match scan_database(&path) {
            Ok(scan) => {
                parsed_states.push(states);
                absorb(&mut harvest, scan);
            }
            Err(error) => errors.push(error),
        }
    }

    if skipped_database && !legacy_sessions.is_empty() {
        match source_session_ids(conn, SOURCE) {
            Ok(session_ids) => harvest.session_ids.extend(session_ids),
            Err(error) => {
                scan_legacy = false;
                errors.push(format!("{SOURCE}: Session metadata read failed: {error}"));
            }
        }
    }

    if scan_legacy {
        for session_path in legacy_sessions {
            match scan_legacy_session(
                &session_path,
                message_root.clone(),
                &harvest.session_ids,
            ) {
                Ok(scan) => absorb(&mut harvest, scan),
                Err(error) => {
                    lines_skipped += 1;
                    errors.push(error);
                }
            }
        }
    }
    lines_skipped += harvest.lines_skipped;

    let superseded: Vec<String> = harvest.superseded.into_iter().collect();
    let events_inserted = match insert_events_superseding(conn, &superseded, &harvest.events) {
        Ok(inserted) => {
            match upsert_source_sessions(conn, SOURCE, &harvest.source_sessions) {
                Ok(()) => {
                    for states in parsed_states {
                        if let Err(error) = remember_file_states(conn, &states) {
                            errors.push(format!(
                                "{SOURCE}: Ledger file-state update failed: {error}"
                            ));
                        }
                    }
                }
                Err(error) => errors.push(format!(
                    "{SOURCE}: Session metadata update failed: {error}"
                )),
            }
            inserted
        }
        Err(error) => {
            errors.push(format!("{SOURCE}: Ledger insert failed: {error}"));
            0
        }
    };

    SourceScanResult {
        events_inserted,
        lines_skipped,
        error: (!errors.is_empty()).then(|| errors.join("; ")),
        ..Default::default()
    }
}

/// Merge one scanned Session's results into the run's harvest, keeping the
/// first writer for each dedup key across databases and legacy storage.
fn absorb(harvest: &mut OpencodeScan, scan: OpencodeScan) {
    harvest.lines_skipped += scan.lines_skipped;
    harvest.session_ids.extend(scan.session_ids);
    harvest.source_sessions.extend(scan.source_sessions);
    harvest.superseded.extend(scan.superseded);
    for event in scan.events {
        if harvest.seen_keys.insert(event.dedup_key.clone()) {
            harvest.events.push(event);
        }
    }
}

fn discover_databases(
    data_root: &Path,
    database_override: Option<&Path>,
) -> (Vec<PathBuf>, Vec<String>) {
    let mut databases = Vec::new();
    let mut errors = Vec::new();

    if let Some(path) = database_override {
        if path.is_file() {
            add_unique(&mut databases, path);
        }
        databases.sort();
        return (databases, errors);
    }

    if data_root.is_file() {
        if is_database_name(data_root) {
            add_unique(&mut databases, data_root);
        } else {
            errors.push(format!("{SOURCE}: unsupported Source Artifact file"));
        }
        return (databases, errors);
    }
    if !data_root.is_dir() {
        return (databases, errors);
    }

    let entries = match fs::read_dir(data_root) {
        Ok(entries) => entries,
        Err(error) => {
            errors.push(format!("{SOURCE}: data directory read failed: {error}"));
            return (databases, errors);
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && is_database_name(&path) {
            add_unique(&mut databases, &path);
        }
    }
    databases.sort();
    (databases, errors)
}

fn is_database_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name == "opencode.db"
        || (name.starts_with("opencode-") && name.ends_with(".db") && name.len() >= 13)
}

fn add_unique(paths: &mut Vec<PathBuf>, path: &Path) {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn scan_database(path: &Path) -> Result<OpencodeScan, String> {
    let conn = super::open_sqlite_artifact(SOURCE, path)?;
    for (table, columns) in SUPPORTED_SCHEMA {
        super::require_columns(SOURCE, &conn, table, columns)?;
    }

    let mut scan = OpencodeScan::default();
    let mut sessions = conn
        .prepare(
            "SELECT id, directory, time_created, time_updated
             FROM session ORDER BY id",
        )
        .map_err(|error| format!("{SOURCE}: session query failed: {error}"))?;
    let rows = sessions
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })
        .map_err(|error| format!("{SOURCE}: session read failed: {error}"))?;

    for row in rows {
        let (session_id, directory, created, updated) =
            row.map_err(|error| format!("{SOURCE}: session row failed: {error}"))?;
        scan.session_ids.insert(session_id.clone());

        let timestamp = session_timestamp(updated, created);
        let project = absolute_project(directory.as_deref());
        let created_at = created.map(normalize_epoch).or(timestamp).unwrap_or(0);
        let updated_at = timestamp.unwrap_or(created_at);
        scan.source_sessions.push(SourceSessionMeta {
            session_id: session_id.clone(),
            cwd: project.clone(),
            model: None,
            title: None,
            created_at,
            updated_at,
        });
        let Some(timestamp) = timestamp else {
            scan.lines_skipped += 1;
            continue;
        };
        let mut totals = SessionTotals::default();
        let mut messages = conn
            .prepare(
                "SELECT data, time_created FROM message
                 WHERE session_id = ?1 ORDER BY time_created, id",
            )
            .map_err(|error| format!("{SOURCE}: message query failed: {error}"))?;
        let message_rows = messages
            .query_map([&session_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                ))
            })
            .map_err(|error| format!("{SOURCE}: message read failed: {error}"))?;
        for message in message_rows {
            let (data, message_time) =
                message.map_err(|error| format!("{SOURCE}: message row failed: {error}"))?;
            match serde_json::from_str::<Value>(&data) {
                Ok(value) => match parse_message(&value) {
                    ParsedMessage::NotUsage => {}
                    ParsedMessage::Zero => scan.lines_skipped += 1,
                    ParsedMessage::Usage(snapshot) => totals.add(snapshot, message_time),
                    ParsedMessage::Invalid => scan.lines_skipped += 1,
                },
                Err(_) => scan.lines_skipped += 1,
            }
        }
        let (events, superseded) = totals.events(
            &session_id,
            timestamp,
            project,
            path,
        );
        scan.events.extend(events);
        scan.superseded.extend(superseded);
    }

    Ok(scan)
}

// The tables and columns the parse below is about to trust.
const SUPPORTED_SCHEMA: &[(&str, &[&str])] = &[
    ("session", &["id", "directory", "time_created", "time_updated"]),
    ("message", &["id", "session_id", "time_created", "data"]),
];

fn discover_legacy_sessions(root: &Path) -> Vec<PathBuf> {
    let session_root = legacy_session_root(root);
    let mut files = Vec::new();
    collect_json_files(&session_root, &mut files);
    files.sort();
    files
}

fn collect_json_files(root: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_json_files(&path, files);
        } else if file_type.is_file()
            && path.extension().and_then(|ext| ext.to_str()) == Some("json")
        {
            files.push(path);
        }
    }
}

fn legacy_session_root(root: &Path) -> PathBuf {
    if root.file_name().and_then(|name| name.to_str()) == Some("session") {
        root.to_path_buf()
    } else {
        root.join("session")
    }
}

fn legacy_message_root(root: &Path) -> PathBuf {
    if root.file_name().and_then(|name| name.to_str()) == Some("session") {
        root.parent().unwrap_or(root).join("message")
    } else {
        root.join("message")
    }
}

fn scan_legacy_session(
    path: &Path,
    message_root: PathBuf,
    modern_session_ids: &HashSet<String>,
) -> Result<OpencodeScan, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("{SOURCE}: legacy session read failed: {error}"))?;
    let value = serde_json::from_str::<Value>(&content)
        .map_err(|error| format!("{SOURCE}: malformed legacy session: {error}"))?;
    let session_id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_string)
        })
        .ok_or_else(|| format!("{SOURCE}: legacy session has no stable id"))?;
    if modern_session_ids.contains(&session_id) {
        return Ok(OpencodeScan::default());
    }

    let Some(timestamp) = json_session_timestamp(&value) else {
        return Ok(OpencodeScan {
            lines_skipped: 1,
            ..Default::default()
        });
    };
    let mut totals = SessionTotals::default();
    let mut scan = OpencodeScan::default();
    let messages = message_root.join(&session_id);
    let mut message_files = Vec::new();
    collect_json_files(&messages, &mut message_files);
    message_files.sort();
    for message_path in message_files {
        let content = match fs::read_to_string(&message_path) {
            Ok(content) => content,
            Err(_) => {
                scan.lines_skipped += 1;
                continue;
            }
        };
        let value = match serde_json::from_str::<Value>(&content) {
            Ok(value) => value,
            Err(_) => {
                scan.lines_skipped += 1;
                continue;
            }
        };
        match parse_message(&value) {
            ParsedMessage::NotUsage => {}
            ParsedMessage::Zero | ParsedMessage::Invalid => scan.lines_skipped += 1,
            ParsedMessage::Usage(snapshot) => totals.add(snapshot, None),
        }
    }

    let (events, superseded) = totals.events(
        &session_id,
        timestamp,
        absolute_project(value.get("directory").and_then(Value::as_str)),
        path,
    );
    scan.events = events;
    scan.superseded.extend(superseded);
    Ok(scan)
}

fn session_dedup_key(session_id: &str) -> String {
    format!("{SOURCE}:session:{session_id}")
}

/// GLOB pattern covering every split dedup key of a Session. GLOB (not LIKE)
/// so the `_` in OpenCode session ids stays literal.
fn split_dedup_glob(session_id: &str) -> String {
    format!("{SOURCE}:session:{session_id}:*")
}

fn split_dedup_key(session_id: &str, model: Option<&String>) -> String {
    match model {
        Some(model) => format!("{SOURCE}:session:{session_id}:model:{model}"),
        None => format!("{SOURCE}:session:{session_id}:unattributed"),
    }
}

/// The identity one Model group of a Session is booked under.
struct GroupBooking {
    dedup_key: String,
    session_id: String,
    model: Option<String>,
    timestamp: i64,
    project: Option<String>,
    source_file: PathBuf,
}

impl SessionTotals {
    fn add(&mut self, snapshot: UsageSnapshot, message_time: Option<i64>) {
        self.groups
            .entry(snapshot.model.clone())
            .or_default()
            .add(snapshot, message_time);
    }

    /// One UsageEvent per Model group, plus the GLOB patterns of stale rows
    /// this booking supersedes. Supersession is symmetric: whichever shape a
    /// Session now has (one group or several), every row of the other shape
    /// is replaced, so transitions in either direction leave no stale rows.
    fn events(
        &self,
        session_id: &str,
        session_timestamp: i64,
        project: Option<String>,
        source_file: &Path,
    ) -> (Vec<UsageEvent>, Vec<String>) {
        let mut events = Vec::new();
        if self.groups.is_empty() {
            return (events, Vec::new());
        }
        if self.groups.len() == 1 {
            let (model, totals) = self.groups.iter().next().expect("one group");
            let booking = GroupBooking {
                dedup_key: session_dedup_key(session_id),
                session_id: session_id.to_string(),
                model: model.clone(),
                timestamp: session_timestamp,
                project,
                source_file: source_file.to_path_buf(),
            };
            if let Some(event) = totals.event(booking) {
                events.push(event);
            }
            // Supersession must include the BARE session key alongside the GLOB:
            // single-model events live at `opencode:session:<sid>` (no `:*`)
            // and the GLOB `opencode:session:<sid>:*` requires the trailing `:`
            // to match — verified in SQLite GLOB semantics. Without the bare
            // pattern, a re-scan whose new shape lands on the bare key (e.g.
            // the alias translation rewrites `antigravity-…` to its real
            // Model id) does NOT delete the old Record, and `INSERT_SQL`'s
            // ON CONFLICT skips the `model` column on conflict (it's
            // `Immutable`), so the Record's model name never updates and stays
            // permanently Unpriced. Mirroring the multi-model branch makes
            // both directions symmetric.
            return (
                events,
                vec![session_dedup_key(session_id), split_dedup_glob(session_id)],
            );
        }
        for (model, totals) in &self.groups {
            let timestamp = totals
                .latest_message_ms
                .filter(|time| *time > 0)
                .map(normalize_epoch)
                .unwrap_or(session_timestamp);
            let booking = GroupBooking {
                dedup_key: split_dedup_key(session_id, model.as_ref()),
                session_id: session_id.to_string(),
                model: model.clone(),
                timestamp,
                project: project.clone(),
                source_file: source_file.to_path_buf(),
            };
            if let Some(event) = totals.event(booking) {
                events.push(event);
            }
        }
        (
            events,
            vec![session_dedup_key(session_id), split_dedup_glob(session_id)],
        )
    }
}

impl ModelTotals {
    fn add(&mut self, snapshot: UsageSnapshot, message_time: Option<i64>) {
        self.input = self.input.saturating_add(snapshot.input);
        self.output = self.output.saturating_add(snapshot.output);
        self.cache_read = self.cache_read.saturating_add(snapshot.cache_read);
        self.cache_write = self.cache_write.saturating_add(snapshot.cache_write);
        if let Some(time) = message_time.filter(|time| *time > 0) {
            self.latest_message_ms =
                Some(self.latest_message_ms.map_or(time, |latest| latest.max(time)));
        }

        match snapshot.reasoning {
            Some(reasoning) => {
                self.reasoning_seen = true;
                self.reasoning = self.reasoning.saturating_add(reasoning);
            }
            None => self.reasoning_incomplete = true,
        }
    }

    fn event(&self, booking: GroupBooking) -> Option<UsageEvent> {
        let total = self
            .input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write);
        if total <= 0 {
            return None;
        }
        let reasoning = if self.reasoning_seen && !self.reasoning_incomplete {
            Some(self.reasoning.min(self.output))
        } else {
            None
        };
        Some(UsageEvent {
            dedup_key: booking.dedup_key,
            source: SOURCE.to_string(),
            timestamp: booking.timestamp,
            model: booking.model,
            project: booking.project,
            api_calls: 1,
            input_tokens: self.input,
            output_tokens: self.output,
            cache_read_tokens: self.cache_read,
            cache_write_5m_tokens: self.cache_write,
            cache_write_1h_tokens: 0,
            source_file: booking.source_file.to_string_lossy().into_owned(),
            session_id: Some(booking.session_id),
            reasoning_tokens: reasoning,
            ctx: Default::default(),
        })
    }
}

fn parse_message(value: &Value) -> ParsedMessage {
    if value.get("role").and_then(Value::as_str) != Some("assistant") {
        return ParsedMessage::NotUsage;
    }
    let Some(tokens) = value.get("tokens").and_then(Value::as_object) else {
        return ParsedMessage::NotUsage;
    };
    let Some(input) = token_number(tokens, "input") else {
        return ParsedMessage::Invalid;
    };
    let Some(output) = token_number(tokens, "output") else {
        return ParsedMessage::Invalid;
    };
    let cache = tokens.get("cache").and_then(Value::as_object);
    let cache_read = cache
        .and_then(|cache| cache.get("read"))
        .or_else(|| tokens.get("cache_read"))
        .map_or(Some(0), nonnegative_i64);
    let cache_write = cache
        .and_then(|cache| cache.get("write"))
        .or_else(|| tokens.get("cache_write"))
        .map_or(Some(0), nonnegative_i64);
    let (Some(cache_read), Some(cache_write)) = (cache_read, cache_write) else {
        return ParsedMessage::Invalid;
    };
    let reasoning = match tokens.get("reasoning") {
        None => None,
        Some(value) => match nonnegative_i64(value) {
            Some(value) => Some(value),
            None => return ParsedMessage::Invalid,
        },
    };
    let model = value
        .get("modelID")
        .and_then(Value::as_str)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        // Antigravity Plugin logs `antigravity-…` routing aliases — translate
        // to the underlying real Model so pricing catalogs can resolve them.
        .map(|model| translate_antigravity(&model));
    let snapshot = UsageSnapshot {
        input,
        output,
        cache_read,
        cache_write,
        reasoning,
        model,
    };
    if input + output + cache_read + cache_write == 0 {
        ParsedMessage::Zero
    } else {
        ParsedMessage::Usage(snapshot)
    }
}

fn token_number(tokens: &serde_json::Map<String, Value>, key: &str) -> Option<i64> {
    tokens.get(key).map_or(Some(0), nonnegative_i64)
}

fn nonnegative_i64(value: &Value) -> Option<i64> {
    if let Some(value) = value.as_i64() {
        return (value >= 0).then_some(value);
    }
    if let Some(value) = value.as_u64() {
        return i64::try_from(value).ok();
    }
    value
        .as_f64()
        .filter(|value| value.is_finite() && *value >= 0.0 && value.fract() == 0.0)
        .and_then(|value| i64::try_from(value as u64).ok())
}

fn session_timestamp(updated: Option<i64>, created: Option<i64>) -> Option<i64> {
    updated
        .or(created)
        .filter(|t| *t > 0)
        .map(normalize_epoch)
}

fn json_session_timestamp(value: &Value) -> Option<i64> {
    value
        .get("time")
        .and_then(Value::as_object)
        .and_then(|time| time.get("updated").or_else(|| time.get("created")))
        .and_then(nonnegative_i64)
        .or_else(|| value.get("time_updated").and_then(nonnegative_i64))
        .or_else(|| value.get("time_created").and_then(nonnegative_i64))
        .filter(|t| *t > 0)
        .map(normalize_epoch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use std::fs;
    use tempfile::tempdir;

    type EventRow = (String, i64, Option<String>, i64, i64, i64, i64, String);

    #[test]
    fn translate_antigravity_resolves_known_aliases_and_their_variants() {
        // Bare ids (the Qoder precedent, CONTEXT.md §Model: raw name is
        // displayed); the bare-name rationale is in the function doc.
        assert_eq!(
            translate_antigravity("antigravity-gemini-3-pro"),
            "gemini-3-pro-preview"
        );
        assert_eq!(
            translate_antigravity("antigravity-gemini-3-pro-preview"),
            "gemini-3-pro-preview"
        );
        assert_eq!(
            translate_antigravity("antigravity-gemini-3-pro-low"),
            "gemini-3-pro-preview"
        );
        assert_eq!(
            translate_antigravity("antigravity-gemini-3-pro-high"),
            "gemini-3-pro-preview"
        );
        assert_eq!(
            translate_antigravity("antigravity-gemini-3-flash"),
            "gemini-3-flash-preview"
        );
        assert_eq!(
            translate_antigravity("antigravity-gemini-3-flash-preview"),
            "gemini-3-flash-preview"
        );
        // Bare Gemini variant levels all collapse to the same base.
        for variant in ["minimal", "low", "medium", "high"] {
            assert_eq!(
                translate_antigravity(&format!("antigravity-gemini-3-flash-{variant}")),
                "gemini-3-flash-preview",
                "gemini-3-flash variant `{variant}` must resolve to the base Model"
            );
        }
        assert_eq!(
            translate_antigravity("antigravity-claude-sonnet-4-5"),
            "claude-sonnet-4-5"
        );
        // -thinking alone and -thinking-{level} all share one catalog row.
        assert_eq!(
            translate_antigravity("antigravity-claude-sonnet-4-5-thinking"),
            "claude-sonnet-4-5"
        );
        for variant in ["low", "medium", "high", "max"] {
            assert_eq!(
                translate_antigravity(&format!(
                    "antigravity-claude-sonnet-4-5-thinking-{variant}"
                )),
                "claude-sonnet-4-5",
                "sonnet-4-5 thinking variant `{variant}` must resolve to base Model"
            );
        }
        // Opus has no non-thinking surface; -thinking is the only form.
        assert_eq!(
            translate_antigravity("antigravity-claude-opus-4-5-thinking-low"),
            "claude-opus-4-5"
        );
        assert_eq!(
            translate_antigravity("antigravity-claude-opus-4-5-thinking-high"),
            "claude-opus-4-5"
        );
    }

    #[test]
    fn translate_antigravity_passes_non_antigravity_models_through() {
        // Real model ids without the alias prefix must not be rewritten —
        // OpenCode also logs Anthropic / OpenAI / Z.ai model ids directly.
        for raw in [
            "claude-sonnet-4-5",
            "gpt-5.2",
            "zai/glm-4.7",
            "glm-4.7-free",
            "opencode/big-pickle",
            "antigravity", // prefix with a trailing hyphen stripped below
        ] {
            assert_eq!(translate_antigravity(raw), raw, "must not rewrite `{raw}`");
        }
        // The prefix `antigravity-` requires the trailing `-`; a bare
        // `antigravity` token is not an alias and stays untouched.
        assert_eq!(
            translate_antigravity("antigravity"),
            "antigravity",
            "bare `antigravity` is not an alias — leave it for catalog to resolve"
        );
    }

    #[test]
    fn translate_antigravity_leaves_unknown_aliases_raw_for_unpriced_signal() {
        // A future Antigravity Model (e.g. `antigravity-gemini-4`) lands here
        // until the mapping is updated; the Pricing tab then surfaces it as
        // Unpriced, mirroring `adapters::antigravity::resolve_model`'s
        // pass-through contract.
        assert_eq!(
            translate_antigravity("antigravity-gemini-4"),
            "antigravity-gemini-4",
            "unknown alias must pass through so the rename signal stays visible"
        );
        assert_eq!(
            translate_antigravity("antigravity-claude-opus-5-thinking-max"),
            "antigravity-claude-opus-5-thinking-max",
            "an Opus-class future alias must not silently map to a wrong base"
        );
    }

    #[test]
    fn parse_message_translates_antigravity_alias_on_ingest() {
        // The translator is invoked inside parse_message — confirm end to end
        // that an Antigravity-shaped message produces a UsageEvent carrying the
        // canonical catalog key, not the routing alias.
        let value: serde_json::Value = serde_json::from_str(
            r#"{
                "role": "assistant",
                "modelID": "antigravity-gemini-3-pro-low",
                "tokens": {"input": 10, "output": 4, "cache": {"read": 1, "write": 0}}
            }"#,
        )
        .unwrap();
        let parsed = parse_message(&value);
        let snapshot = match parsed {
            ParsedMessage::Usage(s) => s,
            _ => panic!("expected Usage message, got a non-usage variant"),
        };
        assert_eq!(
            snapshot.model.as_deref(),
            Some("gemini-3-pro-preview"),
            "parse_message must translate the alias before booking"
        );
        assert_eq!(snapshot.input, 10);
        assert_eq!(snapshot.output, 4);
        assert_eq!(snapshot.cache_read, 1);
        assert_eq!(snapshot.cache_write, 0);

        // A plain non-Antigravity model id is not rewritten.
        let plain: serde_json::Value = serde_json::from_str(
            r#"{
                "role": "assistant",
                "modelID": "claude-sonnet-4-5",
                "tokens": {"input": 1, "output": 2, "cache": {"read": 0, "write": 0}}
            }"#,
        )
        .unwrap();
        let parsed_plain = match parse_message(&plain) {
            ParsedMessage::Usage(s) => s,
            _ => panic!("plain model must parse as Usage"),
        };
        assert_eq!(parsed_plain.model.as_deref(), Some("claude-sonnet-4-5"));
    }

    #[test]
    fn single_model_session_supersession_clears_the_bare_dedup_key() {
        // Regression test for the asymmetry fixed by adding
        // `session_dedup_key(session_id)` to the single-model supersession
        // list (see the comment on that branch in `events()`). A stale
        // single-model Record — e.g. an Antigravity alias translation
        // flipping the model column — must be deleted and re-inserted,
        // never left in the Ledger under its old name.
        let tmp = tempdir().unwrap();
        let data_root = tmp.path().join("opencode");
        fs::create_dir_all(&data_root).unwrap();
        let db_path = data_root.join("opencode.db");
        create_database(&db_path);
        {
            let db = Connection::open(&db_path).unwrap();
            insert_session(&db, "alias-switch", "/private/alias", 1_780_000_400_000);
            insert_message(
                &db,
                "m1",
                "alias-switch",
                r#"{"role":"assistant","modelID":"future-unknown-alias","tokens":{"input":12,"output":5,"cache":{"read":0,"write":0}}}"#,
            );
            insert_message(
                &db,
                "m2",
                "alias-switch",
                r#"{"role":"assistant","modelID":"future-unknown-alias","tokens":{"input":3,"output":1,"cache":{"read":0,"write":0}}}"#,
            );
        }

        let ledger_path = tmp.path().join("ledger.db");
        let mut ledger = crate::db::open_db(&ledger_path).unwrap();
        // First scan — model id is unknown so it lands Unpriced in the
        // catalog and the Ledger Record carries the raw alias name.
        let first = scan_opencode(&mut ledger, &data_root, &data_root.join("storage"), None);
        assert_eq!(first.events_inserted, 1);
        let first_row = ledger
            .query_row(
                "SELECT dedup_key, model FROM events WHERE session_id='alias-switch'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .unwrap();
        let (first_key, first_model) = first_row;
        assert_eq!(
            first_key, "opencode:session:alias-switch",
            "single-model Session lands at the bare dedup_key (no `:*`)"
        );
        assert_eq!(first_model.as_deref(), Some("future-unknown-alias"));

        // A new catalog revision (or, in our real-world scenario, the
        // translator's mapping being extended) renames the underlying
        // model: simulate the user side rewriting the message JSON.
        {
            let db = Connection::open(&db_path).unwrap();
            db.execute(
                "UPDATE message SET data = ?1 WHERE id = 'm1'",
                params![r#"{"role":"assistant","modelID":"proper-model-name","tokens":{"input":12,"output":5,"cache":{"read":0,"write":0}}}"#],
            )
            .unwrap();
            db.execute(
                "UPDATE message SET data = ?1 WHERE id = 'm2'",
                params![r#"{"role":"assistant","modelID":"proper-model-name","tokens":{"input":3,"output":1,"cache":{"read":0,"write":0}}}"#],
            )
            .unwrap();
        }

        let second = scan_opencode(&mut ledger, &data_root, &data_root.join("storage"), None);
        assert!(
            second.error.is_none(),
            "re-scan must not report an error: {:?}",
            second.error
        );
        // The bare key is still the right identity for this single-model
        // Session — translate the model id and reuse the key. The Record's
        // model column must move to the new name; nothing about the
        // pre-fix bug (a stale Unpriced Record surviving because the GLOB
        // missed the bare key) may remain.
        let n: i64 = ledger
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id='alias-switch'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "exactly one Record per single-model Session");
        let second_row = ledger
            .query_row(
                "SELECT dedup_key, model FROM events WHERE session_id='alias-switch'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .unwrap();
        let (second_key, second_model) = second_row;
        assert_eq!(second_key, first_key, "single-Model key is stable");
        assert_eq!(
            second_model.as_deref(),
            Some("proper-model-name"),
            "the model column must be rewritten by the supersession \
             re-scan — a stale Record would leave it Unpriced forever"
        );
    }

    fn create_database(path: &Path) {
        let db = Connection::open(path).unwrap();
        db.execute_batch(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY,
                directory TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                data TEXT NOT NULL
            );",
        )
        .unwrap();
    }

    fn insert_session(db: &Connection, id: &str, directory: &str, updated_ms: i64) {
        db.execute(
            "INSERT INTO session (id, directory, time_created, time_updated)
             VALUES (?1, ?2, ?3, ?4)",
            params![id, directory, updated_ms - 1_000, updated_ms],
        )
        .unwrap();
    }

    fn insert_message(db: &Connection, id: &str, session_id: &str, data: &str) {
        insert_message_at(db, id, session_id, 1_780_000_000_000, data);
    }

    fn insert_message_at(db: &Connection, id: &str, session_id: &str, time_ms: i64, data: &str) {
        db.execute(
            "INSERT INTO message (id, session_id, time_created, data)
             VALUES (?1, ?2, ?3, ?4)",
            params![id, session_id, time_ms, data],
        )
        .unwrap();
    }

    fn write_json(path: &Path, value: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }

    #[test]
    fn current_and_legacy_sessions_are_normalized_without_persisting_content() {
        let tmp = tempdir().unwrap();
        let data_root = tmp.path().join("opencode");
        fs::create_dir_all(&data_root).unwrap();

        let current = data_root.join("opencode.db");
        create_database(&current);
        let current = fs::canonicalize(current).unwrap();
        let db = Connection::open(&current).unwrap();
        insert_session(&db, "modern-overlap", "/private/modern", 1_780_000_100_000);
        insert_session(&db, "modern-unknown", "/private/unknown", 1_780_000_200_000);
        insert_session(&db, "modern-zero", "/private/zero", 1_780_000_300_000);
        insert_message(
            &db,
            "m1",
            "modern-overlap",
            r#"{"role":"assistant","modelID":"opencode-model","tokens":{"input":10,"output":4,"reasoning":2,"cache":{"read":3,"write":1},"private":"PRIVATE_RESPONSE_SHOULD_NOT_PERSIST"}}"#,
        );
        insert_message(
            &db,
            "m2",
            "modern-overlap",
            r#"{"role":"user","data":"PRIVATE_PROMPT_SHOULD_NOT_PERSIST"}"#,
        );
        insert_message(
            &db,
            "m3",
            "modern-overlap",
            r#"{"role":"assistant","modelID":"opencode-model","tokens":{"input":20,"output":6,"reasoning":1,"cache":{"read":7,"write":0}}}"#,
        );
        insert_message(
            &db,
            "m4",
            "modern-unknown",
            r#"{"role":"assistant","modelID":"one","tokens":{"input":1,"output":0,"cache":{"read":0,"write":0}}}"#,
        );
        insert_message(
            &db,
            "m5",
            "modern-unknown",
            r#"{"role":"assistant","modelID":"two","tokens":{"input":0,"output":2,"cache":{"read":0,"write":0}}}"#,
        );
        insert_message(
            &db,
            "m6",
            "modern-zero",
            r#"{"role":"assistant","modelID":"ignored","tokens":{"input":0,"output":0,"cache":{"read":0,"write":0}}}"#,
        );
        drop(db);

        let legacy = data_root.join("storage");
        write_json(
            &legacy.join("session/project/modern-overlap.json"),
            r#"{"id":"modern-overlap","directory":"/private/legacy","time":{"updated":1999000000000},"private":"PRIVATE_REASONING_SHOULD_NOT_PERSIST"}"#,
        );
        write_json(
            &legacy.join("message/modern-overlap/legacy.json"),
            r#"{"role":"assistant","modelID":"legacy-model","tokens":{"input":999,"output":999,"cache":{"read":999,"write":999}}}"#,
        );
        write_json(
            &legacy.join("session/project/legacy-only.json"),
            r#"{"id":"legacy-only","directory":"/private/legacy-only","time":{"created":1780000300000,"updated":1780000400000}}"#,
        );
        write_json(
            &legacy.join("message/legacy-only/legacy.json"),
            r#"{"role":"assistant","modelID":"legacy-model","tokens":{"input":5,"output":2,"cache":{"read":4,"write":0}},"private":"PRIVATE_TOOL_RESULT_SHOULD_NOT_PERSIST"}"#,
        );

        let ledger_path = tmp.path().join("ledger.db");
        let mut ledger = crate::db::open_db(&ledger_path).unwrap();
        let first = scan_opencode(&mut ledger, &data_root, &legacy, None);
        assert_eq!(first.events_inserted, 4);
        assert!(
            first.error.is_none(),
            "unexpected scan error: {:?}",
            first.error
        );
        assert!(first.lines_skipped > 0);

        let rows: Vec<EventRow> = ledger
            .prepare(
                "SELECT session_id, timestamp, model, input_tokens, output_tokens,
                        cache_read_tokens, cache_write_5m_tokens, source_file
                 FROM events WHERE source = 'opencode' ORDER BY session_id, model",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(rows.len(), 4);

        let overlap = rows.iter().find(|row| row.0 == "modern-overlap").unwrap();
        assert_eq!(overlap.1, 1_780_000_100);
        assert_eq!(overlap.2.as_deref(), Some("opencode-model"));
        assert_eq!(
            (overlap.3, overlap.4, overlap.5, overlap.6),
            (30, 10, 10, 1)
        );
        assert!(overlap.7.ends_with("opencode.db"));
        assert_eq!(
            ledger
                .query_row(
                    "SELECT reasoning_tokens FROM events WHERE session_id = 'modern-overlap'",
                    [],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .unwrap(),
            Some(3)
        );

        let unknown: Vec<&EventRow> = rows
            .iter()
            .filter(|row| row.0 == "modern-unknown")
            .collect();
        assert_eq!(unknown.len(), 2, "mixed-model session splits per Model");
        let one = unknown.iter().find(|row| row.2.as_deref() == Some("one")).unwrap();
        let two = unknown.iter().find(|row| row.2.as_deref() == Some("two")).unwrap();
        assert_eq!((one.3, one.4), (1, 0));
        assert_eq!((two.3, two.4), (0, 2));

        let legacy_only = rows.iter().find(|row| row.0 == "legacy-only").unwrap();
        assert_eq!(legacy_only.1, 1_780_000_400);
        assert_eq!(legacy_only.2.as_deref(), Some("legacy-model"));
        assert_eq!((legacy_only.3, legacy_only.4, legacy_only.5), (5, 2, 4));

        write_json(
            &legacy.join("message/modern-overlap/legacy.json"),
            r#"{"role":"assistant","modelID":"changed-legacy-model","tokens":{"input":1000,"output":1000,"cache":{"read":1000,"write":1000}}}"#,
        );
        fs::write(&current, "not sqlite").unwrap();
        let mut current_state = crate::adapters::file_state_of(&current);
        current_state.byte_offset = PARSER_VERSION;
        crate::db::set_file_state(&ledger, &current.to_string_lossy(), current_state).unwrap();
        let second = scan_opencode(&mut ledger, &data_root, &legacy, None);
        assert!(
            second.error.is_none(),
            "an unchanged modern database was reopened: {:?}",
            second.error
        );
        assert_eq!(second.events_inserted, 0);
        assert_eq!(
            ledger
                .query_row(
                    "SELECT COUNT(*) FROM events WHERE source = 'opencode'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            4
        );
        assert_eq!(
            ledger
                .query_row(
                    "SELECT model FROM events WHERE session_id = 'modern-overlap'",
                    [],
                    |row| row.get::<_, Option<String>>(0),
                )
                .unwrap()
                .as_deref(),
            Some("opencode-model"),
            "an unchanged modern database still suppresses overlapping legacy storage"
        );

        ledger.execute_batch("DROP TABLE source_sessions;").unwrap();
        write_json(
            &legacy.join("message/modern-overlap/legacy.json"),
            r#"{"role":"assistant","modelID":"newer-legacy-model","tokens":{"input":2000,"output":2000,"cache":{"read":2000,"write":2000}}}"#,
        );
        let third = scan_opencode(&mut ledger, &data_root, &legacy, None);
        assert!(third
            .error
            .as_deref()
            .is_some_and(|error| error.contains("Session metadata read failed")));
        assert_eq!(third.events_inserted, 0);
        assert_eq!(
            ledger
                .query_row(
                    "SELECT model FROM events WHERE session_id = 'modern-overlap'",
                    [],
                    |row| row.get::<_, Option<String>>(0),
                )
                .unwrap()
                .as_deref(),
            Some("opencode-model"),
            "legacy storage must fail closed when modern Session metadata is unavailable"
        );

        drop(ledger);
        let durable = ["", "-wal", "-shm"]
            .into_iter()
            .filter_map(|suffix| fs::read(format!("{}{}", ledger_path.display(), suffix)).ok())
            .flatten()
            .collect::<Vec<_>>();
        for marker in [
            "PRIVATE_PROMPT_SHOULD_NOT_PERSIST",
            "PRIVATE_RESPONSE_SHOULD_NOT_PERSIST",
            "PRIVATE_REASONING_SHOULD_NOT_PERSIST",
            "PRIVATE_TOOL_RESULT_SHOULD_NOT_PERSIST",
        ] {
            assert!(!durable
                .windows(marker.len())
                .any(|window| window == marker.as_bytes()));
        }
    }

    #[test]
    fn unchanged_database_is_not_reopened() {
        let tmp = tempdir().unwrap();
        let data_root = tmp.path().join("opencode");
        fs::create_dir_all(&data_root).unwrap();
        let database_path = data_root.join("opencode.db");
        create_database(&database_path);
        let database_path = fs::canonicalize(database_path).unwrap();
        let database = Connection::open(&database_path).unwrap();
        insert_session(&database, "session", "/project", 1_780_000_100_000);
        insert_message(
            &database,
            "message",
            "session",
            r#"{"role":"assistant","modelID":"model","tokens":{"input":1,"output":1,"cache":{"read":0,"write":0}}}"#,
        );
        drop(database);

        let legacy = data_root.join("storage");
        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        assert_eq!(
            scan_opencode(&mut ledger, &data_root, &legacy, None).events_inserted,
            1
        );
        let saved = crate::db::get_file_state(&ledger, &database_path.to_string_lossy())
            .unwrap()
            .expect("a successful scan records the database fingerprint");

        Connection::open(&database_path)
            .unwrap()
            .execute_batch("DROP TABLE session;")
            .unwrap();
        let mut current = crate::adapters::file_state_of(&database_path);
        current.byte_offset = saved.byte_offset;
        crate::db::set_file_state(&ledger, &database_path.to_string_lossy(), current).unwrap();

        let second = scan_opencode(&mut ledger, &data_root, &legacy, None);
        assert!(second.error.is_none(), "unchanged database was reopened");
        assert_eq!(second.events_inserted, 0);
    }

    #[test]
    fn legacy_sessions_are_not_skipped_by_aggregate_file_state() {
        let tmp = tempdir().unwrap();
        let data_root = tmp.path().join("opencode");
        let legacy = data_root.join("storage");
        let session_path = legacy.join("session/project/legacy.json");
        write_json(
            &session_path,
            r#"{"id":"legacy","directory":"/project","time":{"updated":1780000400000}}"#,
        );
        let message_path = legacy.join("message/legacy/message.json");
        write_json(
            &message_path,
            r#"{"role":"assistant","modelID":"model","tokens":{"input":5,"output":2,"cache":{"read":0,"write":0}}}"#,
        );

        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        let mut state = crate::adapters::file_state_of(&session_path);
        let message_state = crate::adapters::file_state_of(&message_path);
        state.size += message_state.size + 1;
        state.mtime = state.mtime.max(message_state.mtime);
        state.byte_offset = PARSER_VERSION;
        crate::db::set_file_state(&ledger, &session_path.to_string_lossy(), state).unwrap();

        let result = scan_opencode(&mut ledger, &data_root, &legacy, None);
        assert_eq!(result.events_inserted, 1);
        assert!(result.error.is_none());
    }

    #[test]
    fn file_state_failure_is_reported() {
        let tmp = tempdir().unwrap();
        let data_root = tmp.path().join("opencode");
        fs::create_dir_all(&data_root).unwrap();
        let database_path = data_root.join("opencode.db");
        create_database(&database_path);
        let database = Connection::open(&database_path).unwrap();
        insert_session(&database, "session", "/project", 1_780_000_100_000);
        insert_message(
            &database,
            "message",
            "session",
            r#"{"role":"assistant","modelID":"model","tokens":{"input":1,"output":1,"cache":{"read":0,"write":0}}}"#,
        );
        drop(database);

        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        ledger.execute("DROP TABLE scanned_files", []).unwrap();
        let result = scan_opencode(
            &mut ledger,
            &data_root,
            &data_root.join("storage"),
            None,
        );

        assert_eq!(result.events_inserted, 1);
        assert!(result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("file-state update failed")));
    }

    #[test]
    fn unsupported_database_reports_an_opencode_warning_without_blocking_legacy_usage() {
        let tmp = tempdir().unwrap();
        let data_root = tmp.path().join("opencode");
        fs::create_dir_all(&data_root).unwrap();
        let bad = data_root.join("opencode-nightly.db");
        Connection::open(&bad)
            .unwrap()
            .execute_batch("CREATE TABLE unrelated (id TEXT PRIMARY KEY);")
            .unwrap();

        let legacy = data_root.join("storage");
        write_json(
            &legacy.join("session/project/legacy-only.json"),
            r#"{"id":"legacy-only","directory":"/private/legacy-only","time":{"updated":1780000400000}}"#,
        );
        write_json(
            &legacy.join("message/legacy-only/legacy.json"),
            r#"{"role":"assistant","modelID":"legacy-model","tokens":{"input":5,"output":2,"cache":{"read":4,"write":0}}}"#,
        );

        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        let result = scan_opencode(&mut ledger, &data_root, &legacy, None);
        assert_eq!(result.events_inserted, 1);
        assert!(result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("opencode") && error.contains("unsupported")));
    }

    #[test]
    fn alternate_channel_databases_are_scanned_as_current_open_code_artifacts() {
        let tmp = tempdir().unwrap();
        let data_root = tmp.path().join("opencode");
        fs::create_dir_all(&data_root).unwrap();
        let channel = data_root.join("opencode-nightly.db");
        create_database(&channel);
        let db = Connection::open(&channel).unwrap();
        insert_session(
            &db,
            "nightly-session",
            "/private/nightly",
            1_780_000_500_000,
        );
        insert_message(
            &db,
            "nightly-message",
            "nightly-session",
            r#"{"role":"assistant","modelID":"nightly-model","tokens":{"input":3,"output":1,"cache":{"read":0,"write":0}}}"#,
        );
        drop(db);

        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        let result = scan_opencode(&mut ledger, &data_root, &data_root.join("storage"), None);
        assert_eq!(result.events_inserted, 1);
        assert!(
            result.error.is_none(),
            "unexpected scan error: {:?}",
            result.error
        );
        assert_eq!(
            ledger
                .query_row(
                    "SELECT model FROM events WHERE source = 'opencode'",
                    [],
                    |row| row.get::<_, Option<String>>(0),
                )
                .unwrap()
                .as_deref(),
            Some("nightly-model")
        );
    }

    #[test]
    fn session_gaining_a_second_model_supersedes_the_stale_aggregate() {
        let tmp = tempdir().unwrap();
        let data_root = tmp.path().join("opencode");
        fs::create_dir_all(&data_root).unwrap();
        let current = data_root.join("opencode.db");
        create_database(&current);
        let db = Connection::open(&current).unwrap();
        insert_session(&db, "mixed", "/private/mixed", 1_780_000_900_000);
        insert_message_at(
            &db,
            "m1",
            "mixed",
            1_780_000_500_000,
            r#"{"role":"assistant","modelID":"model-a","tokens":{"input":10,"output":4,"cache":{"read":0,"write":0}}}"#,
        );
        drop(db);

        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        let first = scan_opencode(&mut ledger, &data_root, &data_root.join("storage"), None);
        assert_eq!(first.events_inserted, 1);
        assert_eq!(
            ledger
                .query_row(
                    "SELECT dedup_key, model FROM events WHERE source = 'opencode'",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                )
                .unwrap(),
            ("opencode:session:mixed".to_string(), Some("model-a".to_string()))
        );

        let db = Connection::open(&current).unwrap();
        insert_message_at(
            &db,
            "m2",
            "mixed",
            1_780_000_700_000,
            r#"{"role":"assistant","modelID":"model-b","tokens":{"input":2,"output":8,"cache":{"read":0,"write":0}}}"#,
        );
        drop(db);

        let second = scan_opencode(&mut ledger, &data_root, &data_root.join("storage"), None);
        assert!(
            second.error.is_none(),
            "unexpected scan error: {:?}",
            second.error
        );
        let rows: Vec<(String, Option<String>, i64, i64, i64)> = ledger
            .prepare(
                "SELECT dedup_key, model, timestamp, input_tokens, output_tokens
                 FROM events WHERE source = 'opencode' ORDER BY model",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(rows.len(), 2, "stale aggregate row is superseded, not kept");
        assert_eq!(
            rows[0],
            (
                "opencode:session:mixed:model:model-a".to_string(),
                Some("model-a".to_string()),
                1_780_000_500,
                10,
                4
            )
        );
        assert_eq!(
            rows[1],
            (
                "opencode:session:mixed:model:model-b".to_string(),
                Some("model-b".to_string()),
                1_780_000_700,
                2,
                8
            )
        );

        let third = scan_opencode(&mut ledger, &data_root, &data_root.join("storage"), None);
        assert_eq!(third.events_inserted, 0);
        assert_eq!(
            ledger
                .query_row(
                    "SELECT COUNT(*) FROM events WHERE source = 'opencode'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );

        let db = Connection::open(&current).unwrap();
        db.execute("DELETE FROM message WHERE id = 'm2'", [])
            .unwrap();
        drop(db);

        let fourth = scan_opencode(&mut ledger, &data_root, &data_root.join("storage"), None);
        assert!(
            fourth.error.is_none(),
            "unexpected scan error: {:?}",
            fourth.error
        );
        assert_eq!(
            ledger
                .query_row(
                    "SELECT dedup_key, model, input_tokens, output_tokens
                     FROM events WHERE source = 'opencode'",
                    [],
                    |row| Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    )),
                )
                .unwrap(),
            (
                "opencode:session:mixed".to_string(),
                Some("model-a".to_string()),
                10,
                4
            ),
            "split rows are superseded when the session is single-Model again"
        );
    }

    #[test]
    fn mixed_session_books_a_model_less_group_as_unattributed() {
        let tmp = tempdir().unwrap();
        let data_root = tmp.path().join("opencode");
        fs::create_dir_all(&data_root).unwrap();
        let current = data_root.join("opencode.db");
        create_database(&current);
        let db = Connection::open(&current).unwrap();
        insert_session(&db, "split", "/private/split", 1_780_000_900_000);
        insert_message(
            &db,
            "m1",
            "split",
            r#"{"role":"assistant","modelID":"model-a","tokens":{"input":10,"output":1,"cache":{"read":0,"write":0}}}"#,
        );
        insert_message(
            &db,
            "m2",
            "split",
            r#"{"role":"assistant","tokens":{"input":3,"output":7,"cache":{"read":0,"write":0}}}"#,
        );
        drop(db);

        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        let result = scan_opencode(&mut ledger, &data_root, &data_root.join("storage"), None);
        assert_eq!(result.events_inserted, 2);
        let rows: Vec<(String, Option<String>, i64, i64)> = ledger
            .prepare(
                "SELECT dedup_key, model, input_tokens, output_tokens
                 FROM events WHERE source = 'opencode' ORDER BY dedup_key",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0],
            (
                "opencode:session:split:model:model-a".to_string(),
                Some("model-a".to_string()),
                10,
                1
            )
        );
        assert_eq!(
            rows[1],
            (
                "opencode:session:split:unattributed".to_string(),
                None,
                3,
                7
            ),
            "the model-less group books NULL, never a sentinel"
        );
    }

    #[test]
    fn legacy_mixed_session_splits_per_model_at_the_session_timestamp() {
        let tmp = tempdir().unwrap();
        let data_root = tmp.path().join("opencode");
        fs::create_dir_all(&data_root).unwrap();
        let legacy = data_root.join("storage");
        write_json(
            &legacy.join("session/project/legacy-mixed.json"),
            r#"{"id":"legacy-mixed","directory":"/private/legacy-mixed","time":{"updated":1780000600000}}"#,
        );
        write_json(
            &legacy.join("message/legacy-mixed/a.json"),
            r#"{"role":"assistant","modelID":"model-a","tokens":{"input":5,"output":1,"cache":{"read":0,"write":0}}}"#,
        );
        write_json(
            &legacy.join("message/legacy-mixed/b.json"),
            r#"{"role":"assistant","modelID":"model-b","tokens":{"input":1,"output":7,"cache":{"read":2,"write":0}}}"#,
        );

        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        let result = scan_opencode(&mut ledger, &data_root, &legacy, None);
        assert_eq!(result.events_inserted, 2);
        assert!(
            result.error.is_none(),
            "unexpected scan error: {:?}",
            result.error
        );
        let rows: Vec<(Option<String>, i64, i64, i64)> = ledger
            .prepare(
                "SELECT model, timestamp, input_tokens, output_tokens
                 FROM events WHERE source = 'opencode' ORDER BY model",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], (Some("model-a".to_string()), 1_780_000_600, 5, 1));
        assert_eq!(rows[1], (Some("model-b".to_string()), 1_780_000_600, 1, 7));
    }
}
