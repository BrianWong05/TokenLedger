use crate::adapters::ctx::Composition;
use crate::types::{FileState, UsageEvent};
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::LazyLock;

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

// v4: per-tool drill-down weights. Rows are keyed per (source_file, name,
// local day) so re-parses stay idempotent: any parse from byte 0 clears the
// file's rows first; byte-offset resumes add increments over fresh bytes
// only. ctx_tools is scan-state-derived (unlike events) and may be rebuilt.
// Clearing scan state forces the one-time full re-scan that populates
// history. No BEGIN/COMMIT here: migrate() wraps the batches.
const SCHEMA_V4: &str = "\
CREATE TABLE IF NOT EXISTS ctx_tools (
  source TEXT NOT NULL,
  source_file TEXT NOT NULL,
  name TEXT NOT NULL,
  day TEXT NOT NULL,
  est_tokens INTEGER NOT NULL DEFAULT 0,
  calls INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (source_file, name, day)
);
DELETE FROM scanned_files;
DELETE FROM session_ctx;
PRAGMA user_version = 4;";

// v5: Bash command-level facets. One row per (file, local day, classified
// command); kind/exe/cmd are computed at scan time by exec_class. Same
// idempotency contract as ctx_tools: parse-from-byte-0 clears the file's
// rows first; resumes add increments. Scan-state clear forces the one-time
// backfill re-scan. No BEGIN/COMMIT: migrate() wraps the batches.
// The `source` column is claude-only by design: codex logs shell commands as
// JSON arrays inside function_call payloads (no shell string for exec_class to
// classify), and the Overview renders exec facets only under the Bash node.
const SCHEMA_V5: &str = "\
CREATE TABLE IF NOT EXISTS ctx_exec (
  source TEXT NOT NULL,
  source_file TEXT NOT NULL,
  day TEXT NOT NULL,
  kind TEXT NOT NULL,
  exe TEXT NOT NULL,
  cmd TEXT NOT NULL,
  est_tokens INTEGER NOT NULL DEFAULT 0,
  calls INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (source_file, day, kind, exe, cmd)
);
DELETE FROM scanned_files;
DELETE FROM session_ctx;
PRAGMA user_version = 5;";

// v6: user settings. A single-row table (id pinned to 1) holding the app
// shell's five Settings plus the launch/first-run flags. No scan-state clear:
// this is user config with no backfill from logs — an empty table just means
// defaults (settings::get_settings). No BEGIN/COMMIT: migrate() wraps the batch.
const SCHEMA_V6: &str = "\
CREATE TABLE IF NOT EXISTS settings (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  theme TEXT NOT NULL,
  language TEXT NOT NULL,
  currency TEXT NOT NULL,
  usd_rate REAL NOT NULL,
  launch_at_login INTEGER NOT NULL,
  auto_check_updates INTEGER NOT NULL,
  first_run_done INTEGER NOT NULL
);
PRAGMA user_version = 6;";

// v7: Unattributed Usage. SQLite cannot drop a NOT NULL constraint in place,
// so rebuild only events with the same columns, defaults, and indexes while
// allowing Model to be NULL. Price tables stay untouched because they remain
// keyed exclusively by real Model identities (ADR-0008).
const SCHEMA_V7: &str = "\
ALTER TABLE events RENAME TO events_v6;
CREATE TABLE events (
  dedup_key TEXT PRIMARY KEY,
  source TEXT NOT NULL,
  timestamp INTEGER NOT NULL,
  model TEXT,
  project TEXT,
  api_calls INTEGER NOT NULL DEFAULT 1,
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens INTEGER NOT NULL DEFAULT 0,
  cache_write_5m_tokens INTEGER NOT NULL DEFAULT 0,
  cache_write_1h_tokens INTEGER NOT NULL DEFAULT 0,
  source_file TEXT NOT NULL,
  session_id TEXT,
  reasoning_tokens INTEGER,
  ctx_messages INTEGER,
  ctx_system INTEGER,
  ctx_reasoning INTEGER,
  ctx_toolcalls INTEGER,
  ctx_agents INTEGER,
  ctx_mcp INTEGER,
  ctx_skills INTEGER
);
INSERT INTO events (
  dedup_key, source, timestamp, model, project, api_calls,
  input_tokens, output_tokens, cache_read_tokens,
  cache_write_5m_tokens, cache_write_1h_tokens, source_file,
  session_id, reasoning_tokens, ctx_messages, ctx_system, ctx_reasoning,
  ctx_toolcalls, ctx_agents, ctx_mcp, ctx_skills
) SELECT
  dedup_key, source, timestamp, model, project, api_calls,
  input_tokens, output_tokens, cache_read_tokens,
  cache_write_5m_tokens, cache_write_1h_tokens, source_file,
  session_id, reasoning_tokens, ctx_messages, ctx_system, ctx_reasoning,
  ctx_toolcalls, ctx_agents, ctx_mcp, ctx_skills
FROM events_v6;
DROP TABLE events_v6;
CREATE INDEX idx_events_ts ON events(timestamp);
CREATE INDEX idx_events_file ON events(source_file);
PRAGMA user_version = 7;";

// v8: pi fork/clone tool-drill-down ownership. A pi entry's tool weights are
// booked once, to the first file that ingested it; a copy in a fork/clone file
// defers. Usage-bearing copies dedup via the events key, but a usage-less tool
// result has no event, so its owning file is persisted here — surviving the
// incremental skip of an unchanged original when a fork is discovered later.
// Scan-state is not cleared here: the pi parser-version bump forces the pi
// re-parse that backfills this table. No BEGIN/COMMIT: migrate() wraps the batch.
const SCHEMA_V8: &str = "\
CREATE TABLE IF NOT EXISTS pi_tool_owner (
  ident TEXT PRIMARY KEY,
  source_file TEXT NOT NULL
);
PRAGMA user_version = 8;";

// One row of Usage-Record column knowledge: the write grammar (column list,
// placeholders, params binder, and the three conflict bodies) is generated
// from COLS so a new column is added in exactly one place.
struct EventCol {
    name: &'static str,
    insert: InsertConflict,
    keep: KeepMax,
    keep_ord: u8, // emit position within the keep-max SET body (0 = not emitted)
}

// INSERT_SQL's ON CONFLICT: refresh only the append-only columns. Token counts
// are immutable (Ledger), but a backfill re-scan must fill session_id/reasoning/
// ctx on old rows, so those COALESCE(excluded, events).
enum InsertConflict {
    Immutable,             // never rewritten on conflict
    BackfillFromExcluded,  // COALESCE(excluded.x, events.x)
}

// insert_events_keep_max_output's ON CONFLICT. Tie semantics (equal
// output_tokens): only the first-writer columns backfill, and only where the
// stored value is NULL. A resumed/forked session copies the same
// (message.id, requestId) line into a new file with a DIFFERENT sessionId — the
// tie case must NOT re-attribute the row (session_id/source_file/project stay
// first-writer-stable across re-scans), it only fills v2 NULLs left by the
// pre-migration ledger. Hence FirstWriter's COALESCE(events, excluded) argument
// order is the OPPOSITE of BackfillFromExcluded's, and is load-bearing.
enum KeepMax {
    Skip,        // the conflict key; not in the SET body
    Winner,      // CASE: strictly-greater output_tokens takes excluded, else keeps events
    FirstWriter, // COALESCE(events.x, excluded.x): fill NULLs, never re-attribute
    Hybrid,      // winner takes excluded, else COALESCE(events.x, excluded.x)
}

// Canonical column order: matches the events table and every ?N placeholder.
const COLS: [EventCol; 21] = {
    use InsertConflict::*;
    use KeepMax::*;
    [
        EventCol { name: "dedup_key",             insert: Immutable,            keep: Skip,         keep_ord: 0 },
        EventCol { name: "source",                insert: Immutable,            keep: Winner,       keep_ord: 1 },
        EventCol { name: "timestamp",             insert: Immutable,            keep: Winner,       keep_ord: 7 },
        EventCol { name: "model",                 insert: Immutable,            keep: Winner,       keep_ord: 8 },
        EventCol { name: "project",               insert: Immutable,            keep: Winner,       keep_ord: 6 },
        EventCol { name: "api_calls",             insert: Immutable,            keep: Winner,       keep_ord: 9 },
        EventCol { name: "input_tokens",          insert: Immutable,            keep: Winner,       keep_ord: 2 },
        EventCol { name: "output_tokens",         insert: Immutable,            keep: Winner,       keep_ord: 20 },
        EventCol { name: "cache_read_tokens",     insert: Immutable,            keep: Winner,       keep_ord: 3 },
        EventCol { name: "cache_write_5m_tokens", insert: Immutable,            keep: Winner,       keep_ord: 4 },
        EventCol { name: "cache_write_1h_tokens", insert: Immutable,            keep: Winner,       keep_ord: 5 },
        EventCol { name: "source_file",           insert: Immutable,            keep: Winner,       keep_ord: 10 },
        EventCol { name: "session_id",            insert: BackfillFromExcluded, keep: FirstWriter,  keep_ord: 11 },
        EventCol { name: "reasoning_tokens",      insert: BackfillFromExcluded, keep: FirstWriter,  keep_ord: 12 },
        EventCol { name: "ctx_messages",          insert: BackfillFromExcluded, keep: Hybrid,       keep_ord: 13 },
        EventCol { name: "ctx_system",            insert: BackfillFromExcluded, keep: Hybrid,       keep_ord: 14 },
        EventCol { name: "ctx_reasoning",         insert: BackfillFromExcluded, keep: Hybrid,       keep_ord: 15 },
        EventCol { name: "ctx_toolcalls",         insert: BackfillFromExcluded, keep: Hybrid,       keep_ord: 16 },
        EventCol { name: "ctx_agents",            insert: BackfillFromExcluded, keep: Hybrid,       keep_ord: 17 },
        EventCol { name: "ctx_mcp",               insert: BackfillFromExcluded, keep: Hybrid,       keep_ord: 18 },
        EventCol { name: "ctx_skills",            insert: BackfillFromExcluded, keep: Hybrid,       keep_ord: 19 },
    ]
};

fn cols_csv() -> String {
    COLS.iter().map(|c| c.name).collect::<Vec<_>>().join(", ")
}

fn placeholders() -> String {
    (1..=COLS.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",")
}

// The alignment padding below reproduces the hand-formatted literals byte-for-
// byte (pinned by generates_byte_identical_sql). ctx_* columns align to width 13
// with a leading space before "="; the keep-max winner/first-writer block aligns
// to width 17 and jams "=" (no leading space) so the exactly-17-char
// cache_read_tokens has none. ponytail: cache_write_*_tokens overrun width 17,
// so they fall to a single space — the one irregularity worth naming.
fn insert_backfill_entry(c: &EventCol) -> String {
    let n = c.name;
    let w = if n.starts_with("ctx_") { 13 } else { n.len() };
    let g = " ".repeat(w - n.len() + 1);
    format!("{n}{g}= COALESCE(excluded.{n},{g}events.{n})")
}

fn keep_entry(c: &EventCol) -> String {
    let n = c.name;
    let len = n.len();
    match c.keep {
        KeepMax::Skip => unreachable!("Skip columns are filtered before emission"),
        KeepMax::Winner => {
            let eq = " ".repeat(if len <= 17 { 17 - len } else { 1 });
            let v = " ".repeat(if len < 17 { 17 - len } else { 1 });
            format!(
                "{n}{eq}= CASE WHEN excluded.output_tokens > events.output_tokens \
                 THEN excluded.{n}{v}ELSE events.{n}{v}END"
            )
        }
        KeepMax::FirstWriter => {
            let g = " ".repeat(17 - len);
            format!("{n}{g}= COALESCE(events.{n}, excluded.{n})")
        }
        KeepMax::Hybrid => {
            let g = " ".repeat(14 - len);
            format!(
                "{n}{g}= CASE WHEN excluded.output_tokens > events.output_tokens \
                 THEN excluded.{n}{g}ELSE COALESCE(events.{n},{g}excluded.{n}){g}END"
            )
        }
    }
}

static INSERT_SQL: LazyLock<String> = LazyLock::new(|| {
    let body = COLS
        .iter()
        .filter(|c| matches!(c.insert, InsertConflict::BackfillFromExcluded))
        .map(insert_backfill_entry)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "INSERT INTO events ({}) VALUES ({}) ON CONFLICT(dedup_key) DO UPDATE SET {}",
        cols_csv(),
        placeholders(),
        body
    )
});

static REPLACE_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "INSERT OR REPLACE INTO events ({}) VALUES ({})",
        cols_csv(),
        placeholders()
    )
});

// UPDATE SET expressions all evaluate against the pre-update row, so the
// repeated CASE guard reads the original output_tokens consistently.
static KEEP_MAX_SQL: LazyLock<String> = LazyLock::new(|| {
    let mut emitted: Vec<&EventCol> = COLS
        .iter()
        .filter(|c| !matches!(c.keep, KeepMax::Skip))
        .collect();
    emitted.sort_by_key(|c| c.keep_ord);
    let body = emitted
        .iter()
        .map(|c| keep_entry(c))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "INSERT INTO events ({}) VALUES ({}) ON CONFLICT(dedup_key) DO UPDATE SET {} \
         WHERE excluded.output_tokens >= events.output_tokens",
        cols_csv(),
        placeholders(),
        body
    )
});

// Binds a UsageEvent to ?1..?21 in COLS order. One binder for all three writes.
fn event_params(e: &UsageEvent) -> [&dyn rusqlite::ToSql; 21] {
    [
        &e.dedup_key, &e.source, &e.timestamp, &e.model, &e.project, &e.api_calls,
        &e.input_tokens, &e.output_tokens, &e.cache_read_tokens,
        &e.cache_write_5m_tokens, &e.cache_write_1h_tokens, &e.source_file,
        &e.session_id, &e.reasoning_tokens,
        &e.ctx.messages, &e.ctx.system, &e.ctx.reasoning,
        &e.ctx.toolcalls, &e.ctx.agents, &e.ctx.mcp, &e.ctx.skills,
    ]
}

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    // Serialize migrators and re-check the version INSIDE the write lock: two
    // connections opening a v1 DB at once must not both run the ALTERs (the
    // loser would die on "duplicate column"). BEGIN IMMEDIATE takes the write
    // lock up front (waiting via busy_timeout), so the second migrator sees
    // the committed user_version (currently 8) and no-ops.
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
        if version < 4 {
            conn.execute_batch(SCHEMA_V4)?;
        }
        if version < 5 {
            conn.execute_batch(SCHEMA_V5)?;
        }
        if version < 6 {
            conn.execute_batch(SCHEMA_V6)?;
        }
        if version < 7 {
            conn.execute_batch(SCHEMA_V7)?;
        }
        if version < 8 {
            conn.execute_batch(SCHEMA_V8)?;
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
    // busy_timeout FIRST: converting to WAL needs an exclusive lock, so with
    // the default timeout of 0 a concurrent opener would fail the pragma
    // instantly ("database is locked") instead of waiting out the race.
    conn.busy_timeout(std::time::Duration::from_millis(5000))?;
    // journal_mode returns the applied mode as a row, so read it via query_row.
    let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn insert_events(conn: &mut Connection, events: &[UsageEvent]) -> rusqlite::Result<u64> {
    let tx = conn.transaction()?;
    let before: i64 = tx.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
    {
        let mut stmt = tx.prepare(&INSERT_SQL)?;
        for e in events {
            stmt.execute(&event_params(e)[..])?;
        }
    }
    let after: i64 = tx.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
    tx.commit()?;
    Ok((after - before).max(0) as u64)
}

/// Like insert_events but, on dedup_key conflict, keeps the row with the greater
/// output_tokens. Needed for Claude: one turn is logged as several content-block
/// lines sharing (message.id, requestId) with a growing output_tokens snapshot;
/// the final (largest) line carries the true count. Conflict-column behavior and
/// tie semantics live on the KeepMax enum.
pub fn insert_events_keep_max_output(
    conn: &mut Connection,
    events: &[UsageEvent],
) -> rusqlite::Result<u64> {
    let tx = conn.transaction()?;
    // Count distinct NEW rows: an upgrade-UPDATE (raising output_tokens) must not
    // inflate the inserted count, so diff COUNT(*) rather than summing changes().
    let before: i64 = tx.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
    {
        let mut stmt = tx.prepare(&KEEP_MAX_SQL)?;
        for e in events {
            stmt.execute(&event_params(e)[..])?;
        }
    }
    let after: i64 = tx.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
    tx.commit()?;
    Ok((after - before).max(0) as u64)
}

pub fn upsert_events(conn: &mut Connection, events: &[UsageEvent]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(&REPLACE_SQL)?;
        for e in events {
            stmt.execute(&event_params(e)[..])?;
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
        let mut stmt = tx.prepare(&INSERT_SQL)?;
        for e in events {
            stmt.execute(&event_params(e)[..])?;
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

pub fn clear_ctx_tools_for_file(conn: &Connection, source_file: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM ctx_tools WHERE source_file = ?1", [source_file])?;
    Ok(())
}

/// Additive per-(file, name, local-day) upsert of tool weights.
/// Idempotency contract: callers clear the file's rows first whenever they
/// re-parse from byte 0; resumes append increments over fresh bytes only.
pub fn add_ctx_tool_rows(
    conn: &mut Connection,
    source: &str,
    source_file: &str,
    rows: &[(String, i64, i64, i64)], // (name, est_tokens, calls, epoch_ts)
) -> rusqlite::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO ctx_tools (source, source_file, name, day, est_tokens, calls) \
             VALUES (?1, ?2, ?3, strftime('%Y-%m-%d', ?4, 'unixepoch', 'localtime'), ?5, ?6) \
             ON CONFLICT(source_file, name, day) DO UPDATE SET \
               est_tokens = est_tokens + excluded.est_tokens, \
               calls = calls + excluded.calls",
        )?;
        for (name, est, calls, ts) in rows {
            stmt.execute(params![source, source_file, name, ts, est, calls])?;
        }
    }
    tx.commit()
}

/// identity → the source_file that first booked that pi entry's tool weights.
/// Lets a fork/clone copy defer its drill-down to the entry's origin even when
/// the origin's file is skipped as unchanged on a later scan.
pub fn load_pi_tool_owners(conn: &Connection) -> std::collections::HashMap<String, String> {
    let mut owner = std::collections::HashMap::new();
    let Ok(mut stmt) = conn.prepare("SELECT ident, source_file FROM pi_tool_owner") else {
        return owner;
    };
    if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) {
        for row in rows.flatten() {
            owner.insert(row.0, row.1);
        }
    }
    owner
}

/// First-writer-wins record of which file owns each pi entry's tool weights.
/// pi entries are append-only, so ownership never needs clearing; INSERT OR
/// IGNORE keeps the original (earliest-sorted file) as owner.
pub fn record_pi_tool_owners(
    conn: &mut Connection,
    rows: &[(String, String)], // (ident, source_file)
) -> rusqlite::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction()?;
    {
        let mut stmt =
            tx.prepare("INSERT OR IGNORE INTO pi_tool_owner (ident, source_file) VALUES (?1, ?2)")?;
        for (ident, source_file) in rows {
            stmt.execute(params![ident, source_file])?;
        }
    }
    tx.commit()
}

pub fn clear_ctx_exec_for_file(conn: &Connection, source_file: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM ctx_exec WHERE source_file = ?1", [source_file])?;
    Ok(())
}

/// Additive per-(file, day, kind, exe, cmd) upsert of exec weights. Same
/// idempotency contract as add_ctx_tool_rows: callers clear per file on any
/// parse from byte 0; resumes append increments over fresh bytes only.
pub fn add_ctx_exec_rows(
    conn: &mut Connection,
    source: &str,
    source_file: &str,
    rows: &[(String, String, String, i64, i64, i64)], // (kind, exe, cmd, est, calls, ts)
) -> rusqlite::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO ctx_exec (source, source_file, day, kind, exe, cmd, est_tokens, calls) \
             VALUES (?1, ?2, strftime('%Y-%m-%d', ?3, 'unixepoch', 'localtime'), ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT(source_file, day, kind, exe, cmd) DO UPDATE SET \
               est_tokens = est_tokens + excluded.est_tokens, \
               calls = calls + excluded.calls",
        )?;
        for (kind, exe, cmd, est, calls, ts) in rows {
            stmt.execute(params![source, source_file, ts, kind, exe, cmd, est, calls])?;
        }
    }
    tx.commit()
}

// Composition persistence lives here (not in adapters::ctx) so the pure math
// stays rusqlite-free. The Claude adapter's running composition must survive
// byte-offset resumes between scans.
pub fn load_composition(conn: &Connection, session_id: &str) -> rusqlite::Result<Option<Composition>> {
    conn.query_row(
        "SELECT msg_est, tool_est, mcp_est, skill_est, reas_est, sys_est, initialized, tainted \
         FROM session_ctx WHERE session_id = ?1",
        [session_id],
        |r| {
            Ok(Composition {
                msg: r.get(0)?, tool: r.get(1)?, mcp: r.get(2)?, skill: r.get(3)?,
                reas: r.get(4)?, sys: r.get(5)?,
                initialized: r.get::<_, i64>(6)? != 0,
                tainted: r.get::<_, i64>(7)? != 0,
            })
        },
    )
    .optional()
}

pub fn save_composition(conn: &Connection, session_id: &str, c: &Composition) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO session_ctx \
         (session_id, msg_est, tool_est, mcp_est, skill_est, reas_est, sys_est, initialized, tainted) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![session_id, c.msg, c.tool, c.mcp, c.skill, c.reas, c.sys,
                c.initialized as i64, c.tainted as i64],
    )?;
    Ok(())
}

pub fn record_resources(
    conn: &Connection,
    source: &str,
    rows: &[(&'static str, String, i64)],
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(
        "INSERT OR IGNORE INTO ctx_resources (source, kind, name, day) \
         VALUES (?1, ?2, ?3, strftime('%Y-%m-%d', ?4, 'unixepoch', 'localtime'))",
    )?;
    for (kind, name, ts) in rows {
        stmt.execute(params![source, kind, name, ts])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // GOLDEN: the three write statements are the spec of the write grammar. These
    // literals were captured verbatim from the pre-refactor constants; the
    // generator must reproduce them byte-for-byte (spacing and alignment included).
    #[test]
    fn generates_byte_identical_sql() {
        assert_eq!(
            INSERT_SQL.as_str(),
            "INSERT INTO events (dedup_key, source, timestamp, model, project, api_calls, input_tokens, output_tokens, cache_read_tokens, cache_write_5m_tokens, cache_write_1h_tokens, source_file, session_id, reasoning_tokens, ctx_messages, ctx_system, ctx_reasoning, ctx_toolcalls, ctx_agents, ctx_mcp, ctx_skills) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21) ON CONFLICT(dedup_key) DO UPDATE SET session_id = COALESCE(excluded.session_id, events.session_id), reasoning_tokens = COALESCE(excluded.reasoning_tokens, events.reasoning_tokens), ctx_messages  = COALESCE(excluded.ctx_messages,  events.ctx_messages), ctx_system    = COALESCE(excluded.ctx_system,    events.ctx_system), ctx_reasoning = COALESCE(excluded.ctx_reasoning, events.ctx_reasoning), ctx_toolcalls = COALESCE(excluded.ctx_toolcalls, events.ctx_toolcalls), ctx_agents    = COALESCE(excluded.ctx_agents,    events.ctx_agents), ctx_mcp       = COALESCE(excluded.ctx_mcp,       events.ctx_mcp), ctx_skills    = COALESCE(excluded.ctx_skills,    events.ctx_skills)"
        );
        assert_eq!(
            REPLACE_SQL.as_str(),
            "INSERT OR REPLACE INTO events (dedup_key, source, timestamp, model, project, api_calls, input_tokens, output_tokens, cache_read_tokens, cache_write_5m_tokens, cache_write_1h_tokens, source_file, session_id, reasoning_tokens, ctx_messages, ctx_system, ctx_reasoning, ctx_toolcalls, ctx_agents, ctx_mcp, ctx_skills) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)"
        );
        assert_eq!(
            KEEP_MAX_SQL.as_str(),
            "INSERT INTO events (dedup_key, source, timestamp, model, project, api_calls, input_tokens, output_tokens, cache_read_tokens, cache_write_5m_tokens, cache_write_1h_tokens, source_file, session_id, reasoning_tokens, ctx_messages, ctx_system, ctx_reasoning, ctx_toolcalls, ctx_agents, ctx_mcp, ctx_skills) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21) ON CONFLICT(dedup_key) DO UPDATE SET source           = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.source           ELSE events.source           END, input_tokens     = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.input_tokens     ELSE events.input_tokens     END, cache_read_tokens= CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.cache_read_tokens ELSE events.cache_read_tokens END, cache_write_5m_tokens = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.cache_write_5m_tokens ELSE events.cache_write_5m_tokens END, cache_write_1h_tokens = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.cache_write_1h_tokens ELSE events.cache_write_1h_tokens END, project          = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.project          ELSE events.project          END, timestamp        = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.timestamp        ELSE events.timestamp        END, model            = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.model            ELSE events.model            END, api_calls        = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.api_calls        ELSE events.api_calls        END, source_file      = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.source_file      ELSE events.source_file      END, session_id       = COALESCE(events.session_id, excluded.session_id), reasoning_tokens = COALESCE(events.reasoning_tokens, excluded.reasoning_tokens), ctx_messages  = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_messages  ELSE COALESCE(events.ctx_messages,  excluded.ctx_messages)  END, ctx_system    = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_system    ELSE COALESCE(events.ctx_system,    excluded.ctx_system)    END, ctx_reasoning = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_reasoning ELSE COALESCE(events.ctx_reasoning, excluded.ctx_reasoning) END, ctx_toolcalls = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_toolcalls ELSE COALESCE(events.ctx_toolcalls, excluded.ctx_toolcalls) END, ctx_agents    = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_agents    ELSE COALESCE(events.ctx_agents,    excluded.ctx_agents)    END, ctx_mcp       = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_mcp       ELSE COALESCE(events.ctx_mcp,       excluded.ctx_mcp)       END, ctx_skills    = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_skills    ELSE COALESCE(events.ctx_skills,    excluded.ctx_skills)    END, output_tokens    = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.output_tokens    ELSE events.output_tokens    END WHERE excluded.output_tokens >= events.output_tokens"
        );
    }

    fn sample_event(dedup_key: &str, source_file: &str) -> UsageEvent {
        UsageEvent {
            dedup_key: dedup_key.to_string(),
            source: "claude".to_string(),
            timestamp: 1_700_000_000,
            model: Some("claude-sonnet-4-20250514".to_string()),
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
        assert_eq!(version, 8);
        for table in ["events", "scanned_files", "prices", "price_overrides", "ctx_tools", "ctx_exec", "settings", "pi_tool_owner"] {
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
        assert_eq!(version, 8);
    }

    #[test]
    fn insert_accepts_unattributed_usage_without_a_sentinel_model() {
        let (_dir, mut conn) = temp_db();
        let mut usage = sample_event("pi:tool-result:1", "pi.jsonl");
        usage.source = "pi".to_string();
        usage.model = None;

        let inserted = insert_events(&mut conn, &[usage]).unwrap();
        assert_eq!(inserted, 1);
        let stored: (Option<String>, i64, i64) = conn.query_row(
            "SELECT model, input_tokens, api_calls FROM events WHERE dedup_key = 'pi:tool-result:1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).unwrap();
        assert_eq!(stored, (None, 100, 1));
    }

    #[test]
    fn v6_db_migrates_to_nullable_models_without_losing_ledger_or_prices() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute_batch(SCHEMA_V2).unwrap();
            conn.execute_batch(SCHEMA_V3).unwrap();
            conn.execute_batch(SCHEMA_V4).unwrap();
            conn.execute_batch(SCHEMA_V5).unwrap();
            conn.execute_batch(SCHEMA_V6).unwrap();
            conn.execute(
                "INSERT INTO events (dedup_key, source, timestamp, model, project, api_calls, \
                 input_tokens, output_tokens, cache_read_tokens, cache_write_5m_tokens, \
                 cache_write_1h_tokens, source_file, session_id) \
                 VALUES ('claude:old:1','claude',123,'claude-existing','/p',2,10,20,3,4,5,'f','s')",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO prices (model, input_per_tok, output_per_tok) \
                 VALUES ('claude-existing', 0.000001, 0.000002)",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO price_overrides (model, input_per_tok, output_per_tok) \
                 VALUES ('claude-existing', 0.000009, 0.000010)",
                [],
            ).unwrap();
        }

        let conn = open_db(&path).unwrap();
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, 8);
        let model_not_null: i64 = conn.query_row(
            "SELECT [notnull] FROM pragma_table_info('events') WHERE name = 'model'",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(model_not_null, 0, "Model storage must accept SQL NULL");
        let usage: (String, String, i64, String, Option<String>, Option<String>) = conn.query_row(
            "SELECT dedup_key, source, timestamp, model, project, session_id FROM events",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        ).unwrap();
        assert_eq!(usage, (
            "claude:old:1".to_string(), "claude".to_string(), 123,
            "claude-existing".to_string(), Some("/p".to_string()), Some("s".to_string()),
        ));
        let usage_totals: (i64, i64, i64, i64, i64, i64) = conn.query_row(
            "SELECT api_calls, input_tokens, output_tokens, cache_read_tokens, \
             cache_write_5m_tokens, cache_write_1h_tokens FROM events",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        ).unwrap();
        assert_eq!(usage_totals, (2, 10, 20, 3, 4, 5));
        let price: (Option<f64>, Option<f64>) = conn.query_row(
            "SELECT input_per_tok, output_per_tok FROM prices WHERE model = 'claude-existing'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).unwrap();
        assert_eq!(price, (Some(0.000001), Some(0.000002)));
        let override_rates: (Option<f64>, Option<f64>) = conn.query_row(
            "SELECT input_per_tok, output_per_tok FROM price_overrides WHERE model = 'claude-existing'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).unwrap();
        assert_eq!(override_rates, (Some(0.000009), Some(0.000010)));
        drop(conn);

        let conn = open_db(&path).unwrap();
        let usage_count: i64 = conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0)).unwrap();
        assert_eq!(usage_count, 1, "reopening after migration is idempotent");
    }

    #[test]
    fn v1_db_migrates_to_v2_preserving_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        // Build a genuine v1 database by hand (SCHEMA still writes user_version 1),
        // then prove open_db chains every migration through v8 in one shot.
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
        assert_eq!(v, 8);
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
        assert_eq!(v, 8);
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
        // Two processes racing the v1->v7 migration: the loser of the
        // BEGIN IMMEDIATE lock must see user_version=7 and no-op, not die on
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
        assert_eq!(v, 8);
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

    #[test]
    fn v3_db_migrates_to_v4_with_ctx_tools() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        // Build a genuine v3 database by hand.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute_batch(SCHEMA_V2).unwrap();
            conn.execute_batch(SCHEMA_V3).unwrap();
            conn.execute(
                "INSERT INTO scanned_files (path, size, mtime, byte_offset) VALUES ('f',1,1,1)",
                [],
            )
            .unwrap();
        }
        let conn = open_db(&path).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 8);
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='ctx_tools'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        // Scan state cleared: the one-time full re-scan populates ctx_tools history.
        let files: i64 = conn
            .query_row("SELECT COUNT(*) FROM scanned_files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(files, 0);
    }

    #[test]
    fn ctx_tool_rows_accumulate_and_clear_per_file() {
        let (_dir, mut conn) = temp_db();
        let ts = 1_782_907_200i64;
        add_ctx_tool_rows(&mut conn, "claude", "f1.jsonl", &[
            ("Bash".to_string(), 100, 1, ts),
            ("Bash".to_string(), 50, 0, ts + 60), // same local day: accumulates
            ("Read".to_string(), 30, 1, ts),
        ]).unwrap();
        add_ctx_tool_rows(&mut conn, "claude", "f2.jsonl", &[
            ("Bash".to_string(), 7, 1, ts),
        ]).unwrap();
        let (est, calls): (i64, i64) = conn
            .query_row(
                "SELECT est_tokens, calls FROM ctx_tools WHERE source_file='f1.jsonl' AND name='Bash'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((est, calls), (150, 1));
        // Clearing one file leaves the other's rows.
        clear_ctx_tools_for_file(&conn, "f1.jsonl").unwrap();
        let left: i64 = conn.query_row("SELECT COUNT(*) FROM ctx_tools", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 1);
        let src: String = conn
            .query_row("SELECT source_file FROM ctx_tools", [], |r| r.get(0))
            .unwrap();
        assert_eq!(src, "f2.jsonl");
    }

    #[test]
    fn pi_tool_owner_is_first_writer_wins_and_roundtrips() {
        let (_dir, mut conn) = temp_db();
        assert!(load_pi_tool_owners(&conn).is_empty());
        record_pi_tool_owners(&mut conn, &[
            ("a:1".to_string(), "orig.jsonl".to_string()),
            ("b:2".to_string(), "orig.jsonl".to_string()),
        ]).unwrap();
        // A later copy from a fork file must not steal ownership of a:1.
        record_pi_tool_owners(&mut conn, &[
            ("a:1".to_string(), "fork.jsonl".to_string()),
            ("c:3".to_string(), "fork.jsonl".to_string()),
        ]).unwrap();
        let owners = load_pi_tool_owners(&conn);
        assert_eq!(owners.get("a:1").map(String::as_str), Some("orig.jsonl"));
        assert_eq!(owners.get("b:2").map(String::as_str), Some("orig.jsonl"));
        assert_eq!(owners.get("c:3").map(String::as_str), Some("fork.jsonl"));
        assert_eq!(owners.len(), 3);
    }

    #[test]
    fn v4_db_migrates_to_v5_with_ctx_exec() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        // Build a genuine v4 database by hand.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute_batch(SCHEMA_V2).unwrap();
            conn.execute_batch(SCHEMA_V3).unwrap();
            conn.execute_batch(SCHEMA_V4).unwrap();
            conn.execute(
                "INSERT INTO scanned_files (path, size, mtime, byte_offset) VALUES ('f',1,1,1)",
                [],
            )
            .unwrap();
        }
        let conn = open_db(&path).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 8);
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='ctx_exec'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        let files: i64 = conn
            .query_row("SELECT COUNT(*) FROM scanned_files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(files, 0, "scan state cleared for the one-time backfill re-scan");
    }

    #[test]
    fn v5_db_migrates_to_v6_with_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        // Build a genuine v5 database by hand, plus one event: the v6 migration
        // only adds the settings table and must not clear scan state or touch
        // the Ledger (settings needs no backfill).
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute_batch(SCHEMA_V2).unwrap();
            conn.execute_batch(SCHEMA_V3).unwrap();
            conn.execute_batch(SCHEMA_V4).unwrap();
            conn.execute_batch(SCHEMA_V5).unwrap();
            conn.execute(
                "INSERT INTO scanned_files (path, size, mtime, byte_offset) VALUES ('f',1,1,1)",
                [],
            )
            .unwrap();
        }
        let conn = open_db(&path).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 8);
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='settings'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        // Scan state preserved: v6 is not a backfill migration.
        let files: i64 = conn
            .query_row("SELECT COUNT(*) FROM scanned_files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(files, 1, "settings migration must not clear scan state");
    }

    #[test]
    fn ctx_exec_rows_accumulate_and_clear_per_file() {
        let (_dir, mut conn) = temp_db();
        let ts = 1_782_907_200i64;
        add_ctx_exec_rows(&mut conn, "claude", "f1.jsonl", &[
            ("git_local".into(), "git".into(), "git add".into(), 100, 1, ts),
            ("git_local".into(), "git".into(), "git add".into(), 40, 0, ts + 60),
            ("test".into(), "npm".into(), "npm test".into(), 30, 1, ts),
        ]).unwrap();
        add_ctx_exec_rows(&mut conn, "claude", "f2.jsonl", &[
            ("git_local".into(), "git".into(), "git add".into(), 7, 1, ts),
        ]).unwrap();
        let (est, calls): (i64, i64) = conn
            .query_row(
                "SELECT est_tokens, calls FROM ctx_exec WHERE source_file='f1.jsonl' AND cmd='git add'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((est, calls), (140, 1), "same day+key accumulates; result adds size only");
        clear_ctx_exec_for_file(&conn, "f1.jsonl").unwrap();
        let left: i64 = conn.query_row("SELECT COUNT(*) FROM ctx_exec", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 1);
    }

    #[test]
    fn composition_roundtrips_through_db() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_db(&dir.path().join("t.db")).unwrap();
        assert!(load_composition(&conn, "s1").unwrap().is_none());
        let c = Composition { msg: 1, tool: 2, mcp: 3, skill: 4, reas: 5, sys: 6, initialized: true, tainted: true };
        save_composition(&conn, "s1", &c).unwrap();
        assert_eq!(load_composition(&conn, "s1").unwrap(), Some(c));
    }

    #[test]
    fn record_resources_dedupes_per_day() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_db(&dir.path().join("t.db")).unwrap();
        let ts = 1_782_907_200i64; // some day
        record_resources(&conn, "claude", &[
            ("skill", "graphify".to_string(), ts),
            ("skill", "graphify".to_string(), ts + 60), // same local day → deduped
            ("mcp_server", "pencil".to_string(), ts),
        ]).unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM ctx_resources", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
    }
}
