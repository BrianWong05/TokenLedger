use std::collections::HashSet;
use std::fs;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::adapters::{absolute_project, normalize_epoch, upsert_events_count};
use crate::types::{SourceScanResult, UsageEvent};

const SOURCE: &str = "kilo";
const SUPPORTED_SCHEMA: &[(&str, &[&str])] = &[
    (
        "session",
        &[
            "id",
            "project_id",
            "workspace_id",
            "parent_id",
            "slug",
            "directory",
            "path",
            "title",
            "version",
            "cost",
            "model",
            "tokens_input",
            "tokens_output",
            "tokens_reasoning",
            "tokens_cache_read",
            "tokens_cache_write",
            "time_created",
            "time_updated",
        ],
    ),
    ("message", &["id", "session_id", "time_created", "data"]),
];

#[derive(Default)]
struct DatabaseScan {
    events: Vec<UsageEvent>,
    lines_skipped: u64,
}

/// Scan Kilo CLI's current session database.
///
/// Kilo's database aggregates usage on the Session row rather than proving a
/// trustworthy timestamp for each Request. The Ledger therefore receives one
/// Usage Record per usage-bearing Session. Legacy editor migrations that contain
/// only zero-token rows naturally produce no Usage Records.
pub fn scan_kilo(conn: &mut Connection, database: &Path) -> SourceScanResult {
    if !database.exists() {
        return SourceScanResult::default();
    }
    if !database.is_file() {
        return SourceScanResult {
            error: Some(format!("{SOURCE}: unsupported Source Artifact file")),
            ..Default::default()
        };
    }

    let scan = match scan_database(database) {
        Ok(scan) => scan,
        Err(error) => {
            return SourceScanResult {
                error: Some(error),
                ..Default::default()
            }
        }
    };

    let events_inserted = match upsert_events_count(conn, &scan.events) {
        Ok(inserted) => inserted,
        Err(error) => {
            return SourceScanResult {
                lines_skipped: scan.lines_skipped,
                error: Some(format!("{SOURCE}: Ledger insert failed: {error}")),
                ..Default::default()
            }
        }
    };

    SourceScanResult {
        events_inserted,
        lines_skipped: scan.lines_skipped,
        ..Default::default()
    }
}

fn scan_database(path: &Path) -> Result<DatabaseScan, String> {
    let source_file = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let database = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("{SOURCE}: database open failed: {error}"))?;
    let _ = database.busy_timeout(std::time::Duration::from_millis(5000));
    ensure_schema(&database)?;

    let mut scan = DatabaseScan::default();
    let mut sessions = database
        .prepare(
            "SELECT id, directory, time_created, time_updated, model,
                    tokens_input, tokens_output, tokens_reasoning,
                    tokens_cache_read, tokens_cache_write
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
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, Option<i64>>(8)?,
                row.get::<_, Option<i64>>(9)?,
            ))
        })
        .map_err(|error| format!("{SOURCE}: session read failed: {error}"))?;

    for row in rows {
        let (
            session_id,
            directory,
            created,
            updated,
            model,
            input,
            output,
            reasoning,
            cache_read,
            cache_write,
        ) = row.map_err(|error| format!("{SOURCE}: session row failed: {error}"))?;

        let Some(session_id) = (!session_id.trim().is_empty()).then_some(session_id) else {
            scan.lines_skipped += 1;
            continue;
        };
        let Some(timestamp) = session_timestamp(updated, created) else {
            scan.lines_skipped += 1;
            continue;
        };
        let (Some(input), Some(output), Some(reasoning), Some(cache_read), Some(cache_write)) =
            (input, output, reasoning, cache_read, cache_write)
        else {
            scan.lines_skipped += 1;
            continue;
        };
        if [input, output, reasoning, cache_read, cache_write]
            .iter()
            .any(|value| *value < 0)
        {
            scan.lines_skipped += 1;
            continue;
        }
        // Kilo stores response tokens after subtracting reasoning. The
        // Ledger's canonical output field is gross output (ctx_buckets derives
        // response by subtracting reasoning), so restore that partition here.
        let Some(output) = output.checked_add(reasoning) else {
            scan.lines_skipped += 1;
            continue;
        };
        let Some(total) = [input, output, cache_read, cache_write]
            .into_iter()
            .try_fold(0i64, i64::checked_add)
        else {
            scan.lines_skipped += 1;
            continue;
        };
        if total <= 0 {
            scan.lines_skipped += 1;
            continue;
        }

        scan.events.push(UsageEvent {
            dedup_key: format!("{SOURCE}:session:{session_id}"),
            source: SOURCE.to_string(),
            timestamp,
            model: model_name(model.as_deref()),
            project: absolute_project(directory.as_deref()),
            api_calls: 1,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_write_5m_tokens: cache_write,
            cache_write_1h_tokens: 0,
            source_file: source_file.to_string_lossy().into_owned(),
            session_id: Some(session_id),
            reasoning_tokens: Some(reasoning),
            ctx: Default::default(),
        });
    }

    Ok(scan)
}

fn ensure_schema(database: &Connection) -> Result<(), String> {
    for (table, columns) in SUPPORTED_SCHEMA {
        let mut statement = database
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|error| format!("{SOURCE}: schema inspection failed: {error}"))?;
        let found = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|error| format!("{SOURCE}: schema inspection failed: {error}"))?
            .collect::<rusqlite::Result<HashSet<_>>>()
            .map_err(|error| format!("{SOURCE}: schema inspection failed: {error}"))?;
        if !columns.iter().all(|column| found.contains(*column)) {
            return Err(format!("{SOURCE}: unsupported database schema"));
        }
    }
    Ok(())
}

fn model_name(raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    let value = serde_json::from_str::<Value>(raw).ok()?;
    match value {
        Value::String(model) => (!model.trim().is_empty()).then_some(model),
        Value::Object(fields) => fields
            .get("id")
            .or_else(|| fields.get("modelID"))
            .and_then(Value::as_str)
            .filter(|model| !model.trim().is_empty())
            .map(str::to_string),
        _ => None,
    }
}

fn session_timestamp(updated: Option<i64>, created: Option<i64>) -> Option<i64> {
    updated
        .filter(|t| *t > 0)
        .map(normalize_epoch)
        .or_else(|| created.filter(|t| *t > 0).map(normalize_epoch))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn create_database(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        Connection::open(path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE session (
                    id TEXT PRIMARY KEY,
                    project_id TEXT,
                    workspace_id TEXT,
                    parent_id TEXT,
                    slug TEXT,
                    directory TEXT,
                    path TEXT,
                    title TEXT,
                    version TEXT,
                    cost REAL,
                    time_created INTEGER,
                    time_updated INTEGER,
                    model TEXT,
                    tokens_input INTEGER,
                    tokens_output INTEGER,
                    tokens_reasoning INTEGER,
                    tokens_cache_read INTEGER,
                    tokens_cache_write INTEGER
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

    fn insert_session(
        database: &Connection,
        id: &str,
        directory: Option<&str>,
        model: Option<&str>,
        tokens: [i64; 5],
    ) {
        database
            .execute(
                "INSERT INTO session (
                    id, directory, time_created, time_updated, model,
                    tokens_input, tokens_output, tokens_reasoning,
                    tokens_cache_read, tokens_cache_write
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    id,
                    directory,
                    1_780_000_000_000i64,
                    1_780_000_001_000i64,
                    model,
                    tokens[0],
                    tokens[1],
                    tokens[2],
                    tokens[3],
                    tokens[4],
                ],
            )
            .unwrap();
    }

    #[test]
    fn session_rows_are_ingested_once_with_unknown_models_and_zero_rows_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        let database_path = tmp.path().join("kilo.db");
        create_database(&database_path);
        let database = Connection::open(&database_path).unwrap();
        insert_session(
            &database,
            "session-known",
            Some("/Users/dev/projects/kilo"),
            Some(r#"{"id":"kilo-model","providerID":"provider"}"#),
            [30, 8, 2, 10, 1],
        );
        insert_session(
            &database,
            "session-unknown",
            Some("relative/project"),
            None,
            [2, 1, 0, 0, 0],
        );
        insert_session(
            &database,
            "session-reasoning",
            Some("/Users/dev/projects/reasoning"),
            Some(r#"{"id":"reasoning-model"}"#),
            [0, 0, 4, 0, 0],
        );
        insert_session(
            &database,
            "legacy-zero",
            Some("/Users/dev/legacy"),
            None,
            [0; 5],
        );
        database
            .execute(
                "INSERT INTO message (id, session_id, time_created, data)
                 VALUES ('private-message', 'legacy-zero', 1780000000000,
                         'KILO_PRIVATE_PROMPT_MARKER')",
                [],
            )
            .unwrap();
        drop(database);

        let ledger_path = tmp.path().join("ledger.db");
        let mut ledger = crate::db::open_db(&ledger_path).unwrap();
        let first = scan_kilo(&mut ledger, &database_path);
        assert_eq!(first.events_inserted, 3);
        assert!(first.lines_skipped > 0);
        assert!(first.error.is_none(), "unexpected error: {:?}", first.error);

        let known: (i64, i64, i64, i64, Option<String>, Option<String>) = ledger
            .query_row(
                "SELECT input_tokens, output_tokens, cache_read_tokens,
                        cache_write_5m_tokens, model, project
                 FROM events WHERE session_id = 'session-known'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            known,
            (
                30,
                10,
                10,
                1,
                Some("kilo-model".to_string()),
                Some("/Users/dev/projects/kilo".to_string())
            )
        );
        assert_eq!(
            ledger
                .query_row(
                    "SELECT output_tokens, reasoning_tokens
                     FROM events WHERE session_id = 'session-reasoning'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap(),
            (4i64, Some(4i64))
        );
        assert_eq!(
            ledger
                .query_row(
                    "SELECT model, project, reasoning_tokens, api_calls
                     FROM events WHERE session_id = 'session-unknown'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .unwrap(),
            (None::<String>, None::<String>, Some(0i64), 1i64)
        );

        let second = scan_kilo(&mut ledger, &database_path);
        assert_eq!(second.events_inserted, 0);
        assert_eq!(
            ledger
                .query_row(
                    "SELECT COUNT(*) FROM events WHERE source = 'kilo'",
                    [],
                    |row| { row.get::<_, i64>(0) }
                )
                .unwrap(),
            3
        );

        drop(ledger);
        let durable = ["", "-wal", "-shm"]
            .into_iter()
            .filter_map(|suffix| fs::read(format!("{}{}", ledger_path.display(), suffix)).ok())
            .flatten()
            .collect::<Vec<_>>();
        assert!(!durable
            .windows("KILO_PRIVATE_PROMPT_MARKER".len())
            .any(|window| window == b"KILO_PRIVATE_PROMPT_MARKER"));
    }

    #[test]
    fn a_changed_session_is_replaced_without_creating_a_duplicate() {
        let tmp = tempfile::tempdir().unwrap();
        let database_path = tmp.path().join("kilo.db");
        create_database(&database_path);
        let database = Connection::open(&database_path).unwrap();
        insert_session(
            &database,
            "session-1",
            Some("/project"),
            None,
            [1, 2, 0, 0, 0],
        );
        drop(database);

        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        assert_eq!(scan_kilo(&mut ledger, &database_path).events_inserted, 1);

        let moved_path = tmp.path().join("moved/kilo.db");
        fs::create_dir_all(moved_path.parent().unwrap()).unwrap();
        fs::copy(&database_path, &moved_path).unwrap();
        assert_eq!(scan_kilo(&mut ledger, &moved_path).events_inserted, 0);
        assert_eq!(
            ledger
                .query_row("SELECT COUNT(*) FROM events WHERE source = 'kilo'", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );

        let database = Connection::open(&database_path).unwrap();
        database
            .execute(
                "UPDATE session SET tokens_output = 20 WHERE id = 'session-1'",
                [],
            )
            .unwrap();
        drop(database);

        assert_eq!(scan_kilo(&mut ledger, &database_path).events_inserted, 0);
        assert_eq!(
            ledger
                .query_row(
                    "SELECT output_tokens FROM events WHERE dedup_key = 'kilo:session:session-1'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            20
        );
    }

    #[test]
    fn unsupported_database_is_reported_without_writing_usage() {
        let tmp = tempfile::tempdir().unwrap();
        let database_path = tmp.path().join("kilo.db");
        Connection::open(&database_path)
            .unwrap()
            .execute_batch("CREATE TABLE unrelated (id TEXT PRIMARY KEY);")
            .unwrap();
        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();

        let result = scan_kilo(&mut ledger, &database_path);

        assert_eq!(result.events_inserted, 0);
        assert!(result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("kilo") && error.contains("unsupported")));
        assert_eq!(
            ledger
                .query_row("SELECT COUNT(*) FROM events", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn legacy_session_schema_is_reported_as_unsupported() {
        let tmp = tempfile::tempdir().unwrap();
        let database_path = tmp.path().join("kilo.db");
        Connection::open(&database_path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE session (
                    id TEXT PRIMARY KEY,
                    directory TEXT,
                    time_created INTEGER,
                    time_updated INTEGER,
                    model TEXT,
                    tokens_input INTEGER,
                    tokens_output INTEGER,
                    tokens_reasoning INTEGER,
                    tokens_cache_read INTEGER,
                    tokens_cache_write INTEGER
                );
                CREATE TABLE message (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    time_created INTEGER NOT NULL,
                    data TEXT NOT NULL
                );",
            )
            .unwrap();
        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();

        let result = scan_kilo(&mut ledger, &database_path);

        assert_eq!(result.events_inserted, 0);
        assert_eq!(
            result.error.as_deref(),
            Some("kilo: unsupported database schema")
        );
    }
}
