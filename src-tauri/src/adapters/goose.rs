use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rusqlite::{types::ValueRef, Connection, OpenFlags, Row};
use serde_json::Value;

use crate::adapters::{
    absolute_project, file_state_of, normalize_epoch, remember_file_states, sqlite_file_states,
    unchanged,
};
use crate::db::{insert_events, source_session_ids, upsert_source_sessions};
use crate::time::iso_to_epoch;
use crate::types::{SourceScanResult, SourceSessionMeta, UsageEvent};

const SOURCE: &str = "goose";
const SESSIONS_DB: &str = "sessions.db";
const SUPPORTED_SCHEMA_VERSION: i64 = 15;
const PARSER_VERSION: i64 = 1;

#[derive(Default)]
struct DatabaseScan {
    events: Vec<UsageEvent>,
    session_ids: HashSet<String>,
    source_sessions: Vec<SourceSessionMeta>,
    lines_skipped: u64,
}

struct LegacyScan {
    event: Option<UsageEvent>,
    skipped: bool,
}

#[derive(Default, Clone, Copy)]
struct TokenSnapshot {
    input: Option<i64>,
    output: Option<i64>,
    cache_read: Option<i64>,
    cache_write: Option<i64>,
}

/// Scan Goose's local session database and pre-1.10 JSONL session records.
///
/// Goose writes inclusive input tokens: its cache buckets are subsets of the
/// input field. TokenLedger stores exclusive categories, so the adapter is the
/// one place where those cache subsets are removed from Input. The source's
/// logged Cost is deliberately ignored and repriced by the Ledger.
pub fn scan_goose(conn: &mut Connection, session_roots: &[PathBuf]) -> SourceScanResult {
    let (database_paths, legacy_paths, mut errors) = discover_artifacts(session_roots);
    let changed_legacy = legacy_paths
        .into_iter()
        .filter_map(|path| {
            let mut state = file_state_of(&path);
            state.byte_offset = PARSER_VERSION;
            (!unchanged(conn, &path, &state)).then_some((path, state))
        })
        .collect::<Vec<_>>();
    let mut modern_session_ids = HashSet::new();
    let mut events = Vec::new();
    let mut lines_skipped = 0;
    let mut event_keys = HashSet::new();
    let mut parsed_states = Vec::new();
    let mut parsed_legacy_states = Vec::new();
    let mut parsed_source_sessions = Vec::new();
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
                modern_session_ids.extend(scan.session_ids);
                parsed_source_sessions.extend(scan.source_sessions);
                lines_skipped += scan.lines_skipped;
                for event in scan.events {
                    if event_keys.insert(event.dedup_key.clone()) {
                        events.push(event);
                    }
                }
            }
            Err(error) => errors.push(error),
        }
    }

    if skipped_database && !changed_legacy.is_empty() {
        match source_session_ids(conn, SOURCE) {
            Ok(session_ids) => modern_session_ids.extend(session_ids),
            Err(error) => {
                scan_legacy = false;
                errors.push(format!("{SOURCE}: Session metadata read failed: {error}"));
            }
        }
    }

    if scan_legacy {
        for (path, state) in changed_legacy {
            match scan_legacy_file(&path, &modern_session_ids) {
                Ok(scan) => {
                    parsed_legacy_states.push((path, state));
                    lines_skipped += u64::from(scan.skipped);
                    if let Some(event) = scan.event {
                        if event_keys.insert(event.dedup_key.clone()) {
                            events.push(event);
                        }
                    }
                }
                Err(error) => {
                    lines_skipped += 1;
                    errors.push(error);
                }
            }
        }
    }

    let events_inserted = match insert_events(conn, &events) {
        Ok(inserted) => {
            match upsert_source_sessions(conn, SOURCE, &parsed_source_sessions) {
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
            if let Err(error) = remember_file_states(conn, &parsed_legacy_states) {
                errors.push(format!(
                    "{SOURCE}: Ledger file-state update failed: {error}"
                ));
            }
            inserted
        }
        Err(error) => {
            errors.push(format!("goose: Ledger insert failed: {error}"));
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

fn discover_artifacts(roots: &[PathBuf]) -> (Vec<PathBuf>, Vec<PathBuf>, Vec<String>) {
    let mut databases = Vec::new();
    let mut legacy = Vec::new();
    let mut errors = Vec::new();

    for root in roots {
        if root.is_file() {
            match root.file_name().and_then(|name| name.to_str()) {
                Some(SESSIONS_DB) => add_unique(&mut databases, root),
                _ if root.extension().and_then(|ext| ext.to_str()) == Some("jsonl") => {
                    add_unique(&mut legacy, root)
                }
                _ => errors.push("goose: unsupported Source Artifact file".to_string()),
            }
            continue;
        }
        if !root.is_dir() {
            continue;
        }

        let database = root.join(SESSIONS_DB);
        if database.is_file() {
            add_unique(&mut databases, &database);
        }

        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(error) => {
                errors.push(format!("goose: session directory read failed: {error}"));
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                add_unique(&mut legacy, &path);
            }
        }
    }

    databases.sort();
    legacy.sort();
    (databases, legacy, errors)
}

fn add_unique(paths: &mut Vec<PathBuf>, path: &Path) {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn scan_database(path: &Path) -> Result<DatabaseScan, String> {
    // A plain path (not a `file:` URI) keeps Windows verbatim temp paths, which
    // can carry a `\\?\` prefix, working in both production and tests.
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|error| format!("goose: sessions database open failed: {error}"))?;
    let _ = conn.busy_timeout(std::time::Duration::from_millis(5000));

    if let Some(version) = schema_version(&conn)? {
        if !(1..=SUPPORTED_SCHEMA_VERSION).contains(&version) {
            return Err(format!(
                "goose: unsupported schema version {version} (supported 1-{SUPPORTED_SCHEMA_VERSION})"
            ));
        }
    }
    if !table_exists(&conn, "sessions")
        .map_err(|error| format!("goose: schema inspection failed: {error}"))?
    {
        return Err("goose: unsupported sessions database schema".to_string());
    }

    let has_usage_ledger = table_exists(&conn, "usage_ledger")
        .map_err(|error| format!("goose: schema inspection failed: {error}"))?;
    let mut scan = DatabaseScan::default();

    let mut session_statement = conn
        .prepare(
            "SELECT id, working_dir, created_at, updated_at
             FROM sessions ORDER BY id",
        )
        .map_err(|error| format!("goose: Session metadata query failed: {error}"))?;
    let session_rows = session_statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                optional_text(row, 1),
                optional_timestamp(row, 2),
                optional_timestamp(row, 3),
            ))
        })
        .map_err(|error| format!("goose: Session metadata read failed: {error}"))?;
    for row in session_rows {
        let (session_id, project, created_at, updated_at) =
            row.map_err(|error| format!("goose: Session metadata row failed: {error}"))?;
        let observed_at = updated_at.or(created_at).unwrap_or(0);
        scan.source_sessions.push(SourceSessionMeta {
            session_id,
            cwd: absolute_project(project.as_deref()),
            model: None,
            title: None,
            created_at: created_at.unwrap_or(observed_at),
            updated_at: updated_at.unwrap_or(observed_at),
        });
    }

    if has_usage_ledger {
        let mut statement = conn
            .prepare(
                "SELECT u.id, u.session_id, u.created_timestamp, u.model,
                        u.input_tokens, u.output_tokens, u.cache_read_tokens,
                        u.cache_write_tokens, s.working_dir
                 FROM usage_ledger AS u
                 LEFT JOIN sessions AS s ON s.id = u.session_id
                 ORDER BY u.id",
            )
            .map_err(|error| format!("goose: usage ledger query failed: {error}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok(ModernRow {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    timestamp: value_i64(row, 2),
                    model: optional_text(row, 3),
                    input: optional_i64(row, 4),
                    output: optional_i64(row, 5),
                    cache_read: optional_i64(row, 6),
                    cache_write: optional_i64(row, 7),
                    project: optional_text(row, 8),
                })
            })
            .map_err(|error| format!("goose: usage ledger read failed: {error}"))?;

        for row in rows {
            let row = row.map_err(|error| format!("goose: usage ledger row failed: {error}"))?;
            scan.session_ids.insert(row.session_id.clone());
            let Some(timestamp) = row.timestamp else {
                scan.lines_skipped += 1;
                continue;
            };
            let Some((input, output, cache_read, cache_write)) =
                normalize_tokens(row.input, row.output, row.cache_read, row.cache_write)
            else {
                scan.lines_skipped += 1;
                continue;
            };
            if input + output + cache_read + cache_write == 0 {
                scan.lines_skipped += 1;
                continue;
            }
            scan.events.push(UsageEvent {
                dedup_key: format!("{SOURCE}:usage:{}:{}", row.session_id, row.id),
                source: SOURCE.to_string(),
                timestamp: normalize_epoch(timestamp),
                model: clean_model(row.model),
                project: absolute_project(row.project.as_deref()),
                api_calls: 1,
                input_tokens: input,
                output_tokens: output,
                cache_read_tokens: cache_read,
                cache_write_5m_tokens: cache_write,
                cache_write_1h_tokens: 0,
                source_file: path.to_string_lossy().into_owned(),
                session_id: Some(row.session_id),
                reasoning_tokens: None,
                ctx: Default::default(),
            });
        }
    }

    // Legacy-imported sessions may have aggregate counters but no usage_ledger
    // rows. They are one session-level Usage Record, not a guessed per-call
    // split. A modern row (including an all-zero observation) suppresses the
    // fallback so the modern representation wins when both artifacts overlap.
    let fallback_sql = if has_usage_ledger {
        "SELECT s.id, s.updated_at, s.created_at, s.working_dir,
                COALESCE(NULLIF(s.accumulated_input_tokens, 0), s.input_tokens, 0),
                COALESCE(NULLIF(s.accumulated_output_tokens, 0), s.output_tokens, 0),
                COALESCE(NULLIF(s.accumulated_cache_read_tokens, 0), s.cache_read_tokens, 0),
                COALESCE(NULLIF(s.accumulated_cache_write_tokens, 0), s.cache_write_tokens, 0)
         FROM sessions AS s
         WHERE NOT EXISTS (
             SELECT 1 FROM usage_ledger AS u WHERE u.session_id = s.id
         )
         ORDER BY s.id"
    } else {
        "SELECT s.id, s.updated_at, s.created_at, s.working_dir,
                COALESCE(NULLIF(s.accumulated_input_tokens, 0), s.input_tokens, 0),
                COALESCE(NULLIF(s.accumulated_output_tokens, 0), s.output_tokens, 0),
                COALESCE(NULLIF(s.accumulated_cache_read_tokens, 0), s.cache_read_tokens, 0),
                COALESCE(NULLIF(s.accumulated_cache_write_tokens, 0), s.cache_write_tokens, 0)
         FROM sessions AS s
         ORDER BY s.id"
    };
    let mut statement = conn
        .prepare(fallback_sql)
        .map_err(|error| format!("goose: session aggregate query failed: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok(SessionRow {
                id: row.get(0)?,
                updated_at: optional_timestamp(row, 1),
                created_at: optional_timestamp(row, 2),
                project: optional_text(row, 3),
                input: optional_i64(row, 4),
                output: optional_i64(row, 5),
                cache_read: optional_i64(row, 6),
                cache_write: optional_i64(row, 7),
            })
        })
        .map_err(|error| format!("goose: session aggregate read failed: {error}"))?;
    for row in rows {
        let row = row.map_err(|error| format!("goose: session aggregate row failed: {error}"))?;
        scan.session_ids.insert(row.id.clone());
        let Some(timestamp) = row.updated_at.or(row.created_at) else {
            scan.lines_skipped += 1;
            continue;
        };
        let Some((input, output, cache_read, cache_write)) =
            normalize_tokens(row.input, row.output, row.cache_read, row.cache_write)
        else {
            scan.lines_skipped += 1;
            continue;
        };
        if input + output + cache_read + cache_write == 0 {
            scan.lines_skipped += 1;
            continue;
        }
        scan.events.push(UsageEvent {
            dedup_key: format!("{SOURCE}:session:{}", row.id),
            source: SOURCE.to_string(),
            timestamp: normalize_epoch(timestamp),
            model: None,
            project: absolute_project(row.project.as_deref()),
            api_calls: 1,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_write_5m_tokens: cache_write,
            cache_write_1h_tokens: 0,
            source_file: path.to_string_lossy().into_owned(),
            session_id: Some(row.id),
            reasoning_tokens: None,
            ctx: Default::default(),
        });
    }

    Ok(scan)
}

struct ModernRow {
    id: i64,
    session_id: String,
    timestamp: Option<i64>,
    model: Option<String>,
    input: Option<i64>,
    output: Option<i64>,
    cache_read: Option<i64>,
    cache_write: Option<i64>,
    project: Option<String>,
}

struct SessionRow {
    id: String,
    updated_at: Option<i64>,
    created_at: Option<i64>,
    project: Option<String>,
    input: Option<i64>,
    output: Option<i64>,
    cache_read: Option<i64>,
    cache_write: Option<i64>,
}

fn scan_legacy_file(
    path: &Path,
    modern_session_ids: &HashSet<String>,
) -> Result<LegacyScan, String> {
    let file = File::open(path)
        .map_err(|error| format!("goose: legacy Source Artifact open failed: {error}"))?;
    let mut reader = BufReader::new(file);
    let mut header = String::new();
    reader
        .read_line(&mut header)
        .map_err(|error| format!("goose: legacy Source Artifact read failed: {error}"))?;
    if header.trim().is_empty() {
        return Err("goose: malformed legacy Source Artifact header".to_string());
    }
    let value: Value = serde_json::from_str(header.trim_end())
        .map_err(|_| "goose: malformed legacy Source Artifact header".to_string())?;
    let session_id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            path.file_stem()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .ok_or_else(|| "goose: legacy Source Artifact has no session identity".to_string())?;
    if modern_session_ids.contains(&session_id) {
        return Ok(LegacyScan {
            event: None,
            skipped: false,
        });
    }

    let timestamp = json_timestamp(value.get("updated_at"))
        .or_else(|| json_timestamp(value.get("updatedAt")))
        .or_else(|| json_timestamp(value.get("created_at")))
        .or_else(|| json_timestamp(value.get("createdAt")))
        .or_else(|| filename_timestamp(path))
        .or_else(|| file_mtime_timestamp(path))
        .ok_or_else(|| "goose: legacy Source Artifact has no usable timestamp".to_string())?;
    let accumulated = token_snapshot(&value, "accumulated_usage", true);
    let current = token_snapshot(&value, "usage", false);
    let selected = if accumulated.has_nonzero() {
        accumulated
    } else {
        current
    };
    let Some((input, output, cache_read, cache_write)) = normalize_tokens(
        selected.input,
        selected.output,
        selected.cache_read,
        selected.cache_write,
    ) else {
        return Ok(LegacyScan {
            event: None,
            skipped: true,
        });
    };
    if input + output + cache_read + cache_write == 0 {
        return Ok(LegacyScan {
            event: None,
            skipped: true,
        });
    }

    Ok(LegacyScan {
        event: Some(UsageEvent {
            dedup_key: format!("{SOURCE}:session:{session_id}"),
            source: SOURCE.to_string(),
            timestamp: normalize_epoch(timestamp),
            model: None,
            project: absolute_project(
                value.get("working_dir").and_then(Value::as_str),
            ),
            api_calls: 1,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_write_5m_tokens: cache_write,
            cache_write_1h_tokens: 0,
            source_file: fs::canonicalize(path)
                .unwrap_or_else(|_| path.to_path_buf())
                .to_string_lossy()
                .into_owned(),
            session_id: Some(session_id),
            reasoning_tokens: None,
            ctx: Default::default(),
        }),
        skipped: false,
    })
}

fn table_exists(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [name],
        |row| row.get::<_, i64>(0),
    )
    .map(|value| value != 0)
}

fn schema_version(conn: &Connection) -> Result<Option<i64>, String> {
    if !table_exists(conn, "schema_version")
        .map_err(|error| format!("goose: schema inspection failed: {error}"))?
    {
        return Ok(None);
    }
    conn.query_row(
        "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
        [],
        |row| row.get(0),
    )
    .map(Some)
    .map_err(|error| format!("goose: schema version read failed: {error}"))
}

fn value_i64(row: &Row<'_>, index: usize) -> Option<i64> {
    match row.get_ref(index).ok()? {
        ValueRef::Integer(value) => Some(normalize_epoch(value)),
        ValueRef::Real(value) => Some(value as i64),
        ValueRef::Text(value) => parse_number(std::str::from_utf8(value).ok()?),
        ValueRef::Null | ValueRef::Blob(_) => None,
    }
}

fn optional_i64(row: &Row<'_>, index: usize) -> Option<i64> {
    match row.get_ref(index).ok()? {
        ValueRef::Integer(value) => Some(value),
        ValueRef::Real(value) => Some(value as i64),
        ValueRef::Text(value) => parse_number(std::str::from_utf8(value).ok()?),
        ValueRef::Null | ValueRef::Blob(_) => None,
    }
}

fn optional_text(row: &Row<'_>, index: usize) -> Option<String> {
    match row.get_ref(index).ok()? {
        ValueRef::Text(value) => Some(String::from_utf8_lossy(value).into_owned()),
        ValueRef::Integer(value) => Some(value.to_string()),
        ValueRef::Real(value) => Some(value.to_string()),
        ValueRef::Null | ValueRef::Blob(_) => None,
    }
}

fn optional_timestamp(row: &Row<'_>, index: usize) -> Option<i64> {
    match row.get_ref(index).ok()? {
        ValueRef::Integer(value) => Some(normalize_epoch(value)),
        ValueRef::Real(value) => Some(normalize_epoch(value as i64)),
        ValueRef::Text(value) => json_timestamp(Some(&Value::String(
            String::from_utf8_lossy(value).into_owned(),
        ))),
        ValueRef::Null | ValueRef::Blob(_) => None,
    }
}

fn parse_number(value: &str) -> Option<i64> {
    value
        .trim()
        .parse::<i64>()
        .ok()
        .or_else(|| value.trim().parse::<f64>().ok().map(|value| value as i64))
}

fn json_timestamp(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if let Some(number) = value.as_i64() {
        return Some(normalize_epoch(number));
    }
    if let Some(number) = value.as_f64() {
        return Some(normalize_epoch(number as i64));
    }
    let text = value.as_str()?.trim();
    parse_number(text)
        .map(normalize_epoch)
        .or_else(|| iso_to_epoch(text))
}

fn filename_timestamp(path: &Path) -> Option<i64> {
    let stem = path.file_stem()?.to_str()?;
    let (date, time) = stem.split_once('_')?;
    if date.len() != 8 || time.len() < 6 {
        return None;
    }
    let iso = format!(
        "{}-{}-{}T{}:{}:{}Z",
        &date[0..4],
        &date[4..6],
        &date[6..8],
        &time[0..2],
        &time[2..4],
        &time[4..6]
    );
    iso_to_epoch(&iso)
}

fn file_mtime_timestamp(path: &Path) -> Option<i64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
}

fn clean_model(model: Option<String>) -> Option<String> {
    model.and_then(|model| (!model.trim().is_empty()).then_some(model))
}

fn normalize_tokens(
    input: Option<i64>,
    output: Option<i64>,
    cache_read: Option<i64>,
    cache_write: Option<i64>,
) -> Option<(i64, i64, i64, i64)> {
    let input = input.unwrap_or(0);
    let output = output.unwrap_or(0);
    let cache_read = cache_read.unwrap_or(0);
    let cache_write = cache_write.unwrap_or(0);
    if input < 0 || output < 0 || cache_read < 0 || cache_write < 0 {
        return None;
    }
    Some((
        input.saturating_sub(cache_read).saturating_sub(cache_write),
        output,
        cache_read,
        cache_write,
    ))
}

fn token_snapshot(value: &Value, nested: &str, accumulated: bool) -> TokenSnapshot {
    let nested = value.get(nested);
    TokenSnapshot {
        input: token_value(
            nested,
            value,
            if accumulated {
                "accumulated_input_tokens"
            } else {
                "input_tokens"
            },
            "input_tokens",
        ),
        output: token_value(
            nested,
            value,
            if accumulated {
                "accumulated_output_tokens"
            } else {
                "output_tokens"
            },
            "output_tokens",
        ),
        cache_read: token_value(
            nested,
            value,
            if accumulated {
                "accumulated_cache_read_tokens"
            } else {
                "cache_read_tokens"
            },
            "cache_read_tokens",
        ),
        cache_write: token_value(
            nested,
            value,
            if accumulated {
                "accumulated_cache_write_tokens"
            } else {
                "cache_write_tokens"
            },
            "cache_write_tokens",
        ),
    }
}

fn token_value(
    nested: Option<&Value>,
    root: &Value,
    flat_key: &str,
    nested_key: &str,
) -> Option<i64> {
    nested
        .and_then(|value| value.get(nested_key))
        .and_then(Value::as_i64)
        .or_else(|| {
            nested.and_then(|value| {
                let upstream_key = match nested_key {
                    "cache_read_tokens" => Some("cache_read_input_tokens"),
                    "cache_write_tokens" => Some("cache_write_input_tokens"),
                    _ => None,
                }?;
                value.get(upstream_key).and_then(Value::as_i64)
            })
        })
        .or_else(|| root.get(flat_key).and_then(Value::as_i64))
        .or_else(|| root.get(nested_key).and_then(Value::as_i64))
        .or_else(|| {
            let upstream_key = match nested_key {
                "cache_read_tokens" => Some("cache_read_input_tokens"),
                "cache_write_tokens" => Some("cache_write_input_tokens"),
                _ => None,
            }?;
            root.get(upstream_key).and_then(Value::as_i64)
        })
}

fn token_snapshot_has_nonzero(value: Option<i64>) -> bool {
    value.is_some_and(|value| value != 0)
}

impl TokenSnapshot {
    fn has_nonzero(self) -> bool {
        token_snapshot_has_nonzero(self.input)
            || token_snapshot_has_nonzero(self.output)
            || token_snapshot_has_nonzero(self.cache_read)
            || token_snapshot_has_nonzero(self.cache_write)
    }
}

#[cfg(test)]
mod tests {
    use super::scan_goose;
    use crate::adapters::file_state_of;
    use crate::db::{get_file_state, open_db, set_file_state};
    use crate::pricing::{self, OverrideRates};
    use crate::queries::{self, Filters};
    use rusqlite::Connection;
    use std::fs;

    const PRIVATE_PROMPT: &str = "GOOSE_PRIVATE_PROMPT_MARKER";
    const PRIVATE_RESPONSE: &str = "GOOSE_PRIVATE_RESPONSE_MARKER";

    fn build_fixture(root: &std::path::Path) {
        fs::create_dir_all(root).unwrap();
        let db = Connection::open(root.join("sessions.db")).unwrap();
        db.execute_batch(
            "CREATE TABLE schema_version (version INTEGER NOT NULL);
             INSERT INTO schema_version VALUES (15);
             CREATE TABLE sessions (
                 id TEXT PRIMARY KEY,
                 working_dir TEXT,
                 created_at TEXT,
                 updated_at TEXT,
                 input_tokens INTEGER,
                 output_tokens INTEGER,
                 cache_read_tokens INTEGER,
                 cache_write_tokens INTEGER,
                 accumulated_input_tokens INTEGER,
                 accumulated_output_tokens INTEGER,
                 accumulated_cache_read_tokens INTEGER,
                 accumulated_cache_write_tokens INTEGER
             );
             CREATE TABLE usage_ledger (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 session_id TEXT NOT NULL,
                 created_timestamp INTEGER NOT NULL,
                 model TEXT,
                 input_tokens INTEGER,
                 output_tokens INTEGER,
                 total_tokens INTEGER,
                 cache_read_tokens INTEGER,
                 cache_write_tokens INTEGER,
                 cost REAL,
                 cost_source TEXT,
                 is_compaction INTEGER DEFAULT 0
             );",
        )
        .unwrap();
        db.execute(
            "INSERT INTO sessions
             (id, working_dir, created_at, updated_at,
              accumulated_input_tokens, accumulated_output_tokens,
              accumulated_cache_read_tokens, accumulated_cache_write_tokens)
             VALUES ('modern', '/Users/dev/goose-project', '2026-07-01T10:00:00Z',
                     '2026-07-01T10:01:00Z', 0, 0, 0, 0),
                    ('fallback', '/Users/dev/goose-fallback', '2026-07-01T10:02:00Z',
                     '2026-07-01T10:03:00Z', 50, 25, 10, 5),
                    ('legacy-wins', '/Users/dev/modern-wins', '2026-07-01T10:04:00Z',
                     '2026-07-01T10:05:00Z', 0, 0, 0, 0)",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO usage_ledger
             (session_id, created_timestamp, model, input_tokens, output_tokens,
              total_tokens, cache_read_tokens, cache_write_tokens, cost)
             VALUES ('modern', 1780308000, 'goose-model', 120, 30, 180, 20, 10, 999.0),
                    ('modern', 1780308001, NULL, 0, 5, 5, 0, 0, 1.0),
                    ('legacy-wins', 1780308002, NULL, 0, 0, 0, 0, 0, 0.0)",
            [],
        )
        .unwrap();
        drop(db);

        fs::write(
            root.join("legacy-wins.jsonl"),
            format!(
                "{{\"id\":\"legacy-wins\",\"updated_at\":\"2026-07-01T10:06:00Z\",\"working_dir\":\"/Users/dev/legacy\",\"accumulated_usage\":{{\"input_tokens\":999,\"output_tokens\":999,\"cache_read_tokens\":999,\"cache_write_tokens\":999}},\"note\":\"{PRIVATE_PROMPT}\"}}\n{{\"role\":\"assistant\",\"content\":\"{PRIVATE_RESPONSE}\"}}\n"
            ),
        )
        .unwrap();
        fs::write(
            root.join("legacy-only.jsonl"),
            r#"{"id":"legacy-only","created_at":"2026-07-01T10:07:00Z","working_dir":"relative/project","usage":{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":30,"cache_write_input_tokens":10}}
{"role":"assistant","content":"not read"}
"#,
        )
        .unwrap();
        fs::write(root.join("malformed.jsonl"), "{ not json\n").unwrap();
    }

    #[test]
    fn goose_scan_normalizes_modern_and_legacy_usage_without_persisting_content() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("goose/sessions");
        build_fixture(&root);
        let ledger_path = tmp.path().join("ledger.db");
        let mut ledger = open_db(&ledger_path).unwrap();
        pricing::set_override(
            &ledger,
            "goose-model",
            OverrideRates {
                input: Some(1.0),
                output: Some(2.0),
                cache_read: Some(3.0),
                cache_write: Some(4.0),
            },
        )
        .unwrap();

        let first = scan_goose(&mut ledger, &[root.clone(), root.clone()]);
        assert_eq!(first.events_inserted, 4);
        assert_eq!(
            first.lines_skipped, 2,
            "one zero-token row and one malformed legacy header"
        );
        assert!(first
            .error
            .as_deref()
            .is_some_and(|error| error.contains("malformed")));

        let count: i64 = ledger
            .query_row(
                "SELECT COUNT(*) FROM events WHERE source = 'goose'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 4);
        let modern: (i64, i64, i64, i64, i64) = ledger
            .query_row(
                "SELECT input_tokens, output_tokens, cache_read_tokens, cache_write_5m_tokens, api_calls
                 FROM events WHERE dedup_key = 'goose:usage:modern:1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap();
        assert_eq!(modern, (90, 30, 20, 10, 1));

        let legacy_wins: i64 = ledger
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id = 'legacy-wins'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            legacy_wins, 0,
            "modern zero usage suppresses legacy overlap"
        );

        let fallback: (i64, i64, i64, i64) = ledger
            .query_row(
                "SELECT input_tokens, output_tokens, cache_read_tokens, cache_write_5m_tokens
                 FROM events WHERE dedup_key = 'goose:session:fallback'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(fallback, (35, 25, 10, 5));

        let summary = queries::summary(&ledger, &Filters::default()).unwrap();
        assert_eq!(summary.total_tokens, 150 + 5 + 75 + 120);
        assert_eq!(summary.cost, Some(250.0));
        assert!(!summary.has_unpriced);
        assert_eq!(summary.unattributed_tokens, 5 + 75 + 120);
        let source_rows = queries::breakdown(&ledger, "tool", &Filters::default()).unwrap();
        assert_eq!(
            source_rows
                .iter()
                .find(|row| row.key.as_deref() == Some("goose"))
                .unwrap()
                .requests,
            4
        );

        fs::write(
            root.join("legacy-wins.jsonl"),
            r#"{"id":"legacy-wins","updated_at":"2026-07-01T10:06:00Z","accumulated_usage":{"input_tokens":1000,"output_tokens":1000}}
"#,
        )
        .unwrap();
        let database_path = fs::canonicalize(root.join("sessions.db")).unwrap();
        fs::write(&database_path, "not sqlite").unwrap();
        let mut database_state = file_state_of(&database_path);
        database_state.byte_offset = super::PARSER_VERSION;
        set_file_state(
            &ledger,
            &database_path.to_string_lossy(),
            database_state,
        )
        .unwrap();
        let second = scan_goose(&mut ledger, &[root.clone(), root.clone()]);
        assert!(
            !second
                .error
                .as_deref()
                .is_some_and(|error| error.contains("database")),
            "an unchanged modern database was reopened: {:?}",
            second.error
        );
        assert_eq!(second.events_inserted, 0);
        assert_eq!(
            ledger
                .query_row(
                    "SELECT COUNT(*) FROM events WHERE source = 'goose'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            4
        );

        ledger.execute_batch("DROP TABLE source_sessions;").unwrap();
        fs::write(
            root.join("legacy-wins.jsonl"),
            r#"{"id":"legacy-wins","updated_at":"2026-07-01T10:07:00Z","accumulated_usage":{"input_tokens":2000,"output_tokens":2000}}
"#,
        )
        .unwrap();
        let third = scan_goose(&mut ledger, &[root.clone(), root]);
        assert!(third
            .error
            .as_deref()
            .is_some_and(|error| error.contains("Session metadata read failed")));
        assert_eq!(third.events_inserted, 0);
        assert_eq!(
            ledger
                .query_row(
                    "SELECT COUNT(*) FROM events WHERE source = 'goose'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            4
        );

        drop(ledger);
        let mut durable = Vec::new();
        for suffix in ["", "-wal", "-shm"] {
            if let Ok(bytes) = fs::read(format!("{}{}", ledger_path.display(), suffix)) {
                durable.extend(bytes);
            }
        }
        assert!(!durable
            .windows(PRIVATE_PROMPT.len())
            .any(|window| window == PRIVATE_PROMPT.as_bytes()));
        assert!(!durable
            .windows(PRIVATE_RESPONSE.len())
            .any(|window| window == PRIVATE_RESPONSE.as_bytes()));
    }

    #[test]
    fn goose_scan_rejects_unsupported_schema_and_keeps_the_ledger_usable() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("goose/sessions");
        fs::create_dir_all(&root).unwrap();
        let db = Connection::open(root.join("sessions.db")).unwrap();
        db.execute_batch(
            "CREATE TABLE schema_version (version INTEGER NOT NULL);
             INSERT INTO schema_version VALUES (99);
             CREATE TABLE sessions (id TEXT PRIMARY KEY);",
        )
        .unwrap();
        drop(db);

        let mut ledger = open_db(&tmp.path().join("ledger.db")).unwrap();
        let result = scan_goose(&mut ledger, &[root]);
        assert_eq!(result.events_inserted, 0);
        assert!(result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("unsupported schema version")));
        assert_eq!(
            ledger
                .query_row("SELECT COUNT(*) FROM events", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn unchanged_database_is_not_reopened() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("goose/sessions");
        build_fixture(&root);
        for name in ["legacy-wins.jsonl", "legacy-only.jsonl", "malformed.jsonl"] {
            fs::remove_file(root.join(name)).unwrap();
        }
        let database_path = fs::canonicalize(root.join("sessions.db")).unwrap();
        let mut ledger = open_db(&tmp.path().join("ledger.db")).unwrap();

        scan_goose(&mut ledger, &[root.clone()]);
        assert!(get_file_state(&ledger, &database_path.to_string_lossy())
            .unwrap()
            .is_some());

        fs::write(&database_path, "not sqlite").unwrap();
        let mut state = file_state_of(&database_path);
        state.byte_offset = 1;
        set_file_state(&ledger, &database_path.to_string_lossy(), state).unwrap();

        let second = scan_goose(&mut ledger, &[root]);
        assert!(second.error.is_none());
        assert_eq!(second.events_inserted, 0);
    }

    #[test]
    fn unchanged_legacy_file_is_not_reopened() {
        let tmp = tempfile::tempdir().unwrap();
        let session_path = tmp.path().join("legacy.jsonl");
        fs::write(
            &session_path,
            r#"{"id":"legacy","created_at":"2026-07-01T10:07:00Z","usage":{"input_tokens":10,"output_tokens":2}}
"#,
        )
        .unwrap();
        let session_path = fs::canonicalize(session_path).unwrap();
        let mut ledger = open_db(&tmp.path().join("ledger.db")).unwrap();

        scan_goose(&mut ledger, std::slice::from_ref(&session_path));
        assert!(get_file_state(&ledger, &session_path.to_string_lossy())
            .unwrap()
            .is_some());

        fs::write(&session_path, "not json\n").unwrap();
        let mut state = file_state_of(&session_path);
        state.byte_offset = 1;
        set_file_state(&ledger, &session_path.to_string_lossy(), state).unwrap();

        let second = scan_goose(&mut ledger, std::slice::from_ref(&session_path));
        assert!(second.error.is_none());
        assert_eq!(second.events_inserted, 0);
    }

    #[test]
    fn file_state_failure_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("goose/sessions");
        build_fixture(&root);
        let mut ledger = open_db(&tmp.path().join("ledger.db")).unwrap();
        ledger.execute("DROP TABLE scanned_files", []).unwrap();

        let result = scan_goose(&mut ledger, &[root]);

        assert_eq!(result.events_inserted, 4);
        assert!(result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("file-state update failed")));
    }
}
