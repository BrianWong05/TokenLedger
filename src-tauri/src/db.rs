use crate::types::{FileState, UsageEvent};
use rusqlite::{params, Connection, OptionalExtension};

// No BEGIN/COMMIT here: migrate() runs the batches inside its own
// BEGIN IMMEDIATE transaction.
const SCHEMA: &str = "\
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
PRAGMA user_version = 1;";

// After the SCHEMA const. Clearing scanned_files forces the next scan to
// re-parse every log so existing rows get session_id/reasoning backfilled
// via the ON CONFLICT clauses below. Events are never deleted (Ledger rule).
// No BEGIN/COMMIT here: migrate() runs the batches inside its own
// BEGIN IMMEDIATE transaction.
const SCHEMA_V2: &str = "\
ALTER TABLE events ADD COLUMN session_id TEXT;
ALTER TABLE events ADD COLUMN reasoning_tokens INTEGER;
DELETE FROM scanned_files;
PRAGMA user_version = 2;";

// v3: context attribution. ctx_* columns hold each event's attributed share
// of billed context (see types::CtxTokens). ctx_resources powers the panel's
// meta line (distinct names per local day). session_ctx persists Claude's
// running composition across byte-offset resumes. Clearing scanned_files
// forces a full re-scan so existing rows backfill ctx via the conflict
// clauses below. Events are never deleted (Ledger rule).
// No BEGIN/COMMIT here: migrate() runs the batches inside its own
// BEGIN IMMEDIATE transaction.
const SCHEMA_V3: &str = "\
ALTER TABLE events ADD COLUMN ctx_messages INTEGER;
ALTER TABLE events ADD COLUMN ctx_system INTEGER;
ALTER TABLE events ADD COLUMN ctx_reasoning INTEGER;
ALTER TABLE events ADD COLUMN ctx_toolcalls INTEGER;
ALTER TABLE events ADD COLUMN ctx_agents INTEGER;
ALTER TABLE events ADD COLUMN ctx_mcp INTEGER;
ALTER TABLE events ADD COLUMN ctx_skills INTEGER;
CREATE TABLE IF NOT EXISTS ctx_resources (
  source TEXT NOT NULL,
  kind TEXT NOT NULL,
  name TEXT NOT NULL,
  day TEXT NOT NULL,
  PRIMARY KEY (source, kind, name, day)
);
CREATE TABLE IF NOT EXISTS session_ctx (
  session_id TEXT PRIMARY KEY,
  msg_est INTEGER NOT NULL DEFAULT 0,
  tool_est INTEGER NOT NULL DEFAULT 0,
  mcp_est INTEGER NOT NULL DEFAULT 0,
  skill_est INTEGER NOT NULL DEFAULT 0,
  reas_est INTEGER NOT NULL DEFAULT 0,
  sys_est INTEGER NOT NULL DEFAULT 0,
  initialized INTEGER NOT NULL DEFAULT 0,
  tainted INTEGER NOT NULL DEFAULT 0
);
DELETE FROM scanned_files;
DELETE FROM session_ctx;
PRAGMA user_version = 3;";

// On dedup conflict, refresh only the append-only columns: token counts are
// immutable (Ledger), but a backfill re-scan must fill session_id/reasoning/ctx
// on old rows.
const INSERT_SQL: &str = "INSERT INTO events \
(dedup_key, source, timestamp, model, project, api_calls, input_tokens, output_tokens, \
cache_read_tokens, cache_write_5m_tokens, cache_write_1h_tokens, source_file, session_id, reasoning_tokens, \
ctx_messages, ctx_system, ctx_reasoning, ctx_toolcalls, ctx_agents, ctx_mcp, ctx_skills) \
VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21) \
ON CONFLICT(dedup_key) DO UPDATE SET \
  session_id = COALESCE(excluded.session_id, events.session_id), \
  reasoning_tokens = COALESCE(excluded.reasoning_tokens, events.reasoning_tokens), \
  ctx_messages  = COALESCE(excluded.ctx_messages,  events.ctx_messages), \
  ctx_system    = COALESCE(excluded.ctx_system,    events.ctx_system), \
  ctx_reasoning = COALESCE(excluded.ctx_reasoning, events.ctx_reasoning), \
  ctx_toolcalls = COALESCE(excluded.ctx_toolcalls, events.ctx_toolcalls), \
  ctx_agents    = COALESCE(excluded.ctx_agents,    events.ctx_agents), \
  ctx_mcp       = COALESCE(excluded.ctx_mcp,       events.ctx_mcp), \
  ctx_skills    = COALESCE(excluded.ctx_skills,    events.ctx_skills)";

const REPLACE_SQL: &str = "INSERT OR REPLACE INTO events \
(dedup_key, source, timestamp, model, project, api_calls, input_tokens, output_tokens, \
cache_read_tokens, cache_write_5m_tokens, cache_write_1h_tokens, source_file, session_id, reasoning_tokens, \
ctx_messages, ctx_system, ctx_reasoning, ctx_toolcalls, ctx_agents, ctx_mcp, ctx_skills) \
VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)";

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    // Serialize migrators and re-check the version INSIDE the write lock: two
    // connections opening a v1 DB at once must not both run the ALTERs (the
    // loser would die on "duplicate column"). BEGIN IMMEDIATE takes the write
    // lock up front (waiting via busy_timeout), so the second migrator sees
    // the committed user_version=2 and no-ops.
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let apply = || -> rusqlite::Result<()> {
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            conn.execute_batch(SCHEMA)?;
        }
        if version < 2 {
            conn.execute_batch(SCHEMA_V2)?;
        }
        if version < 3 {
            conn.execute_batch(SCHEMA_V3)?;
        }
        Ok(())
    };
    match apply() {
        Ok(()) => conn.execute_batch("COMMIT"),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
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
    let before: i64 = tx.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
    {
        let mut stmt = tx.prepare(INSERT_SQL)?;
        for e in events {
            stmt.execute(params![
                e.dedup_key, e.source, e.timestamp, e.model, e.project, e.api_calls,
                e.input_tokens, e.output_tokens, e.cache_read_tokens,
                e.cache_write_5m_tokens, e.cache_write_1h_tokens, e.source_file,
                e.session_id, e.reasoning_tokens,
                e.ctx.messages, e.ctx.system, e.ctx.reasoning,
                e.ctx.toolcalls, e.ctx.agents, e.ctx.mcp, e.ctx.skills
            ])?;
        }
    }
    let after: i64 = tx.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
    tx.commit()?;
    Ok((after - before).max(0) as u64)
}

/// Like insert_events but, on dedup_key conflict, keeps the row with the greater
/// output_tokens. Needed for Claude: one turn is logged as several content-block
/// lines sharing (message.id, requestId) with a growing output_tokens snapshot;
/// the final (largest) line carries the true count.
///
/// Tie semantics (equal output_tokens): only the v2 columns backfill, and only
/// where the stored value is NULL. A resumed/forked session copies the same
/// (message.id, requestId) line into a new file with a DIFFERENT sessionId —
/// the tie case must NOT re-attribute the row (session_id/source_file/project
/// stay first-writer-stable across re-scans), it only fills v2 NULLs left by
/// the pre-migration ledger.
pub fn insert_events_keep_max_output(
    conn: &mut Connection,
    events: &[UsageEvent],
) -> rusqlite::Result<u64> {
    let tx = conn.transaction()?;
    // Count distinct NEW rows: an upgrade-UPDATE (raising output_tokens) must not
    // inflate the inserted count, so diff COUNT(*) rather than summing changes().
    let before: i64 = tx.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
    {
        // UPDATE SET expressions all evaluate against the pre-update row, so the
        // repeated CASE guard reads the original output_tokens consistently.
        let mut stmt = tx.prepare(
            "INSERT INTO events (dedup_key, source, timestamp, model, project, api_calls, \
             input_tokens, output_tokens, cache_read_tokens, cache_write_5m_tokens, cache_write_1h_tokens, source_file, session_id, reasoning_tokens, \
             ctx_messages, ctx_system, ctx_reasoning, ctx_toolcalls, ctx_agents, ctx_mcp, ctx_skills) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21) \
             ON CONFLICT(dedup_key) DO UPDATE SET \
               source           = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.source           ELSE events.source           END, \
               input_tokens     = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.input_tokens     ELSE events.input_tokens     END, \
               cache_read_tokens= CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.cache_read_tokens ELSE events.cache_read_tokens END, \
               cache_write_5m_tokens = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.cache_write_5m_tokens ELSE events.cache_write_5m_tokens END, \
               cache_write_1h_tokens = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.cache_write_1h_tokens ELSE events.cache_write_1h_tokens END, \
               project          = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.project          ELSE events.project          END, \
               timestamp        = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.timestamp        ELSE events.timestamp        END, \
               model            = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.model            ELSE events.model            END, \
               api_calls        = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.api_calls        ELSE events.api_calls        END, \
               source_file      = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.source_file      ELSE events.source_file      END, \
               session_id       = COALESCE(events.session_id, excluded.session_id), \
               reasoning_tokens = COALESCE(events.reasoning_tokens, excluded.reasoning_tokens), \
               ctx_messages  = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_messages  ELSE COALESCE(events.ctx_messages,  excluded.ctx_messages)  END, \
               ctx_system    = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_system    ELSE COALESCE(events.ctx_system,    excluded.ctx_system)    END, \
               ctx_reasoning = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_reasoning ELSE COALESCE(events.ctx_reasoning, excluded.ctx_reasoning) END, \
               ctx_toolcalls = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_toolcalls ELSE COALESCE(events.ctx_toolcalls, excluded.ctx_toolcalls) END, \
               ctx_agents    = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_agents    ELSE COALESCE(events.ctx_agents,    excluded.ctx_agents)    END, \
               ctx_mcp       = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_mcp       ELSE COALESCE(events.ctx_mcp,       excluded.ctx_mcp)       END, \
               ctx_skills    = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_skills    ELSE COALESCE(events.ctx_skills,    excluded.ctx_skills)    END, \
               output_tokens    = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.output_tokens    ELSE events.output_tokens    END \
             WHERE excluded.output_tokens >= events.output_tokens",
        )?;
        for e in events {
            stmt.execute(params![
                e.dedup_key, e.source, e.timestamp, e.model, e.project, e.api_calls,
                e.input_tokens, e.output_tokens, e.cache_read_tokens,
                e.cache_write_5m_tokens, e.cache_write_1h_tokens, e.source_file,
                e.session_id, e.reasoning_tokens,
                e.ctx.messages, e.ctx.system, e.ctx.reasoning,
                e.ctx.toolcalls, e.ctx.agents, e.ctx.mcp, e.ctx.skills
            ])?;
        }
    }
    let after: i64 = tx.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
    tx.commit()?;
    Ok((after - before).max(0) as u64)
}

pub fn upsert_events(conn: &mut Connection, events: &[UsageEvent]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(REPLACE_SQL)?;
        for e in events {
            stmt.execute(params![
                e.dedup_key, e.source, e.timestamp, e.model, e.project, e.api_calls,
                e.input_tokens, e.output_tokens, e.cache_read_tokens,
                e.cache_write_5m_tokens, e.cache_write_1h_tokens, e.source_file,
                e.session_id, e.reasoning_tokens,
                e.ctx.messages, e.ctx.system, e.ctx.reasoning,
                e.ctx.toolcalls, e.ctx.agents, e.ctx.mcp, e.ctx.skills
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
                e.cache_write_5m_tokens, e.cache_write_1h_tokens, e.source_file,
                e.session_id, e.reasoning_tokens,
                e.ctx.messages, e.ctx.system, e.ctx.reasoning,
                e.ctx.toolcalls, e.ctx.agents, e.ctx.mcp, e.ctx.skills
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
            session_id: None,
            reasoning_tokens: None,
            ctx: Default::default(),
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
        assert_eq!(version, 3);
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
        assert_eq!(version, 3);
    }

    #[test]
    fn v1_db_migrates_to_v2_preserving_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        // Build a genuine v1 database by hand (SCHEMA still writes user_version 1),
        // then prove open_db chains v1->v2->v3 cleanly in one shot.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute(
                "INSERT INTO events (dedup_key, source, timestamp, model, project, api_calls, \
                 input_tokens, output_tokens, cache_read_tokens, cache_write_5m_tokens, \
                 cache_write_1h_tokens, source_file) \
                 VALUES ('claude:old:1','claude',1,'m',NULL,1,10,20,0,0,0,'f')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO scanned_files (path, size, mtime, byte_offset) VALUES ('f',1,1,1)",
                [],
            )
            .unwrap();
        }
        let conn = open_db(&path).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 3);
        // Old row intact, new columns NULL.
        let (input, sid, rt): (i64, Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT input_tokens, session_id, reasoning_tokens FROM events \
                 WHERE dedup_key='claude:old:1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(input, 10);
        assert_eq!(sid, None);
        assert_eq!(rt, None);
        // Scan state cleared so the next scan re-parses every log (backfill).
        let files: i64 = conn
            .query_row("SELECT COUNT(*) FROM scanned_files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(files, 0);
    }

    #[test]
    fn insert_backfills_new_columns_on_existing_rows() {
        let (_dir, mut conn) = temp_db();
        let mut old = sample_event("claude:a:1", "f1.jsonl");
        old.session_id = None;
        old.reasoning_tokens = None;
        insert_events(&mut conn, &[old]).unwrap();
        // A re-scan delivers the same event, now carrying the new fields.
        let mut new = sample_event("claude:a:1", "f1.jsonl");
        new.session_id = Some("sess-1".to_string());
        new.reasoning_tokens = Some(7);
        let inserted = insert_events(&mut conn, &[new]).unwrap();
        assert_eq!(inserted, 0, "backfill update is not a new event");
        let (sid, rt, out): (Option<String>, Option<i64>, i64) = conn
            .query_row(
                "SELECT session_id, reasoning_tokens, output_tokens FROM events \
                 WHERE dedup_key='claude:a:1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(sid, Some("sess-1".to_string()));
        assert_eq!(rt, Some(7));
        assert_eq!(out, 50, "token counts unchanged by backfill");
    }

    #[test]
    fn keep_max_backfills_session_on_equal_output() {
        let (_dir, mut conn) = temp_db();
        let mut old = sample_event("claude:x:1", "f1.jsonl");
        old.session_id = None;
        insert_events_keep_max_output(&mut conn, &[old]).unwrap();
        // Same output_tokens (50): the >= conflict clause must still backfill.
        let mut new = sample_event("claude:x:1", "f1.jsonl");
        new.session_id = Some("sess-x".to_string());
        let n = insert_events_keep_max_output(&mut conn, &[new]).unwrap();
        assert_eq!(n, 0);
        let sid: Option<String> = conn
            .query_row(
                "SELECT session_id FROM events WHERE dedup_key='claude:x:1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sid, Some("sess-x".to_string()));
    }

    #[test]
    fn keep_max_tie_does_not_reattribute() {
        let (_dir, mut conn) = temp_db();
        let mut first = sample_event("claude:dup:1", "fileA.jsonl");
        first.session_id = Some("sess-A".to_string());
        insert_events_keep_max_output(&mut conn, &[first]).unwrap();

        // A fork/resume copies the same line (equal output) into fileB with a
        // new per-line sessionId: the tie must keep first-writer attribution.
        let mut fork = sample_event("claude:dup:1", "fileB.jsonl");
        fork.session_id = Some("sess-B".to_string());
        let n = insert_events_keep_max_output(&mut conn, &[fork]).unwrap();
        assert_eq!(n, 0);
        let (sid, file): (Option<String>, String) = conn
            .query_row(
                "SELECT session_id, source_file FROM events WHERE dedup_key='claude:dup:1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(sid, Some("sess-A".to_string()), "tie keeps first-writer session");
        assert_eq!(file, "fileA.jsonl", "tie keeps first-writer file attribution");

        // A strictly greater output still wins the row (keep-max semantics).
        let mut bigger = sample_event("claude:dup:1", "fileB.jsonl");
        bigger.session_id = Some("sess-B".to_string());
        bigger.output_tokens = 999;
        insert_events_keep_max_output(&mut conn, &[bigger]).unwrap();
        let (out, file2): (i64, String) = conn
            .query_row(
                "SELECT output_tokens, source_file FROM events WHERE dedup_key='claude:dup:1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(out, 999);
        assert_eq!(file2, "fileB.jsonl");
    }

    #[test]
    fn v2_db_migrates_to_v3_preserving_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        // Build a genuine v2 database by hand.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute_batch(SCHEMA_V2).unwrap();
            conn.execute(
                "INSERT INTO events (dedup_key, source, timestamp, model, project, api_calls, \
                 input_tokens, output_tokens, cache_read_tokens, cache_write_5m_tokens, \
                 cache_write_1h_tokens, source_file, session_id, reasoning_tokens) \
                 VALUES ('claude:old:1','claude',1,'m',NULL,1,10,20,0,0,0,'f','s',NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO scanned_files (path, size, mtime, byte_offset) VALUES ('f',1,1,1)",
                [],
            )
            .unwrap();
        }
        let conn = open_db(&path).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 3);
        // Old row intact, ctx columns NULL.
        let (input, cm): (i64, Option<i64>) = conn
            .query_row(
                "SELECT input_tokens, ctx_messages FROM events WHERE dedup_key='claude:old:1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(input, 10);
        assert_eq!(cm, None);
        // Scan state cleared so the next scan re-parses every log (ctx backfill).
        let files: i64 = conn
            .query_row("SELECT COUNT(*) FROM scanned_files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(files, 0);
        // New tables exist.
        for table in ["ctx_resources", "session_ctx"] {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "table {table} missing");
        }
    }

    #[test]
    fn insert_backfills_ctx_on_existing_rows() {
        let (_dir, mut conn) = temp_db();
        insert_events(&mut conn, &[sample_event("claude:a:1", "f1.jsonl")]).unwrap();
        let mut new = sample_event("claude:a:1", "f1.jsonl");
        new.ctx.messages = Some(90);
        new.ctx.system = Some(15);
        new.ctx.reasoning = Some(10);
        let inserted = insert_events(&mut conn, &[new]).unwrap();
        assert_eq!(inserted, 0);
        let (cm, cs, cr): (Option<i64>, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT ctx_messages, ctx_system, ctx_reasoning FROM events WHERE dedup_key='claude:a:1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((cm, cs, cr), (Some(90), Some(15), Some(10)));
    }

    #[test]
    fn keep_max_ctx_follows_winner_and_backfills_on_tie() {
        let (_dir, mut conn) = temp_db();
        let mut first = sample_event("claude:k:1", "f1.jsonl");
        first.ctx.messages = Some(10);
        insert_events_keep_max_output(&mut conn, &[first]).unwrap();
        // Bigger output wins the row: its ctx values replace.
        let mut bigger = sample_event("claude:k:1", "f1.jsonl");
        bigger.output_tokens = 999;
        bigger.ctx.messages = Some(50);
        bigger.ctx.toolcalls = Some(5);
        insert_events_keep_max_output(&mut conn, &[bigger]).unwrap();
        let (cm, ct): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT ctx_messages, ctx_toolcalls FROM events WHERE dedup_key='claude:k:1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((cm, ct), (Some(50), Some(5)));
        // Tie (equal output): NULLs backfill, existing values stay.
        let mut tie = sample_event("claude:k:1", "f1.jsonl");
        tie.output_tokens = 999;
        tie.ctx.messages = Some(1); // must NOT overwrite 50
        tie.ctx.skills = Some(2);   // fills a NULL
        insert_events_keep_max_output(&mut conn, &[tie]).unwrap();
        let (cm2, cs2): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT ctx_messages, ctx_skills FROM events WHERE dedup_key='claude:k:1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((cm2, cs2), (Some(50), Some(2)));
    }

    #[test]
    fn concurrent_opens_of_v1_db_both_succeed() {
        // Two processes racing the v1->v3 migration: the loser of the
        // BEGIN IMMEDIATE lock must see user_version=3 and no-op, not die on
        // "duplicate column name".
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
        }
        let p1 = path.clone();
        let p2 = path.clone();
        let t1 = std::thread::spawn(move || open_db(&p1).map(|_| ()));
        let t2 = std::thread::spawn(move || open_db(&p2).map(|_| ()));
        t1.join().unwrap().expect("first open failed");
        t2.join().unwrap().expect("second open failed");
        let conn = Connection::open(&path).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 3);
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
