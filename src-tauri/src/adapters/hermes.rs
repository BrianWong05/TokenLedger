// TokenLedger — Hermes adapter.
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use super::{file_state_of, unchanged};
use crate::db::{set_file_state, upsert_events};
use crate::types::{SourceScanResult, UsageEvent};

pub fn scan_hermes(conn: &mut Connection, hermes_db: &Path) -> SourceScanResult {
    // Whole-DB skip when neither the main file nor its WAL moved since the
    // last scan; without this every scan re-upserts every session, which
    // keeps the ledger churning (and the frontend reloading) forever. A
    // missing DB never skips — it must fall through to report the open error.
    let db_state = file_state_of(hermes_db);
    let wal_path = hermes_db.with_extension("db-wal");
    let wal_state = file_state_of(&wal_path);
    let db_exists = db_state.size != 0 || db_state.mtime != 0;
    if db_exists && unchanged(conn, hermes_db, &db_state) && unchanged(conn, &wal_path, &wal_state) {
        return SourceScanResult::default();
    }

    // Open the Hermes ledger read-only so we never lock out its live writer.
    let uri = format!("file:{}?mode=ro", hermes_db.display());
    let ro = match Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    ) {
        Ok(c) => c,
        Err(e) => {
            // Lock/open failure: keep prior events, report staleness.
            return SourceScanResult {
                error: Some(format!("hermes: open failed: {e}")),
                ..Default::default()
            };
        }
    };
    let _ = ro.busy_timeout(Duration::from_millis(5000));

    let mut stmt = match ro.prepare(
        "SELECT id, model, started_at, \
                input_tokens, output_tokens, cache_read_tokens, \
                cache_write_tokens, reasoning_tokens, api_call_count, cwd \
         FROM sessions",
    ) {
        Ok(s) => s,
        Err(e) => {
            return SourceScanResult {
                error: Some(format!("hermes: query failed: {e}")),
                ..Default::default()
            };
        }
    };

    let rows = match stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,                   // id
            r.get::<_, Option<String>>(1)?,           // model
            r.get::<_, f64>(2)?,                       // started_at (REAL epoch secs)
            r.get::<_, Option<i64>>(3)?.unwrap_or(0),  // input_tokens
            r.get::<_, Option<i64>>(4)?.unwrap_or(0),  // output_tokens
            r.get::<_, Option<i64>>(5)?.unwrap_or(0),  // cache_read_tokens
            r.get::<_, Option<i64>>(6)?.unwrap_or(0),  // cache_write_tokens
            r.get::<_, Option<i64>>(7)?.unwrap_or(0),  // reasoning_tokens
            r.get::<_, Option<i64>>(8)?.unwrap_or(0),  // api_call_count
            r.get::<_, Option<String>>(9)?,           // cwd
        ))
    }) {
        Ok(r) => r,
        Err(e) => {
            return SourceScanResult {
                error: Some(format!("hermes: read failed: {e}")),
                ..Default::default()
            };
        }
    };

    let mut events: Vec<UsageEvent> = Vec::new();
    let mut skipped: u64 = 0;
    for row in rows {
        let (id, model, started_at, input, output, cache_read, cache_write, reasoning, api_call_count, cwd) =
            match row {
                Ok(v) => v,
                Err(_) => {
                    skipped += 1;
                    continue;
                }
            };

        let total = input + output + cache_read + cache_write + reasoning;
        if total <= 0 && api_call_count <= 0 {
            skipped += 1; // no tokens and no API calls — nothing to record
            continue;
        }

        // api_call_count is authoritative; force at least 1 when tokens exist.
        let api_calls = if api_call_count > 0 { api_call_count } else { 1 };

        let project = match cwd {
            Some(p) if !p.is_empty() => Some(p),
            _ => None,
        };

        events.push(UsageEvent {
            dedup_key: format!("hermes:{id}"),
            source: "hermes".to_string(),
            timestamp: started_at as i64,          // truncate fractional seconds
            model: model.unwrap_or_else(|| "unknown".to_string()),
            project,
            api_calls,
            input_tokens: input,
            output_tokens: output + reasoning,      // reasoning folds into output
            cache_read_tokens: cache_read,
            cache_write_5m_tokens: cache_write,     // single Hermes bucket -> 5m
            cache_write_1h_tokens: 0,
            source_file: hermes_db.display().to_string(),
            session_id: Some(id.clone()),
            reasoning_tokens: Some(reasoning),
            ctx: Default::default(),
        });
    }

    let inserted = events.len() as u64;
    if let Err(e) = upsert_events(conn, &events) {
        return SourceScanResult {
            error: Some(format!("hermes: upsert failed: {e}")),
            ..Default::default()
        };
    }

    let _ = set_file_state(conn, &hermes_db.to_string_lossy(), db_state);
    if wal_state.size != 0 || wal_state.mtime != 0 {
        let _ = set_file_state(conn, &wal_path.to_string_lossy(), wal_state);
    }

    SourceScanResult { events_inserted: inserted, lines_skipped: skipped, error: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_db;
    use rusqlite::Connection;
    use std::path::Path;
    use tempfile::tempdir;

    /// Advance a file's mtime by 2s so an in-place same-second rewrite is
    /// visible to the size+mtime skip (real scans are 30s apart).
    fn bump_mtime(path: &Path) {
        let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        let m = f.metadata().unwrap().modified().unwrap();
        f.set_modified(m + std::time::Duration::from_secs(2)).unwrap();
    }

    /// Build a minimal Hermes-schema sqlite DB (subset of columns the adapter reads).
    fn build_hermes_db(path: &Path) {
        let src = Connection::open(path).unwrap();
        src.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                model TEXT,
                started_at REAL NOT NULL,
                input_tokens INTEGER,
                output_tokens INTEGER,
                cache_read_tokens INTEGER,
                cache_write_tokens INTEGER,
                reasoning_tokens INTEGER,
                api_call_count INTEGER,
                cwd TEXT
            );",
        )
        .unwrap();
        // s1: full row — reasoning + cache_write + cwd populated, fractional started_at.
        src.execute(
            "INSERT INTO sessions VALUES
             ('s1','qwen3.6-35b',1780287300.21103,64728,5088,1394761,100,50,30,'/Users/dev/projects/alpha')",
            [],
        )
        .unwrap();
        // s2: all-zero tokens and zero api calls -> skipped.
        src.execute(
            "INSERT INTO sessions VALUES
             ('s2','qwen3.6-35b',1780289247.5,0,0,0,0,0,0,'')",
            [],
        )
        .unwrap();
        // s3: tokens>0 but api_call_count 0 -> api_calls forced to 1; empty cwd -> project NULL.
        src.execute(
            "INSERT INTO sessions VALUES
             ('s3','qwen-35b',1780310783.7583,905075,8094,0,0,0,0,'')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn extracts_and_normalizes_sessions() {
        let hermes_dir = tempdir().unwrap();
        let hermes_db = hermes_dir.path().join("state.db");
        build_hermes_db(&hermes_db);

        let app_dir = tempdir().unwrap();
        let mut conn = open_db(&app_dir.path().join("tokenledger.db")).unwrap();

        let res = scan_hermes(&mut conn, &hermes_db);
        assert!(res.error.is_none(), "unexpected error: {:?}", res.error);
        assert_eq!(res.events_inserted, 2); // s1 + s3; s2 skipped
        assert_eq!(res.lines_skipped, 1);   // s2

        // s1: reasoning folded into output, cache_write -> 5m bucket, ts truncated, cwd kept.
        let (model, ts, input, output, cr, cw5, cw1, calls, project): (
            String, i64, i64, i64, i64, i64, i64, i64, Option<String>,
        ) = conn
            .query_row(
                "SELECT model, timestamp, input_tokens, output_tokens, cache_read_tokens,
                        cache_write_5m_tokens, cache_write_1h_tokens, api_calls, project
                 FROM events WHERE dedup_key = 'hermes:s1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?,
                        r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?)),
            )
            .unwrap();
        assert_eq!(model, "qwen3.6-35b");
        assert_eq!(ts, 1780287300);        // 1780287300.21103 truncated to whole seconds
        assert_eq!(input, 64728);
        assert_eq!(output, 5088 + 50);     // reasoning_tokens folded into output
        assert_eq!(cr, 1394761);
        assert_eq!(cw5, 100);              // cache_write_tokens -> 5m bucket
        assert_eq!(cw1, 0);
        assert_eq!(calls, 30);             // api_call_count verbatim
        assert_eq!(project, Some("/Users/dev/projects/alpha".to_string()));

        // v2 columns.
        let (sid, rt): (Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT session_id, reasoning_tokens FROM events WHERE dedup_key = 'hermes:s1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(sid, Some("s1".to_string()));
        assert_eq!(rt, Some(50));

        // s3: api_calls forced to 1; empty cwd -> NULL project.
        let (calls3, project3): (i64, Option<String>) = conn
            .query_row(
                "SELECT api_calls, project FROM events WHERE dedup_key = 'hermes:s3'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(calls3, 1);
        assert_eq!(project3, None);
    }

    #[test]
    fn hermes_ctx_is_all_null() {
        let hermes_dir = tempdir().unwrap();
        let hermes_db = hermes_dir.path().join("state.db");
        build_hermes_db(&hermes_db);

        let app_dir = tempdir().unwrap();
        let mut conn = open_db(&app_dir.path().join("tokenledger.db")).unwrap();

        let res = scan_hermes(&mut conn, &hermes_db);
        assert!(res.error.is_none());
        assert_eq!(res.events_inserted, 2); // s1 + s3 scanned

        let nulls: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE source='hermes' AND (\
                 ctx_messages IS NOT NULL OR ctx_system IS NOT NULL OR \
                 ctx_reasoning IS NOT NULL OR ctx_toolcalls IS NOT NULL OR \
                 ctx_agents IS NOT NULL OR ctx_mcp IS NOT NULL OR ctx_skills IS NOT NULL)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nulls, 0, "hermes logs record no content: everything NULL");
    }

    #[test]
    fn missing_db_reports_error_keeps_events() {
        let app_dir = tempdir().unwrap();
        let mut conn = open_db(&app_dir.path().join("tokenledger.db")).unwrap();

        let res = scan_hermes(&mut conn, Path::new("/nonexistent/hermes/state.db"));
        assert!(res.error.is_some());
        assert_eq!(res.events_inserted, 0);
    }

    #[test]
    fn unchanged_db_skips_rescan() {
        let hermes_dir = tempdir().unwrap();
        let hermes_db = hermes_dir.path().join("state.db");
        build_hermes_db(&hermes_db);

        let app_dir = tempdir().unwrap();
        let mut conn = open_db(&app_dir.path().join("tokenledger.db")).unwrap();

        let r1 = scan_hermes(&mut conn, &hermes_db);
        assert_eq!(r1.events_inserted, 2);

        // Untouched DB → whole scan skipped: no phantom re-upserts.
        let r2 = scan_hermes(&mut conn, &hermes_db);
        assert!(r2.error.is_none());
        assert_eq!(r2.events_inserted, 0);
        assert_eq!(r2.lines_skipped, 0);
    }

    #[test]
    fn upsert_grows_live_rows() {
        let hermes_dir = tempdir().unwrap();
        let hermes_db = hermes_dir.path().join("state.db");
        build_hermes_db(&hermes_db);

        let app_dir = tempdir().unwrap();
        let mut conn = open_db(&app_dir.path().join("tokenledger.db")).unwrap();

        scan_hermes(&mut conn, &hermes_db);

        // Simulate a live session growing: s1 gains output tokens.
        {
            let src = Connection::open(&hermes_db).unwrap();
            src.execute("UPDATE sessions SET output_tokens = 9000 WHERE id = 's1'", [])
                .unwrap();
        }
        bump_mtime(&hermes_db); // same-second in-place update: advance past mtime granularity

        let res = scan_hermes(&mut conn, &hermes_db);
        assert!(res.error.is_none());

        let output: i64 = conn
            .query_row(
                "SELECT output_tokens FROM events WHERE dedup_key = 'hermes:s1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(output, 9000 + 50); // upsert replaced the row; reasoning still folded in
    }
}
