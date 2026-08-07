//! Qoder IDE Source adapter.
//!
//! Qoder (a VS Code–based AI coding IDE) stores every chat message in a SQLite
//! database at `~/Library/Application Support/QoderCN/SharedClientCache/cache/db/local.db`
//! (macOS; equivalent paths on Linux/Windows). The `chat_message` table carries
//! one row per message; assistant rows hold a `token_info` JSON blob
//! (`prompt_tokens`, `completion_tokens`, `cached_tokens`) and a `model_info`
//! JSON blob (`model_key`). Each usage-bearing assistant row is one Usage Record
//! (one Request), deduplicating on the row `id`.
//!
//! Cache semantics (ADR-0001): `prompt_tokens` **includes** the cache read
//! (`cached_tokens`), so Input Tokens = `prompt_tokens − cached_tokens`. The
//! Artifact exposes no cache-write or reasoning figures, and no Context tiers.
//! `max_input_tokens` is a context-window size, never usage, and is ignored.
//! The message `content` is never read — only numeric usage and identity fields.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::adapters::{absolute_project, normalize_epoch, upsert_events_count};
use crate::types::{SourceScanResult, UsageEvent};

const SOURCE: &str = "qoder";
const SUPPORTED_SCHEMA: &[(&str, &[&str])] = &[(
    "chat_message",
    &[
        "id",
        "session_id",
        "role",
        "token_info",
        "model_info",
        "gmt_create",
    ],
)];

#[derive(Default)]
struct DatabaseScan {
    events: Vec<UsageEvent>,
    lines_skipped: u64,
}

/// Scan Qoder IDE's local chat database. Each usage-bearing assistant message
/// is one Usage Record; rows without `token_info` or with all-zero tokens
/// produce no Record. Idempotent: re-scanning a stable database books nothing
/// new (dedup on `qoder:<row id>`).
pub fn scan_qoder(conn: &mut Connection, database: &Path) -> SourceScanResult {
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
        error: None,
    }
}

fn scan_database(path: &Path) -> Result<DatabaseScan, String> {
    let source_file = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let database = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("{SOURCE}: database open failed: {error}"))?;
    let _ = database.busy_timeout(std::time::Duration::from_millis(5000));
    ensure_schema(&database)?;

    let mut scan = DatabaseScan::default();
    // Only rows with a non-empty token_info can carry usage; the role guard
    // keeps user/tool rows (which never carry usage) out of the parse path.
    let mut stmt = database
        .prepare(
            "SELECT id, session_id, token_info, model_info, gmt_create \
             FROM chat_message \
             WHERE token_info IS NOT NULL AND token_info != '' \
             ORDER BY id",
        )
        .map_err(|error| format!("{SOURCE}: chat_message query failed: {error}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<i64>>(4)?,
            ))
        })
        .map_err(|error| format!("{SOURCE}: chat_message read failed: {error}"))?;

    for row in rows {
        let (id, session_id, token_info, model_info, gmt_create) =
            row.map_err(|error| format!("{SOURCE}: chat_message row failed: {error}"))?;

        let Some(timestamp) = gmt_create.filter(|t| *t > 0).map(normalize_epoch) else {
            scan.lines_skipped += 1;
            continue;
        };

        let usage = match parse_usage(&token_info) {
            Some(u) => u,
            None => {
                scan.lines_skipped += 1;
                continue;
            }
        };

        let model = model_info
            .as_deref()
            .and_then(parse_model)
            .filter(|m| !m.is_empty());

        let session = session_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);

        scan.events.push(UsageEvent {
            dedup_key: format!("{SOURCE}:{id}"),
            source: SOURCE.to_string(),
            timestamp,
            model,
            // The Artifact exposes no project (cwd) field on a chat_message row.
            project: absolute_project(None),
            api_calls: 1,
            input_tokens: usage.input,
            output_tokens: usage.output,
            cache_read_tokens: usage.cache_read,
            cache_write_5m_tokens: 0,
            cache_write_1h_tokens: 0,
            source_file: source_file.to_string_lossy().into_owned(),
            session_id: session,
            reasoning_tokens: None,
            ctx: Default::default(),
        });
    }

    Ok(scan)
}

struct ExtractedUsage {
    input: i64,
    output: i64,
    cache_read: i64,
}

/// Parse `token_info` JSON: `{"prompt_tokens","completion_tokens","cached_tokens","max_input_tokens"}`.
/// `prompt_tokens` includes the cache read, so Input = prompt − cached (ADR-0001).
/// `max_input_tokens` is a context-window size and is ignored. Returns None for
/// a zero-token observation (not a Usage Record) or an unparseable blob.
fn parse_usage(token_info: &str) -> Option<ExtractedUsage> {
    let v: Value = serde_json::from_str(token_info).ok()?;
    let prompt = v.get("prompt_tokens").and_then(Value::as_i64).unwrap_or(0);
    let completion = v
        .get("completion_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let cached = v.get("cached_tokens").and_then(Value::as_i64).unwrap_or(0);

    let input = (prompt - cached).max(0);
    let output = completion;
    let cache_read = cached;

    // A zero-token observation is not a Usage Record (glossary).
    if input == 0 && output == 0 && cache_read == 0 {
        return None;
    }

    Some(ExtractedUsage {
        input,
        output,
        cache_read,
    })
}

/// Parse `model_info` JSON: `{"model_key":"qmodel_38max"}`. Returns the raw
/// model_key string (display + price matching use the raw logged name).
fn parse_model(model_info: &str) -> Option<String> {
    let v: Value = serde_json::from_str(model_info).ok()?;
    v.get("model_key")
        .and_then(Value::as_str)
        .map(str::to_owned)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn create_database(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        Connection::open(path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE chat_message (
                    id VARCHAR(64) PRIMARY KEY,
                    session_id VARCHAR(64),
                    request_id VARCHAR(64),
                    role VARCHAR(64),
                    content text,
                    summary text,
                    summary_modified INTEGER,
                    summary_trigger INTEGER DEFAULT 0,
                    tool_result text,
                    token_info text,
                    model_info text,
                    extra text DEFAULT '',
                    gmt_create INTEGER
                );",
            )
            .unwrap();
    }

    fn insert_message(
        db: &Connection,
        id: &str,
        session: Option<&str>,
        role: &str,
        token_info: Option<&str>,
        model_info: Option<&str>,
        gmt_create: i64,
    ) {
        db.execute(
            "INSERT INTO chat_message (id, session_id, role, token_info, model_info, gmt_create)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![id, session, role, token_info, model_info, gmt_create],
        )
        .unwrap();
    }

    #[test]
    fn assistant_messages_with_usage_become_records_with_cache_split() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("local.db");
        create_database(&db_path);
        let db = Connection::open(&db_path).unwrap();
        // prompt_tokens includes cached: 25038 = 420 fresh + 24618 cached.
        insert_message(
            &db,
            "m1",
            Some("task-abc.session.execution"),
            "assistant",
            Some(
                r#"{"prompt_tokens":25038,"completion_tokens":470,"cached_tokens":24618,"max_input_tokens":1000000}"#,
            ),
            Some(r#"{"model_key":"qmodel_38max"}"#),
            1_786_112_276_027i64,
        );
        // No cache: all prompt tokens are fresh input.
        insert_message(
            &db,
            "m2",
            Some("task-abc.session.execution"),
            "assistant",
            Some(
                r#"{"prompt_tokens":24624,"completion_tokens":234,"cached_tokens":0,"max_input_tokens":1000000}"#,
            ),
            Some(r#"{"model_key":"qmodel_38max"}"#),
            1_786_112_300_884i64,
        );
        drop(db);

        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        let result = scan_qoder(&mut ledger, &db_path);
        assert_eq!(result.events_inserted, 2);
        assert!(
            result.error.is_none(),
            "unexpected error: {:?}",
            result.error
        );

        let rows: Vec<(String, Option<String>, i64, i64, i64, Option<String>)> = ledger
            .prepare(
                "SELECT dedup_key, model, input_tokens, output_tokens, cache_read_tokens, session_id \
                 FROM events WHERE source = 'qoder' ORDER BY dedup_key",
            )
            .unwrap()
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "qoder:m1");
        assert_eq!(rows[0].1.as_deref(), Some("qmodel_38max"));
        assert_eq!(rows[0].2, 420); // 25038 − 24618
        assert_eq!(rows[0].3, 470);
        assert_eq!(rows[0].4, 24618);
        assert_eq!(rows[0].5.as_deref(), Some("task-abc.session.execution"));
        assert_eq!(rows[1].0, "qoder:m2");
        assert_eq!(rows[1].2, 24624);
        assert_eq!(rows[1].4, 0);
    }

    #[test]
    fn zero_token_and_missing_token_info_rows_produce_no_records() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("local.db");
        create_database(&db_path);
        let db = Connection::open(&db_path).unwrap();
        // Zero-token: not a Record.
        insert_message(
            &db,
            "zero",
            Some("s"),
            "assistant",
            Some(
                r#"{"prompt_tokens":0,"completion_tokens":0,"cached_tokens":0,"max_input_tokens":1000000}"#,
            ),
            Some(r#"{"model_key":"qmodel_38max"}"#),
            1_786_112_276_027i64,
        );
        // No token_info at all (user/tool row shape).
        insert_message(
            &db,
            "none",
            Some("s"),
            "user",
            None,
            None,
            1_786_112_276_028,
        );
        // Empty token_info string.
        insert_message(
            &db,
            "empty",
            Some("s"),
            "assistant",
            Some(""),
            None,
            1_786_112_276_029,
        );
        drop(db);

        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        let result = scan_qoder(&mut ledger, &db_path);
        assert_eq!(result.events_inserted, 0);
        assert!(result.error.is_none());
        // zero is skipped at parse (zero-token), none/empty are filtered by the
        // WHERE clause; zero counts as a skipped line.
        assert!(result.lines_skipped >= 1);
    }

    #[test]
    fn rescan_is_idempotent_and_missing_model_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("local.db");
        create_database(&db_path);
        let db = Connection::open(&db_path).unwrap();
        insert_message(
            &db,
            "m1",
            Some("s"),
            "assistant",
            Some(
                r#"{"prompt_tokens":100,"completion_tokens":10,"cached_tokens":0,"max_input_tokens":1000000}"#,
            ),
            None, // no model_info
            1_786_112_276_027i64,
        );
        drop(db);

        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        let first = scan_qoder(&mut ledger, &db_path);
        assert_eq!(first.events_inserted, 1);
        let second = scan_qoder(&mut ledger, &db_path);
        assert_eq!(second.events_inserted, 0, "rescan books nothing new");

        let model: Option<String> = ledger
            .query_row("SELECT model FROM events WHERE source = 'qoder'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(model, None, "missing model_info yields no model");
    }

    #[test]
    fn unsupported_database_is_reported_without_writing_usage() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("local.db");
        Connection::open(&db_path)
            .unwrap()
            .execute_batch("CREATE TABLE unrelated (id TEXT PRIMARY KEY);")
            .unwrap();
        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();

        let result = scan_qoder(&mut ledger, &db_path);
        assert_eq!(result.events_inserted, 0);
        assert!(result
            .error
            .as_deref()
            .is_some_and(|e| e.contains("qoder") && e.contains("unsupported")));
    }

    #[test]
    fn missing_database_is_scanned_quietly() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        let result = scan_qoder(&mut ledger, &tmp.path().join("does-not-exist.db"));
        assert_eq!(result.events_inserted, 0);
        assert!(result.error.is_none());
    }

    #[test]
    fn privacy_markers_never_enter_the_ledger() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("local.db");
        create_database(&db_path);
        let db = Connection::open(&db_path).unwrap();
        // content carries a private marker; only numeric usage may be read.
        db.execute(
            "INSERT INTO chat_message (id, session_id, role, content, token_info, model_info, gmt_create)
             VALUES ('m1', 's', 'assistant', 'QODER_PRIVATE_PROMPT_MARKER',
                     '{\"prompt_tokens\":100,\"completion_tokens\":10,\"cached_tokens\":0,\"max_input_tokens\":1000000}',
                     '{\"model_key\":\"qmodel_38max\"}', 1786112276027)",
            [],
        )
        .unwrap();
        drop(db);

        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        scan_qoder(&mut ledger, &db_path);

        let durable = fs::read(tmp.path().join("ledger.db")).unwrap();
        assert!(!durable
            .windows("QODER_PRIVATE_PROMPT_MARKER".len())
            .any(|w| w == b"QODER_PRIVATE_PROMPT_MARKER"));
    }
}
