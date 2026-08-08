use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::{Map, Value};

use crate::adapters::{is_absolute_path, normalize_epoch, upsert_events_count};
use crate::time::iso_to_epoch;
use crate::types::{SourceScanResult, UsageEvent};

const SOURCE: &str = "zed";
const HOSTED_PROVIDER: &str = "zed.dev";
const SUPPORTED_VERSIONS: &[&str] = &["0.2.0", "0.3.0"];
const REQUIRED_COLUMNS: &[&str] = &["id", "summary", "updated_at", "data_type", "data"];

#[derive(Default)]
struct DatabaseScan {
    events: Vec<UsageEvent>,
    lines_skipped: u64,
    warnings: Vec<String>,
}

struct ThreadRow {
    id: String,
    updated_at: String,
    data_type: String,
    data: Vec<u8>,
    folder_paths: Option<String>,
}

/// Scan Zed's native hosted-model thread database.
///
/// Zed stores cumulative usage on a whole thread and does not provide a
/// trustworthy timestamp for each request. The Ledger therefore receives one
/// Usage Record per usage-bearing Session. Only the native hosted provider is
/// proven to be Zed usage; external ACP providers are excluded before any
/// Usage Record is persisted.
pub fn scan_zed(conn: &mut Connection, databases: &[PathBuf]) -> SourceScanResult {
    let mut roots = Vec::new();
    for database in databases {
        push_unique_path(&mut roots, database.clone());
    }

    let mut events = HashMap::new();
    let mut lines_skipped = 0;
    let mut warnings = Vec::new();

    for database in roots {
        if !database.exists() {
            continue;
        }
        if !database.is_file() {
            warnings.push(format!("{SOURCE}: unsupported Source Artifact file"));
            continue;
        }

        match scan_database(&database) {
            Ok(scan) => {
                lines_skipped += scan.lines_skipped;
                warnings.extend(scan.warnings);
                for event in scan.events {
                    let key = event.dedup_key.clone();
                    let replace = events
                        .get(&key)
                        .map(|existing| prefer_event(existing, &event))
                        .unwrap_or(true);
                    if replace {
                        events.insert(key, event);
                    }
                }
            }
            Err(error) => warnings.push(error),
        }
    }

    let mut events = events.into_values().collect::<Vec<_>>();
    events.sort_by(|left, right| left.dedup_key.cmp(&right.dedup_key));
    let events_inserted = match upsert_events_count(conn, &events) {
        Ok(inserted) => inserted,
        Err(error) => {
            warnings.push(format!("{SOURCE}: Ledger insert failed: {error}"));
            0
        }
    };

    SourceScanResult {
        events_inserted,
        lines_skipped,
        error: (!warnings.is_empty()).then(|| warnings.join("; ")),
        ..Default::default()
    }
}

fn scan_database(path: &Path) -> Result<DatabaseScan, String> {
    let source_file = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let database = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| format!("{SOURCE}: database open failed"))?;
    let _ = database.busy_timeout(std::time::Duration::from_millis(5000));
    let columns = ensure_schema(&database)?;
    let folder_paths = columns.contains("folder_paths");
    let query = if folder_paths {
        "SELECT id, updated_at, data_type, data, folder_paths
         FROM threads ORDER BY id"
    } else {
        "SELECT id, updated_at, data_type, data, NULL
         FROM threads ORDER BY id"
    };
    let mut statement = database
        .prepare(query)
        .map_err(|_| format!("{SOURCE}: thread query failed"))?;
    let rows = statement
        .query_map([], |row| {
            Ok(ThreadRow {
                id: row.get(0)?,
                updated_at: row.get(1)?,
                data_type: row.get(2)?,
                data: row.get(3)?,
                folder_paths: row.get(4)?,
            })
        })
        .map_err(|_| format!("{SOURCE}: thread read failed"))?;

    let mut scan = DatabaseScan::default();
    for row in rows {
        let row = match row {
            Ok(row) => row,
            Err(_) => {
                scan.lines_skipped += 1;
                scan.warnings
                    .push(format!("{SOURCE}: malformed thread row"));
                continue;
            }
        };
        match event_from_thread(row, &source_file) {
            Ok(Some(event)) => scan.events.push(event),
            Ok(None) => scan.lines_skipped += 1,
            Err(error) => {
                scan.lines_skipped += 1;
                scan.warnings.push(error);
            }
        }
    }

    Ok(scan)
}

fn ensure_schema(database: &Connection) -> Result<HashSet<String>, String> {
    let mut statement = database
        .prepare("PRAGMA table_info(threads)")
        .map_err(|_| format!("{SOURCE}: schema inspection failed"))?;
    let found = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|_| format!("{SOURCE}: schema inspection failed"))?
        .collect::<rusqlite::Result<HashSet<_>>>()
        .map_err(|_| format!("{SOURCE}: schema inspection failed"))?;
    if !REQUIRED_COLUMNS
        .iter()
        .all(|column| found.contains(*column))
    {
        return Err(format!("{SOURCE}: unsupported database schema"));
    }
    Ok(found)
}

fn event_from_thread(row: ThreadRow, source_file: &Path) -> Result<Option<UsageEvent>, String> {
    let id = row.id.trim();
    if id.is_empty() {
        return Err(format!("{SOURCE}: malformed thread row"));
    }
    let timestamp = parse_timestamp(&row.updated_at)
        .ok_or_else(|| format!("{SOURCE}: malformed thread timestamp"))?;
    let data = decode_thread(&row.data_type, &row.data)?;
    let version = data
        .get("version")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{SOURCE}: unsupported thread version"))?;
    if !SUPPORTED_VERSIONS.contains(&version) {
        return Err(format!("{SOURCE}: unsupported thread version"));
    }

    // Native DbThread persistence has this shape. ACP threads use a separate
    // representation, so an ambiguous row is safer to leave unrecorded than
    // to attribute to Zed merely because it shares the database.
    if !is_native_thread(&data) {
        return Ok(None);
    }

    let model = data.get("model").and_then(Value::as_object);
    let provider = model.and_then(|model| model.get("provider").and_then(Value::as_str));
    if provider != Some(HOSTED_PROVIDER) {
        // A missing or different provider may be an external ACP agent. It is
        // never safe to infer that its usage belongs to Zed.
        return Ok(None);
    }

    let usage = data
        .get("cumulative_token_usage")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{SOURCE}: malformed token usage"))?;
    let input = token_value(usage, "input_tokens")?;
    let output = token_value(usage, "output_tokens")?;
    let cache_write = token_value(usage, "cache_creation_input_tokens")?;
    let cache_read = token_value(usage, "cache_read_input_tokens")?;
    let total = [input, output, cache_read, cache_write]
        .into_iter()
        .try_fold(0i64, i64::checked_add)
        .ok_or_else(|| format!("{SOURCE}: token count overflow"))?;
    if total <= 0 {
        return Ok(None);
    }

    let model = model
        .and_then(|model| model.get("model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string);
    let api_calls = request_count(&data).max(1);

    Ok(Some(UsageEvent {
        dedup_key: format!("{SOURCE}:thread:{id}"),
        source: SOURCE.to_string(),
        timestamp,
        model,
        project: absolute_project(row.folder_paths.as_deref()),
        api_calls,
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_write_5m_tokens: cache_write,
        cache_write_1h_tokens: 0,
        source_file: source_file.to_string_lossy().into_owned(),
        session_id: Some(id.to_string()),
        reasoning_tokens: None,
        ctx: Default::default(),
    }))
}

fn is_native_thread(data: &Value) -> bool {
    data.get("messages").is_some_and(Value::is_array)
        && data
            .get("cumulative_token_usage")
            .is_some_and(Value::is_object)
        && data
            .get("request_token_usage")
            .is_some_and(Value::is_object)
}

fn decode_thread(data_type: &str, data: &[u8]) -> Result<Value, String> {
    let decoded = match data_type.trim().to_ascii_lowercase().as_str() {
        "zstd" => zstd::decode_all(data).map_err(|_| format!("{SOURCE}: malformed thread data"))?,
        _ => return Err(format!("{SOURCE}: unsupported thread data type")),
    };
    serde_json::from_slice(&decoded).map_err(|_| format!("{SOURCE}: malformed thread data"))
}

fn token_value(usage: &Map<String, Value>, key: &str) -> Result<i64, String> {
    let Some(value) = usage.get(key) else {
        return Ok(0);
    };
    let value = value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .ok_or_else(|| format!("{SOURCE}: malformed token usage"))?;
    if value < 0 {
        return Err(format!("{SOURCE}: malformed token usage"));
    }
    Ok(value)
}

fn request_count(data: &Value) -> i64 {
    let count = data
        .get("request_token_usage")
        .and_then(Value::as_object)
        .map(|requests| requests.len())
        .and_then(|count| i64::try_from(count).ok())
        .unwrap_or(0);
    count.max(1)
}

fn absolute_project(folder_paths: Option<&str>) -> Option<String> {
    folder_paths?
        .split('\n')
        .map(str::trim)
        .find(|path| is_absolute_path(path))
        .map(str::to_string)
}

fn parse_timestamp(value: &str) -> Option<i64> {
    let value = value.trim();
    if let Ok(timestamp) = value.parse::<i64>() {
        return (timestamp > 0).then(|| normalize_epoch(timestamp));
    }
    iso_to_epoch(value)
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    let normalized = path_identity(&path);
    if !paths
        .iter()
        .any(|existing| path_identity(existing) == normalized)
    {
        paths.push(path);
    }
}

fn path_identity(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| normalized_path(path))
}

fn normalized_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn prefer_event(existing: &UsageEvent, candidate: &UsageEvent) -> bool {
    candidate.timestamp > existing.timestamp
        || (candidate.timestamp == existing.timestamp
            && event_total(candidate) > event_total(existing))
}

fn event_total(event: &UsageEvent) -> i64 {
    [
        event.input_tokens,
        event.output_tokens,
        event.cache_read_tokens,
        event.cache_write_5m_tokens,
        event.cache_write_1h_tokens,
    ]
    .into_iter()
    .try_fold(0i64, i64::checked_add)
    .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use serde_json::json;
    use std::fs;
    use std::path::Path;

    const HOSTED_PROVIDER: &str = "zed.dev";
    const PRIVATE_PROMPT: &str = "ZED_PRIVATE_PROMPT_MARKER";
    const PRIVATE_RESPONSE: &str = "ZED_PRIVATE_RESPONSE_MARKER";
    const PRIVATE_TOOL: &str = "ZED_PRIVATE_TOOL_MARKER";

    fn create_database(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        Connection::open(path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE threads (
                    id TEXT PRIMARY KEY,
                    summary TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    data_type TEXT NOT NULL,
                    data BLOB NOT NULL,
                    parent_id TEXT,
                    folder_paths TEXT,
                    folder_paths_order TEXT,
                    created_at TEXT
                );",
            )
            .unwrap();
    }

    fn insert_thread(
        database: &Connection,
        id: &str,
        provider: Option<&str>,
        model: Option<&str>,
        updated_at: &str,
        usage: [u64; 4],
        request_count: usize,
        folder_paths: Option<&str>,
        private_content: &str,
    ) {
        let request_token_usage = (0..request_count)
            .map(|index| {
                (
                    format!("request-{index}"),
                    json!({"input_tokens": 1, "output_tokens": 1}),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        let mut data = json!({
            "version": "0.3.0",
            "messages": [{
                "role": "user",
                "content": private_content,
                "tool_arguments": PRIVATE_TOOL,
                "assistant": PRIVATE_RESPONSE
            }],
            "cumulative_token_usage": {
                "input_tokens": usage[0],
                "output_tokens": usage[1],
                "cache_creation_input_tokens": usage[2],
                "cache_read_input_tokens": usage[3]
            },
            "request_token_usage": request_token_usage,
            "model": provider.map(|provider| json!({
                "provider": provider,
                "model": model
            }))
        });
        if id.starts_with("external-acp") {
            // ACP persistence does not use the native DbThread message and
            // request-usage shape. Keep this row hosted-looking to prove the
            // scanner fails closed on identity rather than provider alone.
            let object = data.as_object_mut().unwrap();
            object.remove("messages");
            object.remove("request_token_usage");
        }
        let compressed = zstd::encode_all(data.to_string().as_bytes(), 0).unwrap();
        database
            .execute(
                "INSERT INTO threads (
                    id, summary, updated_at, data_type, data, folder_paths, folder_paths_order
                 ) VALUES (?1, ?2, ?3, 'zstd', ?4, ?5, '0')",
                params![id, PRIVATE_PROMPT, updated_at, compressed, folder_paths],
            )
            .unwrap();
    }

    fn stored_events(
        ledger: &Connection,
    ) -> Vec<(
        String,
        Option<String>,
        i64,
        i64,
        i64,
        i64,
        i64,
        Option<String>,
    )> {
        let mut statement = ledger
            .prepare(
                "SELECT dedup_key, model, api_calls, input_tokens, output_tokens,
                        cache_read_tokens, cache_write_5m_tokens, project
                 FROM events WHERE source = 'zed' ORDER BY dedup_key",
            )
            .unwrap();
        statement
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
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    #[test]
    fn hosted_sessions_are_attributed_at_session_granularity_and_acp_is_excluded() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("zed/threads/threads.db");
        create_database(&database_path);
        let database = Connection::open(&database_path).unwrap();
        insert_thread(
            &database,
            "hosted-model",
            Some(HOSTED_PROVIDER),
            Some("zed-hosted-model"),
            "2026-07-01T10:00:00Z",
            [100, 20, 30, 10],
            2,
            Some("/Users/dev/projects/zed\n/Users/dev/projects/other"),
            PRIVATE_PROMPT,
        );
        insert_thread(
            &database,
            "hosted-unknown-model",
            Some(HOSTED_PROVIDER),
            None,
            "2026-07-01T11:00:00Z",
            [1, 2, 0, 0],
            0,
            Some("relative/project"),
            PRIVATE_RESPONSE,
        );
        insert_thread(
            &database,
            "external-acp-agent",
            Some(HOSTED_PROVIDER),
            Some("agent-model"),
            "2026-07-01T12:00:00Z",
            [900, 800, 700, 600],
            4,
            Some("/Users/dev/projects/acp"),
            PRIVATE_TOOL,
        );
        insert_thread(
            &database,
            "unproven-provider",
            None,
            Some("looks-like-a-model"),
            "2026-07-01T13:00:00Z",
            [1000, 1000, 1000, 1000],
            1,
            Some("/Users/dev/projects/unproven"),
            PRIVATE_PROMPT,
        );
        insert_thread(
            &database,
            "zero-usage",
            Some(HOSTED_PROVIDER),
            Some("zed-hosted-model"),
            "2026-07-01T14:00:00Z",
            [0, 0, 0, 0],
            1,
            Some("/Users/dev/projects/zero"),
            PRIVATE_RESPONSE,
        );
        drop(database);

        let ledger_path = directory.path().join("ledger.db");
        let mut ledger = crate::db::open_db(&ledger_path).unwrap();
        let equivalent_database_path = database_path.parent().unwrap().join(".").join("threads.db");
        let first = scan_zed(
            &mut ledger,
            &[database_path.clone(), equivalent_database_path],
        );

        assert_eq!(first.events_inserted, 2);
        assert!(first.lines_skipped >= 3);
        assert!(first.error.is_none(), "unexpected error: {:?}", first.error);
        assert_eq!(stored_events(&ledger).len(), 2);
        assert_eq!(
            stored_events(&ledger),
            vec![
                (
                    "zed:thread:hosted-model".to_string(),
                    Some("zed-hosted-model".to_string()),
                    2,
                    100,
                    20,
                    10,
                    30,
                    Some("/Users/dev/projects/zed".to_string()),
                ),
                (
                    "zed:thread:hosted-unknown-model".to_string(),
                    None,
                    1,
                    1,
                    2,
                    0,
                    0,
                    None,
                ),
            ]
        );

        let second = scan_zed(&mut ledger, std::slice::from_ref(&database_path));
        assert_eq!(second.events_inserted, 0);
        assert_eq!(stored_events(&ledger).len(), 2);
        drop(ledger);

        let ledger_bytes = fs::read(&ledger_path).unwrap();
        assert!(!ledger_bytes
            .windows(PRIVATE_PROMPT.len())
            .any(|window| window == PRIVATE_PROMPT.as_bytes()));
        assert!(!ledger_bytes
            .windows(PRIVATE_RESPONSE.len())
            .any(|window| window == PRIVATE_RESPONSE.as_bytes()));
        assert!(!ledger_bytes
            .windows(PRIVATE_TOOL.len())
            .any(|window| window == PRIVATE_TOOL.as_bytes()));
    }

    #[test]
    fn unsupported_database_is_reported_without_writing_usage() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("threads.db");
        Connection::open(&database_path)
            .unwrap()
            .execute_batch("CREATE TABLE unrelated (id TEXT PRIMARY KEY);")
            .unwrap();
        let mut ledger = crate::db::open_db(&directory.path().join("ledger.db")).unwrap();

        let result = scan_zed(&mut ledger, std::slice::from_ref(&database_path));

        assert_eq!(result.events_inserted, 0);
        assert!(result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("zed") && error.contains("unsupported")));
        assert_eq!(
            ledger
                .query_row("SELECT COUNT(*) FROM events", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn unsupported_thread_version_is_reported_without_writing_usage() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("threads.db");
        create_database(&database_path);
        let database = Connection::open(&database_path).unwrap();
        let data = json!({
            "version": "9.9.9",
            "cumulative_token_usage": {"input_tokens": 1, "output_tokens": 1},
            "request_token_usage": {"request-1": {}},
            "model": {"provider": HOSTED_PROVIDER, "model": "future-zed-model"}
        });
        let compressed = zstd::encode_all(data.to_string().as_bytes(), 0).unwrap();
        database
            .execute(
                "INSERT INTO threads (
                    id, summary, updated_at, data_type, data, folder_paths, folder_paths_order
                 ) VALUES ('future', 'summary', '2026-07-01T10:00:00Z',
                           'zstd', ?1, NULL, NULL)",
                [compressed],
            )
            .unwrap();
        drop(database);
        let mut ledger = crate::db::open_db(&directory.path().join("ledger.db")).unwrap();

        let result = scan_zed(&mut ledger, std::slice::from_ref(&database_path));

        assert_eq!(result.events_inserted, 0);
        assert_eq!(result.lines_skipped, 1);
        assert!(result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("zed") && error.contains("unsupported")));
    }

    #[test]
    fn malformed_thread_rows_are_skipped_without_leaking_content() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("threads.db");
        create_database(&database_path);
        let database = Connection::open(&database_path).unwrap();
        database
            .execute(
                "INSERT INTO threads (
                    id, summary, updated_at, data_type, data, folder_paths, folder_paths_order
                 ) VALUES ('malformed', ?1, '2026-07-01T10:00:00Z', 'zstd', ?2, NULL, NULL)",
                params![PRIVATE_PROMPT, b"not zstd".as_slice()],
            )
            .unwrap();
        drop(database);
        let mut ledger = crate::db::open_db(&directory.path().join("ledger.db")).unwrap();

        let result = scan_zed(&mut ledger, std::slice::from_ref(&database_path));

        assert_eq!(result.events_inserted, 0);
        assert_eq!(result.lines_skipped, 1);
        assert!(result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("zed") && error.contains("malformed")));
    }
}
