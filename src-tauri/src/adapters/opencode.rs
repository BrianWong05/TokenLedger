use std::collections::HashSet;
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
const PARSER_VERSION: i64 = 2;

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
        let mut messages = conn
            .prepare(
                "SELECT id, data, time_created FROM message
                 WHERE session_id = ?1 ORDER BY time_created, id",
            )
            .map_err(|error| format!("{SOURCE}: message query failed: {error}"))?;
        let message_rows = messages
            .query_map([&session_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            })
            .map_err(|error| format!("{SOURCE}: message read failed: {error}"))?;
        let mut booked: Vec<String> = Vec::new();
        for message in message_rows {
            let (message_id, data, message_time) =
                message.map_err(|error| format!("{SOURCE}: message row failed: {error}"))?;
            match serde_json::from_str::<Value>(&data) {
                Ok(value) => match parse_message(&value) {
                    ParsedMessage::NotUsage => {}
                    ParsedMessage::Zero => scan.lines_skipped += 1,
                    ParsedMessage::Usage(snapshot) => {
                        let dedup_key = message_dedup_key(&session_id, &message_id);
                        booked.push(dedup_key.clone());
                        scan.events.push(snapshot.event(MessageBooking {
                            dedup_key,
                            session_id: session_id.clone(),
                            timestamp: message_timestamp(message_time, timestamp),
                            project: project.clone(),
                            source_file: path.to_path_buf(),
                        }));
                    }
                    ParsedMessage::Invalid => scan.lines_skipped += 1,
                },
                Err(_) => scan.lines_skipped += 1,
            }
        }
        // Only a Session that booked something supersedes its own aggregate.
        // A parse that suddenly yields nothing — a renamed token field — must
        // leave the pre-TOKL-24 Record standing, on TOKL-28's reasoning: the
        // aggregate is then the only surviving evidence, and this parser can no
        // longer re-derive it.
        if !booked.is_empty() {
            scan.superseded
                .extend(supersedes_session_aggregates(&session_id, &booked));
        }
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
    let mut scan = OpencodeScan::default();
    let mut booked: Vec<String> = Vec::new();
    let project = absolute_project(value.get("directory").and_then(Value::as_str));
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
            ParsedMessage::Usage(snapshot) => {
                let message_id = legacy_message_id(&value, &message_path, &messages);
                let dedup_key = message_dedup_key(&session_id, &message_id);
                booked.push(dedup_key.clone());
                scan.events.push(snapshot.event(MessageBooking {
                    dedup_key,
                    session_id: session_id.clone(),
                    timestamp: message_timestamp(json_message_created_ms(&value), timestamp),
                    project: project.clone(),
                    source_file: path.to_path_buf(),
                }));
            }
        }
    }
    if !booked.is_empty() {
        scan.superseded
            .extend(supersedes_session_aggregates(&session_id, &booked));
    }
    Ok(scan)
}

/// Every stale booking shape a re-Scan of this Session replaces: the
/// pre-TOKL-24 per-Session aggregate at the BARE key, and — via the GLOB —
/// both its per-Model splits and this Session's own per-Request Records.
///
/// The bare key must be listed alongside the GLOB, not folded into it:
/// `opencode:session:<sid>:*` needs the trailing `:` to match, so the GLOB
/// alone leaves the single-Model aggregate behind. That matters because
/// `INSERT_SQL`'s ON CONFLICT skips the `model` column (it is `Immutable`) —
/// a Record that is not superseded keeps its old Model name for ever and
/// stays permanently Unpriced. GLOB, not LIKE, so the `_` in OpenCode
/// session ids stays literal.
///
/// Deliberately NOT the blanket `…:session:<sid>:*`. That pattern also matches
/// the `…:message:<id>` keys this parser writes, so a Request the Artifact no
/// longer holds — OpenCode compacts a Session's messages, and `message` rows
/// CASCADE — had its Record DELETEd and never re-INSERTed. CONTEXT.md (Ledger)
/// allows a scan to delete a Record only "to supersede a coarser Record with
/// the finer Records the Source proves stand in its place"; a pruned Request
/// has nothing standing in its place, so it keeps the Record it already
/// proved. Naming the two legacy split shapes instead of globbing them is what
/// lets the pruned Record survive.
///
/// `booked` carries the key of every Request this parse did book, so a present
/// Request is still DELETEd and re-INSERTed. That preserves the whole reason
/// the per-Request key is Session-scoped: `translate_antigravity` learning a
/// new alias must update the Record's Model rather than land on ON CONFLICT,
/// which skips the Immutable `model` column and would freeze it Unpriced.
fn supersedes_session_aggregates(session_id: &str, booked: &[String]) -> Vec<String> {
    let mut patterns = Vec::with_capacity(booked.len() + 3);
    patterns.push(format!("{SOURCE}:session:{session_id}"));
    patterns.push(format!("{SOURCE}:session:{session_id}:model:*"));
    patterns.push(format!("{SOURCE}:session:{session_id}:unattributed"));
    patterns.extend(booked.iter().cloned());
    patterns
}

/// One Usage Record per Request, keyed on the Message that is that Request.
///
/// Session-scoped on purpose. The scope keeps every Record of a Session under
/// `supersedes_session_aggregates`' GLOB, so a re-Scan DELETEs and re-INSERTs
/// the row instead of landing on the ON CONFLICT path that skips the
/// `model` column. A flat `opencode:message:<id>` key would escape the GLOB
/// and freeze a Record's Model name the first time `translate_antigravity`
/// learned a new alias — the same permanently-Unpriced failure the bare key
/// above guards against.
fn message_dedup_key(session_id: &str, message_id: &str) -> String {
    format!("{SOURCE}:session:{session_id}:message:{message_id}")
}

/// A legacy Message's stable id: the `id` the JSON carries — the
/// Source-native identity ADR-0014 asks for, and what the real Artifact
/// holds.
///
/// The fallback is the file's path relative to the Session's message
/// directory, which is path-derived and so weaker than ADR-0014 wants: a
/// Message file copied under a new name manufactures a Record. It is the
/// only identity a Message with no `id` has, and OpenCode names the file
/// after the id it omits.
///
/// Relative path, not the bare file stem: `collect_json_files` recurses, so
/// two `a.json` in different subdirectories would collide on one dedup key —
/// and the token columns are `Immutable` on conflict, so the second Request's
/// tokens would be silently dropped rather than summed.
///
/// Joined with `/` rather than `to_string_lossy`, which would spell the same
/// Message `nested\c` on Windows and `nested/c` everywhere else: a dedup key
/// is a stored identity, so it must not read differently per platform.
fn legacy_message_id(value: &Value, path: &Path, message_root: &Path) -> String {
    value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            path.strip_prefix(message_root)
                .unwrap_or(path)
                .with_extension("")
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        })
}

/// The identity one Request is booked under.
struct MessageBooking {
    dedup_key: String,
    session_id: String,
    timestamp: i64,
    project: Option<String>,
    source_file: PathBuf,
}

/// A Request's own creation time, falling back to the Session's timestamp only
/// when the Message genuinely has none. OpenCode stamps every usage-bearing
/// assistant Message with `time.created` — present, and equal to the database's
/// `message.time_created`, on 253 of 253 Requests in the validated Artifact
/// (TOKL-24). The pre-TOKL-24 parser knew that column and discarded it.
fn message_timestamp(message_time: Option<i64>, session_timestamp: i64) -> i64 {
    message_time
        .filter(|time| *time > 0)
        .map(normalize_epoch)
        .unwrap_or(session_timestamp)
}

impl UsageSnapshot {
    /// `api_calls: 1` is literal here: one usage-bearing assistant Message is
    /// one Request. No zero guard — `ParsedMessage::Zero` already claims every
    /// snapshot whose tokens sum to zero, so a `Usage` snapshot is non-zero.
    fn event(self, booking: MessageBooking) -> UsageEvent {
        UsageEvent {
            dedup_key: booking.dedup_key,
            source: SOURCE.to_string(),
            timestamp: booking.timestamp,
            model: self.model,
            project: booking.project,
            api_calls: 1,
            input_tokens: self.input,
            output_tokens: self.output,
            cache_read_tokens: self.cache_read,
            cache_write_5m_tokens: self.cache_write,
            cache_write_1h_tokens: 0,
            source_file: booking.source_file.to_string_lossy().into_owned(),
            session_id: Some(booking.session_id),
            reasoning_tokens: self.reasoning.map(|reasoning| reasoning.min(self.output)),
            ctx: Default::default(),
        }
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

/// A legacy Message's own creation time, in RAW milliseconds — the same shape
/// the database's `message.time_created` column has, which is what
/// `message_timestamp` normalizes. Named for the unit because the sibling
/// `json_session_timestamp` returns normalized seconds instead.
///
/// The legacy JSON carries the same `time.created` / `time.completed` pair the
/// database mirrors; only `created` is used, matching how every other
/// per-Request Source is stamped.
fn json_message_created_ms(value: &Value) -> Option<i64> {
    value
        .get("time")
        .and_then(Value::as_object)
        .and_then(|time| time.get("created"))
        .and_then(nonnegative_i64)
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
    fn alias_rewrite_supersedes_every_per_request_record() {
        // The reason per-Request dedup keys stay Session-scoped
        // (`message_dedup_key`). A Model rename — an Antigravity alias
        // translation learning a new mapping — must DELETE and re-insert
        // each Record, because `INSERT_SQL`'s ON CONFLICT skips the
        // `model` column. A key outside the Session GLOB would take the
        // conflict path instead and stay Unpriced under its old name for
        // ever.
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
        assert_eq!(first.events_inserted, 2, "one Record per Request");
        assert_eq!(
            alias_switch_rows(&ledger),
            vec![
                (
                    "opencode:session:alias-switch:message:m1".to_string(),
                    Some("future-unknown-alias".to_string())
                ),
                (
                    "opencode:session:alias-switch:message:m2".to_string(),
                    Some("future-unknown-alias".to_string())
                ),
            ]
        );

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
        // Each Record keeps its key and moves to the new Model name.
        // Nothing about the pre-fix bug — a stale Unpriced Record surviving
        // because the GLOB missed it — may remain.
        assert_eq!(
            alias_switch_rows(&ledger),
            vec![
                (
                    "opencode:session:alias-switch:message:m1".to_string(),
                    Some("proper-model-name".to_string())
                ),
                (
                    "opencode:session:alias-switch:message:m2".to_string(),
                    Some("proper-model-name".to_string())
                ),
            ],
            "the model column must be rewritten by the supersession \
             re-scan — a stale Record would leave it Unpriced forever"
        );
    }

    fn alias_switch_rows(ledger: &Connection) -> Vec<(String, Option<String>)> {
        ledger
            .prepare(
                "SELECT dedup_key, model FROM events
                 WHERE session_id = 'alias-switch' ORDER BY dedup_key",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
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
        assert_eq!(first.events_inserted, 5, "one Record per usage-bearing Request");
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
                 FROM events WHERE source = 'opencode' ORDER BY session_id, model, dedup_key",
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
        assert_eq!(rows.len(), 5);

        let overlap: Vec<&EventRow> = rows.iter().filter(|row| row.0 == "modern-overlap").collect();
        assert_eq!(overlap.len(), 2, "two Requests, two Records");
        for row in &overlap {
            // The Session's own timestamp is 1_780_000_100; the Messages'
            // is 1_780_000_000. Booking at the Session's would be the
            // pre-TOKL-24 bug.
            assert_eq!(row.1, 1_780_000_000, "each Record lands at its Message's time");
            assert_eq!(row.2.as_deref(), Some("opencode-model"));
            assert!(row.7.ends_with("opencode.db"));
        }
        assert_eq!(
            overlap
                .iter()
                .map(|row| (row.3, row.4, row.5, row.6))
                .collect::<Vec<_>>(),
            vec![(10, 4, 3, 1), (20, 6, 7, 0)],
            "tokens are split across the Requests, never re-totalled"
        );
        assert_eq!(
            ledger
                .prepare(
                    "SELECT reasoning_tokens FROM events
                     WHERE session_id = 'modern-overlap' ORDER BY dedup_key",
                )
                .unwrap()
                .query_map([], |row| row.get::<_, Option<i64>>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap(),
            vec![Some(2), Some(1)],
            "reasoning is per-Request, not a Session sum"
        );

        let unknown: Vec<&EventRow> = rows
            .iter()
            .filter(|row| row.0 == "modern-unknown")
            .collect();
        assert_eq!(unknown.len(), 2, "two Requests, two Records");
        let one = unknown.iter().find(|row| row.2.as_deref() == Some("one")).unwrap();
        let two = unknown.iter().find(|row| row.2.as_deref() == Some("two")).unwrap();
        assert_eq!((one.3, one.4), (1, 0));
        assert_eq!((two.3, two.4), (0, 2));

        let legacy_only = rows.iter().find(|row| row.0 == "legacy-only").unwrap();
        assert_eq!(
            legacy_only.1, 1_780_000_400,
            "a Message with no time of its own falls back to the Session's"
        );
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
            5
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
    fn a_session_books_and_supersedes_one_record_per_surviving_request() {
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
            (
                "opencode:session:mixed:message:m1".to_string(),
                Some("model-a".to_string())
            )
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
        assert_eq!(rows.len(), 2, "a new Request adds a Record, never rewrites one");
        assert_eq!(
            rows[0],
            (
                "opencode:session:mixed:message:m1".to_string(),
                Some("model-a".to_string()),
                1_780_000_500,
                10,
                4
            )
        );
        assert_eq!(
            rows[1],
            (
                "opencode:session:mixed:message:m2".to_string(),
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
        // The Ledger outlives the Source's logs (CONTEXT.md, Ledger): a scan
        // deletes a Record only to supersede it with the finer Records that
        // stand in its place, and a pruned Request has none. m2's Record must
        // survive the Session it is no longer in.
        //
        // Mutation check: widen `supersedes_session_aggregates` back to the
        // blanket `…:session:<sid>:*` and this assertion drops to 1.
        let surviving: Vec<(String, Option<String>, i64, i64)> = ledger
            .prepare(
                "SELECT dedup_key, model, input_tokens, output_tokens
                 FROM events WHERE source = 'opencode' ORDER BY dedup_key",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            surviving.len(),
            2,
            "a pruned Request keeps the Record it already proved: {surviving:?}"
        );
        assert_eq!(
            surviving[0],
            (
                "opencode:session:mixed:message:m1".to_string(),
                Some("model-a".to_string()),
                10,
                4
            )
        );
        assert_eq!(
            surviving[1],
            (
                "opencode:session:mixed:message:m2".to_string(),
                Some("model-b".to_string()),
                2,
                8
            ),
            "the pruned Request's Record is unchanged, not recounted"
        );
    }

    /// The reason the per-Request key stays Session-scoped: a Request still
    /// present must be DELETEd and re-INSERTed so a Model rename lands, rather
    /// than hitting `INSERT_SQL`'s ON CONFLICT, which skips the Immutable
    /// `model` column. Narrowing supersession to protect pruned Requests must
    /// not cost this.
    ///
    /// Mutation check: drop `booked` from the patterns
    /// `supersedes_session_aggregates` returns and the Model stays `"one"`.
    #[test]
    fn a_present_requests_model_still_refreshes_on_a_rescan() {
        let tmp = tempdir().unwrap();
        let data_root = tmp.path().join("opencode");
        fs::create_dir_all(&data_root).unwrap();
        let current = data_root.join("opencode.db");
        create_database(&current);
        let db = Connection::open(&current).unwrap();
        insert_session(&db, "renamed", "/private/renamed", 1_780_000_900_000);
        insert_message(
            &db,
            "m1",
            "renamed",
            r#"{"role":"assistant","modelID":"one","tokens":{"input":10,"output":4,"cache":{"read":0,"write":0}}}"#,
        );
        drop(db);

        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        scan_opencode(&mut ledger, &data_root, &data_root.join("storage"), None);

        let db = Connection::open(&current).unwrap();
        db.execute(
            "UPDATE message SET data = ?1 WHERE id = 'm1'",
            params![
                r#"{"role":"assistant","modelID":"two","tokens":{"input":10,"output":4,"cache":{"read":0,"write":0}}}"#
            ],
        )
        .unwrap();
        drop(db);

        scan_opencode(&mut ledger, &data_root, &data_root.join("storage"), None);
        assert_eq!(
            ledger
                .query_row(
                    "SELECT model FROM events WHERE dedup_key = 'opencode:session:renamed:message:m1'",
                    [],
                    |row| row.get::<_, Option<String>>(0)
                )
                .unwrap(),
            Some("two".to_string()),
            "a present Request's Model must follow the Artifact"
        );
    }

    #[test]
    fn a_request_with_no_model_books_its_own_unattributed_record() {
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
                "opencode:session:split:message:m1".to_string(),
                Some("model-a".to_string()),
                10,
                1
            )
        );
        assert_eq!(
            rows[1],
            ("opencode:session:split:message:m2".to_string(), None, 3, 7),
            "an unattributed Request stays its own Record, booking NULL \
             rather than joining a group or borrowing a sibling's Model"
        );
    }

    #[test]
    fn legacy_requests_book_at_their_own_message_time() {
        // The legacy JSON carries the same `time.created` the database
        // mirrors. Before TOKL-24 `scan_legacy_session` passed `None` and
        // every Record collapsed onto the Session timestamp; `c.json` is
        // the only Message here that legitimately has none.
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
            r#"{"id":"msg-a","role":"assistant","modelID":"model-a","time":{"created":1780000100000,"completed":1780000150000},"tokens":{"input":5,"output":1,"cache":{"read":0,"write":0}}}"#,
        );
        write_json(
            &legacy.join("message/legacy-mixed/b.json"),
            r#"{"id":"msg-b","role":"assistant","modelID":"model-b","time":{"created":1780000200000,"completed":1780000250000},"tokens":{"input":1,"output":7,"cache":{"read":2,"write":0}}}"#,
        );
        write_json(
            &legacy.join("message/legacy-mixed/c.json"),
            r#"{"role":"assistant","modelID":"model-b","tokens":{"input":4,"output":2,"cache":{"read":0,"write":0}}}"#,
        );
        // Same file stem, one directory down: `collect_json_files` recurses,
        // so a bare-stem fallback would collide with `c.json` on one dedup
        // key and drop these tokens instead of booking them.
        write_json(
            &legacy.join("message/legacy-mixed/nested/c.json"),
            r#"{"role":"assistant","modelID":"model-b","tokens":{"input":9,"output":3,"cache":{"read":0,"write":0}}}"#,
        );

        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        let result = scan_opencode(&mut ledger, &data_root, &legacy, None);
        assert_eq!(result.events_inserted, 4);
        assert!(
            result.error.is_none(),
            "unexpected scan error: {:?}",
            result.error
        );
        let rows: Vec<(String, Option<String>, i64, i64, i64)> = ledger
            .prepare(
                "SELECT dedup_key, model, timestamp, input_tokens, output_tokens
                 FROM events WHERE source = 'opencode' ORDER BY dedup_key",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![
                (
                    // No `id` in the JSON — the path below the Session's
                    // message directory names the Record.
                    "opencode:session:legacy-mixed:message:c".to_string(),
                    Some("model-b".to_string()),
                    1_780_000_600,
                    4,
                    2
                ),
                (
                    "opencode:session:legacy-mixed:message:msg-a".to_string(),
                    Some("model-a".to_string()),
                    1_780_000_100,
                    5,
                    1
                ),
                (
                    "opencode:session:legacy-mixed:message:msg-b".to_string(),
                    Some("model-b".to_string()),
                    1_780_000_200,
                    1,
                    7
                ),
                (
                    "opencode:session:legacy-mixed:message:nested/c".to_string(),
                    Some("model-b".to_string()),
                    1_780_000_600,
                    9,
                    3
                ),
            ],
            "each legacy Request lands at its own `time.created`, and only \
             a Message with none falls back to the Session's 1_780_000_600; \
             two Messages sharing a file stem stay two Records"
        );
    }

    #[test]
    fn each_request_books_at_its_own_time_with_session_tokens_unchanged() {
        // TOKL-24's load-bearing pair: a Session whose Requests are minutes
        // apart books one Record each at its own timestamp, and the tokens
        // the Session totals to are exactly what the aggregate booked before.
        let tmp = tempdir().unwrap();
        let data_root = tmp.path().join("opencode");
        fs::create_dir_all(&data_root).unwrap();
        let current = data_root.join("opencode.db");
        create_database(&current);
        let db = Connection::open(&current).unwrap();
        // Session timestamp is 1_780_003_600_000 — an hour past the first
        // Request, and equal to none of them.
        insert_session(&db, "spread", "/private/spread", 1_780_003_600_000);
        for (id, minute, input) in [("m1", 0, 100), ("m2", 7, 200), ("m3", 41, 300)] {
            insert_message_at(
                &db,
                id,
                "spread",
                1_780_000_000_000 + minute * 60_000,
                &format!(
                    r#"{{"role":"assistant","modelID":"one-model","tokens":{{"input":{input},"output":1,"cache":{{"read":0,"write":0}}}}}}"#
                ),
            );
        }
        drop(db);

        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        let result = scan_opencode(&mut ledger, &data_root, &data_root.join("storage"), None);
        assert!(result.error.is_none(), "scan error: {:?}", result.error);
        assert_eq!(result.events_inserted, 3, "three Requests, three Records");

        let rows: Vec<(i64, i64, i64)> = ledger
            .prepare(
                "SELECT timestamp, api_calls, input_tokens FROM events
                 WHERE source = 'opencode' ORDER BY dedup_key",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![
                (1_780_000_000, 1, 100),
                (1_780_000_420, 1, 200),
                (1_780_002_460, 1, 300),
            ],
            "every Record lands at its own Message time, never the \
             Session's 1_780_003_600, and books exactly one Request"
        );
        assert_eq!(
            ledger
                .query_row(
                    "SELECT SUM(input_tokens + output_tokens + cache_read_tokens
                                + cache_write_5m_tokens)
                     FROM events WHERE source = 'opencode'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            603,
            "splitting a Session into Requests must not move a single token"
        );
    }

    #[test]
    fn a_rescan_leaves_no_pre_request_aggregate_row_behind() {
        // The pre-TOKL-24 parser booked both shapes: a bare
        // `opencode:session:<sid>` for a single-Model Session and
        // `…:model:<model>` splits for a Session that used several. Seed
        // both, then Scan at the new shape. Asserted on the ROW COUNT: an
        // unsuperseded aggregate hides perfectly inside a doubled total.
        let tmp = tempdir().unwrap();
        let data_root = tmp.path().join("opencode");
        fs::create_dir_all(&data_root).unwrap();
        let current = data_root.join("opencode.db");
        create_database(&current);
        let db = Connection::open(&current).unwrap();
        insert_session(&db, "legacy-shape", "/private/legacy-shape", 1_780_000_900_000);
        insert_message_at(
            &db,
            "m1",
            "legacy-shape",
            1_780_000_500_000,
            r#"{"role":"assistant","modelID":"model-a","tokens":{"input":10,"output":4,"cache":{"read":0,"write":0}}}"#,
        );
        insert_message_at(
            &db,
            "m2",
            "legacy-shape",
            1_780_000_700_000,
            r#"{"role":"assistant","modelID":"model-b","tokens":{"input":2,"output":8,"cache":{"read":0,"write":0}}}"#,
        );
        drop(db);

        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        let stale: Vec<UsageEvent> = [
            ("opencode:session:legacy-shape", "model-a"),
            ("opencode:session:legacy-shape:model:model-a", "model-a"),
            ("opencode:session:legacy-shape:model:model-b", "model-b"),
            ("opencode:session:legacy-shape:unattributed", "model-a"),
        ]
        .into_iter()
        .map(|(dedup_key, model)| UsageEvent {
            dedup_key: dedup_key.to_string(),
            source: SOURCE.to_string(),
            timestamp: 1_780_000_900,
            model: Some(model.to_string()),
            project: Some("/private/legacy-shape".to_string()),
            api_calls: 1,
            input_tokens: 12,
            output_tokens: 12,
            cache_read_tokens: 0,
            cache_write_5m_tokens: 0,
            cache_write_1h_tokens: 0,
            source_file: current.to_string_lossy().into_owned(),
            session_id: Some("legacy-shape".to_string()),
            reasoning_tokens: None,
            ctx: Default::default(),
        })
        .collect();
        insert_events_superseding(&mut ledger, &[], &stale).unwrap();
        assert_eq!(opencode_keys(&ledger).len(), 4, "the old shape is seeded");

        let result = scan_opencode(&mut ledger, &data_root, &data_root.join("storage"), None);
        assert!(result.error.is_none(), "scan error: {:?}", result.error);
        assert_eq!(
            opencode_keys(&ledger),
            vec![
                "opencode:session:legacy-shape:message:m1".to_string(),
                "opencode:session:legacy-shape:message:m2".to_string(),
            ],
            "every aggregate row — the bare key included — is superseded"
        );
    }

    fn opencode_keys(ledger: &Connection) -> Vec<String> {
        ledger
            .prepare("SELECT dedup_key FROM events WHERE source = 'opencode' ORDER BY dedup_key")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }
}
