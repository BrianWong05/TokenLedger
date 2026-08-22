//! GitHub Copilot CLI usage from its native metadata-only SQLite ledger.

use std::path::Path;

use rusqlite::Connection;

use super::{file_state_of, unchanged, upsert_events_count};
use crate::db::set_file_state;
use crate::time::iso_to_epoch;
use crate::types::{SourceScanResult, UsageEvent};

const SCHEMA_VERSION: i64 = 6;

pub fn scan_copilot(conn: &mut Connection, database: &Path) -> SourceScanResult {
    let db_state = file_state_of(database);
    let wal = database.with_extension("db-wal");
    let wal_state = file_state_of(&wal);
    if !database.is_file() {
        return SourceScanResult::default();
    }
    if unchanged(conn, database, &db_state) && unchanged(conn, &wal, &wal_state) {
        return SourceScanResult::default();
    }

    let ro = match super::open_sqlite_artifact("copilot", database) {
        Ok(connection) => connection,
        // The reader's refusal is already Source-named, so it skips failed()'s prefix.
        Err(error) => {
            return SourceScanResult {
                error: Some(error),
                ..Default::default()
            }
        }
    };
    let version = ro.query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
        row.get::<_, i64>(0)
    });
    match version {
        Ok(SCHEMA_VERSION) => {}
        Ok(version) => return failed(format!("unsupported schema version {version}")),
        Err(error) => return failed(format!("schema check failed: {error}")),
    }

    let mut statement = match ro.prepare(
        "SELECT id, session_id, model, input_tokens, output_tokens, \
                cache_read_tokens, cache_write_tokens, reasoning_tokens, \
                token_details_json, created_at \
         FROM assistant_usage_events",
    ) {
        Ok(statement) => statement,
        Err(error) => return failed(format!("query failed: {error}")),
    };
    let rows = match statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<i64>>(3)?.unwrap_or(0),
            row.get::<_, Option<i64>>(4)?.unwrap_or(0),
            row.get::<_, Option<i64>>(5)?.unwrap_or(0),
            row.get::<_, Option<i64>>(6)?.unwrap_or(0),
            row.get::<_, Option<i64>>(7)?.unwrap_or(0),
            row.get::<_, Option<String>>(8)?,
            row.get::<_, String>(9)?,
        ))
    }) {
        Ok(rows) => rows,
        Err(error) => return failed(format!("read failed: {error}")),
    };

    let mut events = Vec::new();
    let mut skipped = 0;
    for row in rows {
        let Ok((
            id,
            session_id,
            model,
            input,
            output,
            cache_read,
            cache_write,
            reasoning,
            details,
            created_at,
        )) = row
        else {
            skipped += 1;
            continue;
        };
        let Some(timestamp) = iso_to_epoch(&created_at) else {
            skipped += 1;
            continue;
        };
        if id <= 0
            || timestamp <= 0
            || session_id.trim().is_empty()
            || model.trim().is_empty()
            || [input, output, cache_read, cache_write, reasoning]
                .iter()
                .any(|value| *value < 0)
        {
            skipped += 1;
            continue;
        }

        let (fresh_input, cache_read, cache_write) = details
            .as_deref()
            .and_then(|details| exact_input_partition(details, input, output))
            .unwrap_or_else(|| conservative_input_partition(input, cache_read, cache_write));
        if fresh_input
            .saturating_add(cache_read)
            .saturating_add(cache_write)
            .saturating_add(output)
            == 0
        {
            skipped += 1;
            continue;
        }

        events.push(UsageEvent {
            dedup_key: format!("copilot:{session_id}:{id}"),
            source: "copilot".to_string(),
            timestamp,
            model: Some(model),
            project: None,
            api_calls: 1,
            input_tokens: fresh_input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_write_5m_tokens: cache_write,
            cache_write_1h_tokens: 0,
            source_file: database.display().to_string(),
            session_id: Some(session_id),
            reasoning_tokens: Some(reasoning.min(output)),
            ctx: Default::default(),
        });
    }

    let inserted = match upsert_events_count(conn, &events) {
        Ok(inserted) => inserted,
        Err(error) => return failed(format!("upsert failed: {error}")),
    };
    let _ = set_file_state(conn, &database.to_string_lossy(), db_state);
    if wal_state.size != 0 || wal_state.mtime != 0 {
        let _ = set_file_state(conn, &wal.to_string_lossy(), wal_state);
    }
    SourceScanResult {
        events_inserted: inserted,
        lines_skipped: skipped,
        ..Default::default()
    }
}

fn failed(message: String) -> SourceScanResult {
    SourceScanResult {
        error: Some(format!("copilot: {message}")),
        ..Default::default()
    }
}

fn conservative_input_partition(input: i64, cache_read: i64, cache_write: i64) -> (i64, i64, i64) {
    let cache_read = cache_read.min(input);
    let cache_write = cache_write.min(input - cache_read);
    (input - cache_read - cache_write, cache_read, cache_write)
}

fn exact_input_partition(details: &str, input: i64, output: i64) -> Option<(i64, i64, i64)> {
    let values = serde_json::from_str::<serde_json::Value>(details)
        .ok()?
        .as_array()?
        .clone();
    let mut fresh = None;
    let mut cache_read = None;
    let mut cache_write = None;
    let mut detailed_output = None;
    for value in values {
        let kind = value.get("tokenType")?.as_str()?;
        let count = value.get("tokenCount")?.as_i64()?;
        if count < 0 {
            return None;
        }
        match kind {
            "input" => fresh = Some(count),
            "cache_read" => cache_read = Some(count),
            "cache_write" => cache_write = Some(count),
            "output" => detailed_output = Some(count),
            _ => return None,
        }
    }
    let (fresh, cache_read, cache_write, detailed_output) = (
        fresh.unwrap_or(0),
        cache_read.unwrap_or(0),
        cache_write.unwrap_or(0),
        detailed_output.unwrap_or(0),
    );
    (fresh.checked_add(cache_read)?.checked_add(cache_write)? == input && detailed_output == output)
        .then_some((fresh, cache_read, cache_write))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_db;

    #[test]
    fn schema_v6_is_normalized_once_and_other_versions_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("session-store.db");
        let source = Connection::open(&database).unwrap();
        source
            .execute_batch(
                "CREATE TABLE schema_version (version INTEGER NOT NULL);
                 INSERT INTO schema_version VALUES (6);
                 CREATE TABLE assistant_usage_events (
                    id INTEGER PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    model TEXT NOT NULL,
                    input_tokens INTEGER,
                    output_tokens INTEGER,
                    cache_read_tokens INTEGER,
                    cache_write_tokens INTEGER,
                    reasoning_tokens INTEGER,
                    token_details_json TEXT,
                    created_at TEXT
                 );
                 INSERT INTO assistant_usage_events VALUES
                    (1, 'session-1', 'copilot-model', 100, 20, 30, 10, 5,
                     '[{\"tokenType\":\"input\",\"tokenCount\":60},{\"tokenType\":\"cache_read\",\"tokenCount\":30},{\"tokenType\":\"cache_write\",\"tokenCount\":10},{\"tokenType\":\"output\",\"tokenCount\":20}]',
                     '2026-08-13T01:02:03Z');
                 INSERT INTO assistant_usage_events VALUES
                    (2, 'session-1', 'copilot-model', 12, 4, 20, 5, 9,
                     '[]', '2026-08-13T01:02:04Z');",
            )
            .unwrap();
        drop(source);

        let mut ledger = open_db(&directory.path().join("ledger.db")).unwrap();
        let first = scan_copilot(&mut ledger, &database);
        assert_eq!(first.events_inserted, 2);
        assert!(first.error.is_none());
        assert_eq!(
            ledger
                .query_row(
                    "SELECT input_tokens, output_tokens, cache_read_tokens,
                            cache_write_5m_tokens, reasoning_tokens
                     FROM events WHERE dedup_key = 'copilot:session-1:1'",
                    [],
                    |row| Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?
                    )),
                )
                .unwrap(),
            (60, 20, 30, 10, 5),
        );
        assert_eq!(
            ledger
                .query_row(
                    "SELECT input_tokens, cache_read_tokens, cache_write_5m_tokens, reasoning_tokens
                     FROM events WHERE dedup_key = 'copilot:session-1:2'",
                    [],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?)),
                )
                .unwrap(),
            (0, 12, 0, 4),
        );
        assert_eq!(scan_copilot(&mut ledger, &database).events_inserted, 0);

        Connection::open(&database)
            .unwrap()
            .execute_batch(
                "UPDATE schema_version SET version = 7;
                 CREATE TABLE schema_drift (id INTEGER);",
            )
            .unwrap();
        let unsupported = scan_copilot(&mut ledger, &database);
        assert!(unsupported
            .error
            .as_deref()
            .is_some_and(|error| error.contains("unsupported schema version 7")));
    }
}
