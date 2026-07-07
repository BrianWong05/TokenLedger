use crate::types::{FileState, UsageEvent};
use rusqlite::{params, Connection, OptionalExtension};

const SCHEMA: &str = "\
BEGIN;
CREATE TABLE IF NOT EXISTS events (
  dedup_key TEXT PRIMARY KEY,
  source TEXT NOT NULL,
  timestamp INTEGER NOT NULL,
  model TEXT NOT NULL,
  project TEXT,
  api_calls INTEGER NOT NULL DEFAULT 1,
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens INTEGER NOT NULL DEFAULT 0,
  cache_write_5m_tokens INTEGER NOT NULL DEFAULT 0,
  cache_write_1h_tokens INTEGER NOT NULL DEFAULT 0,
  source_file TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_events_ts ON events(timestamp);
CREATE INDEX IF NOT EXISTS idx_events_file ON events(source_file);
CREATE TABLE IF NOT EXISTS scanned_files (
  path TEXT PRIMARY KEY,
  size INTEGER,
  mtime INTEGER,
  byte_offset INTEGER
);
CREATE TABLE IF NOT EXISTS prices (
  model TEXT PRIMARY KEY,
  input_per_tok REAL,
  output_per_tok REAL,
  cache_read_per_tok REAL,
  cache_write_5m_per_tok REAL,
  cache_write_1h_per_tok REAL
);
CREATE TABLE IF NOT EXISTS price_overrides (
  model TEXT PRIMARY KEY,
  input_per_tok REAL,
  output_per_tok REAL,
  cache_read_per_tok REAL,
  cache_write_per_tok REAL
);
PRAGMA user_version = 1;
COMMIT;";

const INSERT_SQL: &str = "INSERT OR IGNORE INTO events \
(dedup_key, source, timestamp, model, project, api_calls, input_tokens, output_tokens, \
cache_read_tokens, cache_write_5m_tokens, cache_write_1h_tokens, source_file) \
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)";

const REPLACE_SQL: &str = "INSERT OR REPLACE INTO events \
(dedup_key, source, timestamp, model, project, api_calls, input_tokens, output_tokens, \
cache_read_tokens, cache_write_5m_tokens, cache_write_1h_tokens, source_file) \
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)";

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 1 {
        conn.execute_batch(SCHEMA)?;
    }
    Ok(())
}

pub fn open_db(path: &std::path::Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    // journal_mode returns the applied mode as a row, so read it via query_row.
    let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    conn.busy_timeout(std::time::Duration::from_millis(5000))?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn insert_events(conn: &mut Connection, events: &[UsageEvent]) -> rusqlite::Result<u64> {
    let tx = conn.transaction()?;
    let mut inserted = 0u64;
    {
        let mut stmt = tx.prepare(INSERT_SQL)?;
        for e in events {
            // execute() returns changes(): 1 when inserted, 0 when the key already exists.
            inserted += stmt.execute(params![
                e.dedup_key, e.source, e.timestamp, e.model, e.project, e.api_calls,
                e.input_tokens, e.output_tokens, e.cache_read_tokens,
                e.cache_write_5m_tokens, e.cache_write_1h_tokens, e.source_file
            ])? as u64;
        }
    }
    tx.commit()?;
    Ok(inserted)
}

/// Like insert_events but, on dedup_key conflict, keeps the row with the greater
/// output_tokens. Needed for Claude: one turn is logged as several content-block
/// lines sharing (message.id, requestId) with a growing output_tokens snapshot;
/// the final (largest) line carries the true count. Other fields are stable across
/// those lines, so overwriting them from the winning row is a no-op in practice.
pub fn insert_events_keep_max_output(
    conn: &mut Connection,
    events: &[UsageEvent],
) -> rusqlite::Result<u64> {
    let tx = conn.transaction()?;
    let mut n = 0u64;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO events (dedup_key, source, timestamp, model, project, api_calls, \
             input_tokens, output_tokens, cache_read_tokens, cache_write_5m_tokens, cache_write_1h_tokens, source_file) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12) \
             ON CONFLICT(dedup_key) DO UPDATE SET \
               input_tokens=excluded.input_tokens, output_tokens=excluded.output_tokens, \
               cache_read_tokens=excluded.cache_read_tokens, cache_write_5m_tokens=excluded.cache_write_5m_tokens, \
               cache_write_1h_tokens=excluded.cache_write_1h_tokens, project=excluded.project, \
               timestamp=excluded.timestamp, model=excluded.model, api_calls=excluded.api_calls, \
               source_file=excluded.source_file \
             WHERE excluded.output_tokens > events.output_tokens",
        )?;
        for e in events {
            n += stmt.execute(params![
                e.dedup_key, e.source, e.timestamp, e.model, e.project, e.api_calls,
                e.input_tokens, e.output_tokens, e.cache_read_tokens,
                e.cache_write_5m_tokens, e.cache_write_1h_tokens, e.source_file
            ])? as u64;
        }
    }
    tx.commit()?;
    Ok(n)
}

pub fn upsert_events(conn: &mut Connection, events: &[UsageEvent]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(REPLACE_SQL)?;
        for e in events {
            stmt.execute(params![
                e.dedup_key, e.source, e.timestamp, e.model, e.project, e.api_calls,
                e.input_tokens, e.output_tokens, e.cache_read_tokens,
                e.cache_write_5m_tokens, e.cache_write_1h_tokens, e.source_file
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn replace_file_events(
    conn: &mut Connection,
    source_file: &str,
    events: &[UsageEvent],
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM events WHERE source_file = ?1", [source_file])?;
    {
        let mut stmt = tx.prepare(INSERT_SQL)?;
        for e in events {
            stmt.execute(params![
                e.dedup_key, e.source, e.timestamp, e.model, e.project, e.api_calls,
                e.input_tokens, e.output_tokens, e.cache_read_tokens,
                e.cache_write_5m_tokens, e.cache_write_1h_tokens, e.source_file
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn get_file_state(conn: &Connection, path: &str) -> rusqlite::Result<Option<FileState>> {
    conn.query_row(
        "SELECT size, mtime, byte_offset FROM scanned_files WHERE path = ?1",
        [path],
        |row| {
            Ok(FileState {
                size: row.get(0)?,
                mtime: row.get(1)?,
                byte_offset: row.get(2)?,
            })
        },
    )
    .optional()
}

pub fn set_file_state(conn: &Connection, path: &str, state: FileState) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO scanned_files (path, size, mtime, byte_offset) \
         VALUES (?1, ?2, ?3, ?4)",
        params![path, state.size, state.mtime, state.byte_offset],
    )?;
    Ok(())
}

pub fn prune_missing_files(conn: &Connection) -> rusqlite::Result<u64> {
    let paths: Vec<String> = {
        let mut stmt = conn.prepare("SELECT path FROM scanned_files")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<String>>>()?
    };
    let mut removed = 0u64;
    for p in paths {
        if !std::path::Path::new(&p).exists() {
            conn.execute("DELETE FROM scanned_files WHERE path = ?1", [&p])?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event(dedup_key: &str, source_file: &str) -> UsageEvent {
        UsageEvent {
            dedup_key: dedup_key.to_string(),
            source: "claude".to_string(),
            timestamp: 1_700_000_000,
            model: "claude-sonnet-4-20250514".to_string(),
            project: Some("/Users/dev/projects/alpha".to_string()),
            api_calls: 1,
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 10,
            cache_write_5m_tokens: 5,
            cache_write_1h_tokens: 2,
            source_file: source_file.to_string(),
        }
    }

    fn temp_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db(&dir.path().join("test.db")).unwrap();
        (dir, conn)
    }

    #[test]
    fn fresh_db_has_tables_and_user_version() {
        let (_dir, conn) = temp_db();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 1);
        for table in ["events", "scanned_files", "prices", "price_overrides"] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "table {table} missing");
        }
    }

    #[test]
    fn open_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        {
            let mut conn = open_db(&path).unwrap();
            insert_events(&mut conn, &[sample_event("claude:a:1", "f1.jsonl")]).unwrap();
        }
        // Re-opening must migrate cleanly without wiping data.
        let conn = open_db(&path).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 1);
    }

    #[test]
    fn insert_ignores_duplicates_and_counts_inserted() {
        let (_dir, mut conn) = temp_db();
        let first = insert_events(
            &mut conn,
            &[
                sample_event("claude:a:1", "f1.jsonl"),
                sample_event("claude:b:1", "f1.jsonl"),
            ],
        )
        .unwrap();
        assert_eq!(first, 2);
        // One duplicate key + one new: only the new one counts.
        let second = insert_events(
            &mut conn,
            &[
                sample_event("claude:a:1", "f1.jsonl"),
                sample_event("claude:c:1", "f1.jsonl"),
            ],
        )
        .unwrap();
        assert_eq!(second, 1);
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 3);
    }

    #[test]
    fn upsert_replaces_existing_row() {
        let (_dir, mut conn) = temp_db();
        insert_events(&mut conn, &[sample_event("hermes:s1", "state.db")]).unwrap();
        let mut grown = sample_event("hermes:s1", "state.db");
        grown.output_tokens = 999;
        upsert_events(&mut conn, &[grown]).unwrap();
        let out: i64 = conn
            .query_row(
                "SELECT output_tokens FROM events WHERE dedup_key='hermes:s1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(out, 999);
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 1);
    }

    #[test]
    fn replace_file_events_is_scoped_to_source_file() {
        let (_dir, mut conn) = temp_db();
        insert_events(
            &mut conn,
            &[
                sample_event("gemini:s1:m1", "sessionA.json"),
                sample_event("gemini:s2:m1", "sessionB.json"),
            ],
        )
        .unwrap();
        // Replace only sessionA's events.
        replace_file_events(
            &mut conn,
            "sessionA.json",
            &[sample_event("gemini:s1:m2", "sessionA.json")],
        )
        .unwrap();
        let a_key: String = conn
            .query_row(
                "SELECT dedup_key FROM events WHERE source_file='sessionA.json'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(a_key, "gemini:s1:m2");
        let b: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE source_file='sessionB.json'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(b, 1);
    }

    #[test]
    fn file_state_roundtrip_and_overwrite() {
        let (_dir, conn) = temp_db();
        assert!(get_file_state(&conn, "f1.jsonl").unwrap().is_none());
        set_file_state(
            &conn,
            "f1.jsonl",
            FileState { size: 1024, mtime: 42, byte_offset: 512 },
        )
        .unwrap();
        let st = get_file_state(&conn, "f1.jsonl").unwrap().unwrap();
        assert_eq!((st.size, st.mtime, st.byte_offset), (1024, 42, 512));
        set_file_state(
            &conn,
            "f1.jsonl",
            FileState { size: 2048, mtime: 43, byte_offset: 1000 },
        )
        .unwrap();
        let st2 = get_file_state(&conn, "f1.jsonl").unwrap().unwrap();
        assert_eq!((st2.size, st2.mtime, st2.byte_offset), (2048, 43, 1000));
    }

    #[test]
    fn prune_removes_missing_files_but_never_events() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(&dir.path().join("test.db")).unwrap();

        let real = dir.path().join("real.jsonl");
        std::fs::write(&real, b"x").unwrap();
        let real_str = real.to_str().unwrap().to_string();

        set_file_state(&conn, &real_str, FileState { size: 1, mtime: 0, byte_offset: 0 }).unwrap();
        set_file_state(
            &conn,
            "/nonexistent/gone.jsonl",
            FileState { size: 1, mtime: 0, byte_offset: 0 },
        )
        .unwrap();

        // The ledger is permanent: an event referencing the missing file must survive.
        insert_events(&mut conn, &[sample_event("claude:x:1", "/nonexistent/gone.jsonl")]).unwrap();

        let removed = prune_missing_files(&conn).unwrap();
        assert_eq!(removed, 1);

        let files: i64 = conn
            .query_row("SELECT COUNT(*) FROM scanned_files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(files, 1);
        let events: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(events, 1);
    }
}
