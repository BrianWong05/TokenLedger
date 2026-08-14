use crate::adapters::ctx::Composition;
use crate::types::{FileState, LimitReading, ModelScope, UsageEvent};
use chrono::{DateTime, Local, LocalResult, TimeZone, Utc};
use rusqlite::functions::FunctionFlags;
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::LazyLock;

/// The user_version an opened database ends at — the last SCHEMA_Vn applied.
/// Each SCHEMA_Vn keeps its own literal `PRAGMA user_version = n`, because a
/// migration stamps the version it introduces forever; this is only what the
/// tests assert a fully-migrated database reaches, so adding SCHEMA_V18 means
/// bumping one line here instead of every migration test.
#[cfg(test)]
const CURRENT_USER_VERSION: i64 = 18;

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

// v9: record which catalog supplied each List Price, so the origin reported to
// the Pricing tab is stored data rather than a hardcoded constant (ADR-0009).
// Existing rows leave it NULL and RateMap reads that as "litellm" — every
// pre-v9 row came from the one catalog then read. Scan-state is not cleared:
// prices are rebuilt from the catalog at every start, independent of the Ledger.
// No BEGIN/COMMIT: migrate() wraps the batch.
const SCHEMA_V9: &str = "\
ALTER TABLE prices ADD COLUMN catalog TEXT;
PRAGMA user_version = 9;";

// v10: Source Session metadata. The Ledger persists Usage Records forever, but
// a Source may prune its own transcripts (WorkBuddy does), taking with them
// every trace of Sessions never scanned while the file existed. This table
// records those Sessions' metadata (cwd, model, timestamps, title) so the
// Source's history outlives its log retention — never token figures, which
// only a Usage Record may carry (ADR-0016). Written idempotently per scan.
// Scan-state is not cleared: rows are upserted from the Source's own metadata
// store, independent of the Ledger's events. No BEGIN/COMMIT: migrate() wraps.
const SCHEMA_V10: &str = "\
CREATE TABLE IF NOT EXISTS source_sessions (
  source TEXT NOT NULL,
  session_id TEXT NOT NULL,
  cwd TEXT,
  model TEXT,
  title TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (source, session_id)
);
PRAGMA user_version = 10;";

// Unreadable Artifacts per Source (ADR-0017): count + latest mtime, upserted
// by every scan for every scanned Source. Persisted (not held in memory) so
// the ≥ floor marker is honest from launch, before the first scan of a run —
// metadata only, never content, per ADR-0011. No BEGIN/COMMIT: migrate() wraps.
const SCHEMA_V11: &str = "\
CREATE TABLE IF NOT EXISTS unreadable_artifacts (
  source TEXT PRIMARY KEY,
  count INTEGER NOT NULL,
  max_mtime INTEGER
);
PRAGMA user_version = 11;";

// v12: per-skill weights. A skill's instructions are re-injected on every
// invocation, so `uses` counts injections and est_tokens sums them — the same
// scan-state-derived, rebuildable contract as ctx_tools (parse-from-byte-0
// clears the file's rows; resumes append increments). Clearing scan state
// forces the one-time full re-scan that populates history; without it the
// panel would under-report every transcript scanned before this version.
const SCHEMA_V12: &str = "\
CREATE TABLE IF NOT EXISTS ctx_skills_usage (
  source TEXT NOT NULL,
  source_file TEXT NOT NULL,
  name TEXT NOT NULL,
  day TEXT NOT NULL,
  est_tokens INTEGER NOT NULL DEFAULT 0,
  uses INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (source_file, name, day)
);
DELETE FROM scanned_files;
DELETE FROM session_ctx;
PRAGMA user_version = 12;";

// v13: ctx_buckets finds each session's first cache write via this partial
// index instead of ROW_NUMBER-ranking the whole events table on every call —
// that full-table sort was the dominant cost of an Overview range switch.
// The query's WHERE repeats the predicate verbatim so the planner can prove
// the index applies.
const SCHEMA_V13: &str = "\
CREATE INDEX IF NOT EXISTS idx_events_first_cw
  ON events(source, session_id, timestamp, dedup_key)
  WHERE cache_write_5m_tokens + cache_write_1h_tokens > 0 AND session_id IS NOT NULL;
PRAGMA user_version = 13;";

// v14: Limit Readings — one observation of one Limit, the rolling window with a
// ceiling that a Source's vendor imposes on the subscription (CONTEXT.md). They hold no tokens, so they live beside the Ledger in their own
// table rather than in `events`. The PRIMARY KEY is the reading's *content*
// (#108), so with INSERT OR IGNORE an epoch stores its whole fill-curve at
// integer resolution while re-scans, per-request repeats at an unchanged
// percentage, and fork replays all collapse onto the row already there.
// `used_pct` keeps the vendor's own figure unconverted. `window_key` is opaque
// TEXT — Claude's response key, Codex's `w{canonical minutes}`, and one day
// perhaps a pool prefix — never parsed for structure.
// (V18 widened that key with `observed_at` and rebuilt the table: a repeat at an
// unchanged percentage is now its own Reading, because evidence needs it as an
// anchor. What stands below is v14's shape, not today's.)
// Clearing scanned_files is the established backfill pattern (V2-V5, V12): the
// next scan re-parses every log once and Codex limit history back-fills
// retroactively. Ingest is idempotent by the PK, so re-running the migration
// (or running it from an intermediate dev build) can never consume the backfill.
// No BEGIN/COMMIT here: migrate() runs the batches inside its own
// BEGIN IMMEDIATE transaction.
const SCHEMA_V14: &str = "\
CREATE TABLE IF NOT EXISTS limit_readings (
  source         TEXT NOT NULL,
  window_key     TEXT NOT NULL,
  window_minutes INTEGER,
  used_pct       REAL NOT NULL,
  resets_at      INTEGER NOT NULL,
  observed_at    INTEGER NOT NULL,
  via            TEXT NOT NULL,
  plan           TEXT,
  PRIMARY KEY (source, window_key, resets_at, used_pct)
);
DELETE FROM scanned_files;
PRAGMA user_version = 14;";

// v15: daily resolved-price snapshots for Codex Auto Review. The live `prices`
// table still drives today's Pricing tab, while this tiny history keeps a past
// day's Cost fixed after that day closes. `priced = 0` records a real Unpriced
// observation, so the first later priced snapshot can backfill it once and then
// remain stable. No scan-state clear: rates are independent of Source Artifacts.
const SCHEMA_V15: &str = "\
CREATE TABLE IF NOT EXISTS model_price_history (
  model                  TEXT NOT NULL,
  day                    TEXT NOT NULL,
  priced                 INTEGER NOT NULL,
  input_per_tok          REAL NOT NULL DEFAULT 0,
  output_per_tok         REAL NOT NULL DEFAULT 0,
  cache_read_per_tok     REAL NOT NULL DEFAULT 0,
  cache_write_5m_per_tok REAL NOT NULL DEFAULT 0,
  cache_write_1h_per_tok REAL NOT NULL DEFAULT 0,
  PRIMARY KEY (model, day)
);
PRAGMA user_version = 15;";

// v16: physical aliases could make one Source Artifact contribute multiple
// path-keyed Context rows. They are derived scan state, so discard and rebuild
// them with pi's ownership map after discovery starts collapsing aliases by
// file identity. The Ledger is permanent and remains untouched.
const SCHEMA_V16: &str = "\
DELETE FROM ctx_tools;
DELETE FROM ctx_exec;
DELETE FROM ctx_skills_usage;
DELETE FROM pi_tool_owner;
DELETE FROM scanned_files;
DELETE FROM session_ctx;
PRAGMA user_version = 16;";

// v17: Qoder previously persisted its internal `dfmodel` routing alias in the
// Ledger. Those source rows may already have aged out of Qoder's database, so
// a normal re-scan cannot re-translate them. Rewrite only Qoder's historical
// alias; the Usage Records and their token figures remain intact.
const SCHEMA_V17: &str = "\
UPDATE events
SET model = 'deepseek-v4-flash'
WHERE source = 'qoder' AND model = 'dfmodel';
PRAGMA user_version = 17;";

// v18: the provenance a Limit Reading needs before it can be *evidence* for a
// Limit Token Estimate (spec: "Evidence participation contract", "Migration and
// backfill"). Two changes, both additive to what the Limits page already draws.
//
// 1. A Reading's identity widens to the exact source observation it records.
//    V14 keyed it by content — (source, window_key, resets_at, used_pct) —
//    which collapsed every later Reading at an unchanged percentage onto the row
//    already there, including one a gap later that the estimator needs as its
//    next anchor. `observed_at` joins the key: it comes from the Artifact, not
//    the clock, so re-ingesting one source observation still lands on the stored
//    row, while a genuinely later Reading at an equal percentage keeps its own.
//    SQLite cannot widen a key in place, so the table is rebuilt (the V7 idiom:
//    rename aside, create the canonical name, copy, drop) and every row copied.
// 2. Seven nullable provenance columns carry the facts the contract requires
//    beyond the figures: account identity, metering regime, Limit identity,
//    Model scope, source order, completeness coverage, and external activity.
//    Nothing repeats what a Reading already stores — `source`, `plan` and
//    `resets_at` are the Source, plan identity and reset epoch themselves.
//    ponytail: the provenance stays OUT of the key. A nullable column in a
//    rowid table's key would defeat the dedup it is meant to sharpen, because
//    SQLite counts every NULL as distinct — so until a Source proves an account,
//    each re-ingestion would insert again. The ceiling: two Readings alike in
//    all five key columns but differing in a proven identity fact cannot both be
//    stored; the second is dropped rather than merged (insert_limit_readings
//    refuses to blend contradicting identities). Key the identity too (with ''
//    for unknown, or a COALESCE unique index) if that ever stops being a corner.
//
// Legacy rows keep unknown provenance, and NULL is unknown rather than a
// wildcard, so no stored history can accidentally match estimator evidence.
// scanned_files is deliberately NOT cleared, against the pattern of V2-V5/V12/
// V14: re-parsing today's Artifacts cannot prove an old interval's account or
// completeness (spec hard rule), so Claude and Codex cold-start instead.
// limit_readings gains no index of its own: the widened key indexes what the
// table is read by today — the display query's (source, window_key, resets_at),
// and now Reading order within an epoch. It is not the Series key, which leads
// with account, meter and Limit identity rather than `window_key`, so if the
// evidence query wants that order it brings its own index, once a Source proves
// those facts and there is a query to measure. events gains the one Usage
// membership needs, by Source, account and time with the raw Model along for
// scoped filtering. No estimate, materialisation, or history table: an estimate
// is derived (ADR-0024).
//
// The migration comes in two halves, and both are v18's DDL: the ADD COLUMNs
// below cannot live in the batch because SQLite has no ADD COLUMN IF NOT EXISTS
// and the rebuild copies the new columns by name, so they must exist before it
// runs and a second pass must find them rather than fail on a duplicate. That is
// what makes the whole migration re-runnable, the rule V14 set: a second pass
// copies an already-migrated table onto itself with every column, provenance
// included, so an intermediate dev build cannot eat evidence.
const SCHEMA_V18_COLUMNS: [(&str, &str, &str); 8] = [
    ("limit_readings", "account_id", "TEXT"),
    ("limit_readings", "metering_regime", "TEXT"),
    ("limit_readings", "limit_id", "TEXT"),
    ("limit_readings", "model_scope", "TEXT"),
    ("limit_readings", "source_order", "INTEGER"),
    ("limit_readings", "covered_from", "INTEGER"),
    ("limit_readings", "external_activity", "TEXT"),
    // Deliberately outside COLS, which is otherwise the one place an events
    // column is declared (see COLS below): nothing derives an account identity
    // yet, and the shape of Usage-capture provenance is the per-Source tickets'
    // to settle. Whoever first writes this MUST add it to COLS — REPLACE_SQL has
    // no slot for a column COLS does not know, so replace_file_events would drop
    // the account identity of every fork or replay it rewrites.
    ("events", "account_id", "TEXT"),
];

const SCHEMA_V18: &str = "\
ALTER TABLE limit_readings RENAME TO limit_readings_v17;
CREATE TABLE limit_readings (
  source            TEXT NOT NULL,
  window_key        TEXT NOT NULL,
  window_minutes    INTEGER,
  used_pct          REAL NOT NULL,
  resets_at         INTEGER NOT NULL,
  observed_at       INTEGER NOT NULL,
  via               TEXT NOT NULL,
  plan              TEXT,
  account_id        TEXT,
  metering_regime   TEXT,
  limit_id          TEXT,
  model_scope       TEXT,
  source_order      INTEGER,
  covered_from      INTEGER,
  external_activity TEXT,
  PRIMARY KEY (source, window_key, resets_at, observed_at, used_pct)
);
INSERT INTO limit_readings
  (source, window_key, window_minutes, used_pct, resets_at, observed_at, via, plan,
   account_id, metering_regime, limit_id, model_scope, source_order, covered_from,
   external_activity)
  SELECT source, window_key, window_minutes, used_pct, resets_at, observed_at, via, plan,
         account_id, metering_regime, limit_id, model_scope, source_order, covered_from,
         external_activity
  FROM limit_readings_v17;
DROP TABLE limit_readings_v17;
CREATE INDEX IF NOT EXISTS idx_events_evidence
  ON events(source, account_id, timestamp, model);
PRAGMA user_version = 18;";

/// `ALTER TABLE ... ADD COLUMN` with the `IF NOT EXISTS` SQLite has no syntax
/// for, so a migration that carries new columns stays re-runnable.
fn add_column(conn: &Connection, table: &str, column: &str, decl: &str) -> rusqlite::Result<()> {
    let present: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
        params![table, column],
        |r| r.get(0),
    )?;
    if !present {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"))?;
    }
    Ok(())
}

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
// Latest is the one tie exception: a parser-version re-parse re-inserts every
// row with equal output, and the fix it carries (e.g. a corrected Model name)
// must REPLACE the stored value or the bump would change nothing for existing
// rows. Model is derived from the Request's own content, so a forked copy
// carries the same value and last-writer-wins cannot re-attribute anything.
enum KeepMax {
    Skip,        // the conflict key; not in the SET body
    Winner,      // CASE: strictly-greater output_tokens takes excluded, else keeps events
    FirstWriter, // COALESCE(events.x, excluded.x): fill NULLs, never re-attribute
    Hybrid,      // winner takes excluded, else COALESCE(events.x, excluded.x)
    Latest,      // always excluded: the newest write has the better answer
}

// Canonical column order: matches the events table and every ?N placeholder.
const COLS: [EventCol; 21] = {
    use InsertConflict::*;
    use KeepMax::*;
    [
        EventCol { name: "dedup_key",             insert: Immutable,            keep: Skip,         keep_ord: 0 },
        EventCol { name: "source",                insert: Immutable,            keep: Winner,       keep_ord: 1 },
        EventCol { name: "timestamp",             insert: Immutable,            keep: Winner,       keep_ord: 7 },
        EventCol { name: "model",                 insert: Immutable,            keep: Latest,       keep_ord: 8 },
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
        KeepMax::Latest => {
            let g = " ".repeat(17 - len);
            format!("{n}{g}= excluded.{n}")
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
    // the committed user_version — whatever the latest SCHEMA_Vn stamped — and
    // no-ops.
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
        if version < 9 {
            conn.execute_batch(SCHEMA_V9)?;
        }
        if version < 10 {
            conn.execute_batch(SCHEMA_V10)?;
        }
        if version < 11 {
            conn.execute_batch(SCHEMA_V11)?;
        }
        if version < 12 {
            conn.execute_batch(SCHEMA_V12)?;
        }
        if version < 13 {
            conn.execute_batch(SCHEMA_V13)?;
        }
        if version < 14 {
            conn.execute_batch(SCHEMA_V14)?;
        }
        if version < 15 {
            conn.execute_batch(SCHEMA_V15)?;
        }
        if version < 16 {
            conn.execute_batch(SCHEMA_V16)?;
        }
        if version < 17 {
            conn.execute_batch(SCHEMA_V17)?;
        }
        if version < 18 {
            for (table, column, decl) in SCHEMA_V18_COLUMNS {
                add_column(conn, table, column, decl)?;
            }
            conn.execute_batch(SCHEMA_V18)?;
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
    // busy_timeout FIRST: every lock wait below rides on it — the migration's
    // BEGIN IMMEDIATE waits out a concurrent opener instead of failing the
    // instant it finds the database held.
    conn.busy_timeout(std::time::Duration::from_millis(5000))?;
    set_wal(&conn)?;
    migrate(&conn)?;
    register_query_functions(&conn)?;
    Ok(conn)
}

/// SQLite's built-in `strftime(..., 'localtime')` crosses into the operating
/// system for every input row on macOS. Overview series run it twice, turning
/// an 80k-record Ledger into seconds of blocking work. Chrono loads the local
/// IANA zone once per thread, then resolves offsets in memory. Keeping the
/// function inside SQLite preserves the existing GROUP BY semantics, and the
/// fallbacks below preserve strftime's guarantee of never yielding NULL.
pub(crate) fn register_query_functions(conn: &Connection) -> rusqlite::Result<()> {
    conn.create_scalar_function(
        "tokenledger_local_bucket",
        2,
        FunctionFlags::SQLITE_UTF8,
        |ctx| {
            let timestamp = ctx.get::<i64>(0)?;
            let hourly = ctx.get::<bool>(1)?;
            // Instant -> local wall clock is total for real zones, but degrade
            // rather than drop the row: take the earlier reading when the zone
            // is ambiguous, and fall back to UTC when it cannot map the instant
            // at all.
            let local = match Local.timestamp_opt(timestamp, 0) {
                LocalResult::Single(dt) => dt,
                LocalResult::Ambiguous(dt, _) => dt,
                // Rendering an instant in UTC equals any zone's rendering when
                // that zone cannot map it; equivalence is all we owe here.
                LocalResult::None => {
                    let utc = Utc
                        .timestamp_opt(timestamp, 0)
                        .single()
                        .expect("UTC maps every instant");
                    DateTime::from(utc)
                }
            };
            let fmt = if hourly { "%Y-%m-%d %H:00" } else { "%Y-%m-%d" };
            Ok(local.format(fmt).to_string())
        },
    )
}

/// Switches the journal to WAL, retrying while another opener is converting.
/// The busy timeout above cannot cover this one: converting needs an exclusive
/// lock while the connection already holds a shared one, and when a second
/// opener wants that same upgrade SQLite calls it a deadlock — answering BUSY
/// at once and never running the busy handler, so waiting is left to the
/// caller. The retry costs nothing once the winner lands: the loser then finds
/// the journal already WAL and the pragma is a no-op.
fn set_wal(conn: &Connection) -> rusqlite::Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(5000);
    loop {
        // journal_mode returns the applied mode as a row, so read it via query_row.
        match conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get::<_, String>(0)) {
            Ok(_) => return Ok(()),
            Err(e)
                if std::time::Instant::now() < deadline
                    && matches!(&e, rusqlite::Error::SqliteFailure(f, _)
                        if f.code == rusqlite::ErrorCode::DatabaseBusy) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Append Limit Readings. The key is the exact source observation — the
/// reading's content plus `observed_at` — so a re-scan, a fork replay of the same
/// block, and a Companion export read twice all land on the stored row, while a
/// later Reading at an unchanged percentage stays a Reading of its own. Returns
/// the number of Readings written, new or genuinely revised: the "relevant
/// Reading facts changed" signal an evaluation triggers on (ADR-0024).
///
/// Provenance and plan fill in on conflict, under two rules. A pass may fill an
/// unknown identity fact or confirm a known one, never contradict it: readings
/// alike in all five key columns but disagreeing about account, meter, Limit,
/// scope, or the order they were read in are two different Readings that this
/// table cannot hold apart, so the second is dropped whole rather than blended
/// into the first (see SCHEMA_V18's ponytail note). Source order joins that rule
/// rather than the newest-proof one because an order that moves under a Reading
/// between scans orders nothing. Coverage, external activity, and plan take the
/// newest proof,
/// which is how a later unreadable-Artifact discovery withdraws coverage — a
/// pass that proves nothing still erases nothing. The Reading's own figures are
/// never rewritten; they are what the identity is made of. `window_minutes` and
/// `via` are neither keyed nor revised: the first writer's stand, as in V14.
///
/// The `WHERE` keeps the conflict path as free as V14's `INSERT OR IGNORE` was —
/// it fires only when merging actually changes the row, so re-reading a whole
/// rollout or credit log, which every scan of a live session does, costs the
/// reads and no writes.
pub fn insert_limit_readings(
    conn: &mut Connection,
    readings: &[LimitReading],
) -> rusqlite::Result<u64> {
    let tx = conn.transaction()?;
    let mut written = 0u64;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO limit_readings \
             (source, window_key, window_minutes, used_pct, resets_at, observed_at, via, plan, \
              account_id, metering_regime, limit_id, model_scope, source_order, covered_from, \
              external_activity) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15) \
             ON CONFLICT(source, window_key, resets_at, observed_at, used_pct) DO UPDATE SET \
               plan            = COALESCE(excluded.plan,            limit_readings.plan), \
               account_id      = COALESCE(excluded.account_id,      limit_readings.account_id), \
               metering_regime = COALESCE(excluded.metering_regime, \
                                          limit_readings.metering_regime), \
               limit_id        = COALESCE(excluded.limit_id,        limit_readings.limit_id), \
               model_scope     = COALESCE(excluded.model_scope,     limit_readings.model_scope), \
               source_order    = COALESCE(excluded.source_order,    limit_readings.source_order), \
               covered_from      = COALESCE(excluded.covered_from, \
                                            limit_readings.covered_from), \
               external_activity = COALESCE(excluded.external_activity, \
                                            limit_readings.external_activity) \
             WHERE (COALESCE(excluded.plan,              limit_readings.plan), \
                    COALESCE(excluded.account_id,        limit_readings.account_id), \
                    COALESCE(excluded.metering_regime,   limit_readings.metering_regime), \
                    COALESCE(excluded.limit_id,          limit_readings.limit_id), \
                    COALESCE(excluded.model_scope,       limit_readings.model_scope), \
                    COALESCE(excluded.source_order,      limit_readings.source_order), \
                    COALESCE(excluded.covered_from,      limit_readings.covered_from), \
                    COALESCE(excluded.external_activity, limit_readings.external_activity)) \
                IS NOT (limit_readings.plan,            limit_readings.account_id, \
                        limit_readings.metering_regime, limit_readings.limit_id, \
                        limit_readings.model_scope,     limit_readings.source_order, \
                        limit_readings.covered_from,    limit_readings.external_activity) \
               AND (excluded.account_id IS NULL \
                    OR limit_readings.account_id IS NULL \
                    OR excluded.account_id IS limit_readings.account_id) \
               AND (excluded.metering_regime IS NULL \
                    OR limit_readings.metering_regime IS NULL \
                    OR excluded.metering_regime IS limit_readings.metering_regime) \
               AND (excluded.limit_id IS NULL \
                    OR limit_readings.limit_id IS NULL \
                    OR excluded.limit_id IS limit_readings.limit_id) \
               AND (excluded.model_scope IS NULL \
                    OR limit_readings.model_scope IS NULL \
                    OR excluded.model_scope IS limit_readings.model_scope) \
               AND (excluded.source_order IS NULL \
                    OR limit_readings.source_order IS NULL \
                    OR excluded.source_order IS limit_readings.source_order)",
        )?;
        for r in readings {
            let p = &r.provenance;
            written += stmt.execute(params![
                r.source,
                r.window_key,
                r.window_minutes,
                r.used_pct,
                r.resets_at,
                r.observed_at,
                r.via,
                r.plan,
                p.account_id,
                p.metering_regime,
                p.limit_id,
                p.model_scope.as_ref().and_then(ModelScope::stored),
                p.source_order,
                p.covered_from,
                p.external_activity,
            ])? as u64;
        }
    }
    tx.commit()?;
    Ok(written)
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

/// Like insert_events, but first deletes every row whose dedup_key matches one
/// of the `superseded` GLOB patterns, in the same transaction. A Source uses
/// this when its Artifact proves that coarser Records were replaced by the
/// Records being inserted and carry the same usage — the only deletion the
/// Ledger's add-only rule permits (see CONTEXT.md, Ledger).
pub fn insert_events_superseding(
    conn: &mut Connection,
    superseded: &[String],
    events: &[UsageEvent],
) -> rusqlite::Result<u64> {
    let tx = conn.transaction()?;
    // Count distinct NEW rows: a key that survives a supersession delete only
    // to be re-inserted unchanged is not new, so check existence before the
    // deletes run rather than diffing COUNT(*).
    let mut existed_stmt = tx.prepare("SELECT 1 FROM events WHERE dedup_key = ?1")?;
    let mut existed = 0i64;
    for e in events {
        if existed_stmt.exists([e.dedup_key.as_str()])? {
            existed += 1;
        }
    }
    drop(existed_stmt);
    for pattern in superseded {
        tx.execute("DELETE FROM events WHERE dedup_key GLOB ?1", [pattern])?;
    }
    {
        let mut stmt = tx.prepare(&INSERT_SQL)?;
        for e in events {
            stmt.execute(&event_params(e)[..])?;
        }
    }
    tx.commit()?;
    Ok(((events.len() as i64) - existed).max(0) as u64)
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

pub fn clear_file_state(conn: &Connection, path: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM scanned_files WHERE path = ?1", [path])?;
    Ok(())
}

/// Drops scan state for paths that have left disk. The Ledger is permanent, so
/// events are never touched. From v15 this also drops the Codex Context rows
/// keyed to those paths, which is how a rollout demoted to a physical alias
/// stops contributing a second drill-down — the cost being that a Codex rollout
/// the user really deletes takes its Context with it for good. Codex only: its
/// Context is keyed to whichever physical alias won discovery, so a path that
/// leaves disk must not keep a drill-down alive under a name nothing scans any
/// more. Every other Source keeps its Context, on the same rule as events.
pub fn prune_missing_files(conn: &Connection) -> rusqlite::Result<u64> {
    let paths: Vec<String> = {
        let mut stmt = conn.prepare("SELECT path FROM scanned_files")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<String>>>()?
    };
    let mut removed = 0u64;
    for p in paths {
        if !std::path::Path::new(&p).exists() {
            conn.execute(
                "DELETE FROM ctx_tools WHERE source = 'codex' AND source_file = ?1",
                [&p],
            )?;
            clear_file_state(conn, &p)?;
            removed += 1;
        }
    }
    Ok(removed)
}

pub fn clear_ctx_tools_for_file(conn: &Connection, source_file: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM ctx_tools WHERE source_file = ?1", [source_file])?;
    Ok(())
}

pub fn clear_codex_ctx_tools_for_session(
    conn: &Connection,
    source_file: &str,
    session_id: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM ctx_tools
         WHERE source = 'codex'
           AND (source_file = ?1 OR EXISTS (
             SELECT 1 FROM events
             WHERE events.source_file = ctx_tools.source_file
               AND events.source = 'codex' AND events.session_id = ?2
           ))",
        params![source_file, session_id],
    )?;
    Ok(())
}

pub fn clear_ctx_skills_for_file(conn: &Connection, source_file: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM ctx_skills_usage WHERE source_file = ?1", [source_file])?;
    Ok(())
}

/// Additive per-(file, name, local-day) upsert of skill weights. Same
/// idempotency contract as `add_ctx_tool_rows`: callers clear the file's rows
/// before any parse from byte 0.
pub fn add_ctx_skill_rows(
    conn: &mut Connection,
    source: &str,
    source_file: &str,
    rows: &[(String, i64, i64, i64)], // (name, est_tokens, uses, epoch_ts)
) -> rusqlite::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO ctx_skills_usage (source, source_file, name, day, est_tokens, uses) \
             VALUES (?1, ?2, ?3, strftime('%Y-%m-%d', ?4, 'unixepoch', 'localtime'), ?5, ?6) \
             ON CONFLICT(source_file, name, day) DO UPDATE SET \
               est_tokens = est_tokens + excluded.est_tokens, \
               uses = uses + excluded.uses",
        )?;
        for (name, est, uses, ts) in rows {
            stmt.execute(params![source, source_file, name, ts, est, uses])?;
        }
    }
    tx.commit()
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

/// Record a Source Session's metadata (never token figures) so a Session whose
/// transcript the Source has pruned still leaves history in the Ledger
/// (ADR-0016). Idempotent per (source, session_id): re-scanning the same
/// metadata updates the row in place, never duplicating it.
pub fn upsert_source_session(
    conn: &Connection,
    source: &str,
    session: &crate::types::SourceSessionMeta,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO source_sessions (source, session_id, cwd, model, title, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(source, session_id) DO UPDATE SET \
           cwd = COALESCE(excluded.cwd, source_sessions.cwd), \
           model = COALESCE(excluded.model, source_sessions.model), \
           title = COALESCE(excluded.title, source_sessions.title), \
           created_at = COALESCE(excluded.created_at, source_sessions.created_at), \
           updated_at = excluded.updated_at",
        params![
            source,
            session.session_id,
            session.cwd,
            session.model,
            session.title,
            session.created_at,
            session.updated_at,
        ],
    )?;
    Ok(())
}

/// Persist every scanned Source's Unreadable-Artifact state (ADR-0017),
/// zero counts included so a Source whose Artifacts become readable stops
/// marking. Written by every scan; read back for the ≥ floor marker.
pub fn record_unreadable(
    conn: &Connection,
    statuses: &[crate::types::SourceStatus],
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(
        "INSERT OR REPLACE INTO unreadable_artifacts (source, count, max_mtime) \
         VALUES (?1, ?2, ?3)",
    )?;
    // The discovery is also the withdrawal (spec: completeness corrections take
    // effect on the next evaluation). An unreadable Artifact could hold usage as
    // late as its own mtime, so no stored Reading of that Source may go on
    // claiming coverage from before it: every claim below the new floor is
    // raised to it, here at the discovery site rather than at ingest, because an
    // unchanged export file skips ingest entirely. Idempotent — the WHERE fires
    // only on claims still below the floor — and an unreadable with no mtime
    // bounds nothing, so it raises nothing (the pass proves nothing; new
    // Readings' claims are its ingest-side counterpart's business).
    let mut withdraw = conn.prepare(
        "UPDATE limit_readings SET covered_from = ?2 \
         WHERE source = ?1 AND covered_from IS NOT NULL AND covered_from < ?2",
    )?;
    for s in statuses {
        stmt.execute(params![s.source, s.artifacts_unreadable as i64, s.unreadable_max_mtime])?;
        if s.artifacts_unreadable > 0 {
            if let Some(max_mtime) = s.unreadable_max_mtime {
                withdraw.execute(params![s.source, max_mtime + 1])?;
            }
        }
    }
    Ok(())
}

/// Sources currently holding Unreadable Artifacts — the persisted state, so
/// the answer is honest from launch, before this run's first scan.
pub fn load_unreadable(conn: &Connection) -> Vec<crate::types::SourceUnreadable> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT source, count, max_mtime FROM unreadable_artifacts WHERE count > 0 ORDER BY source",
    ) else {
        return Vec::new();
    };
    stmt.query_map([], |r| {
        Ok(crate::types::SourceUnreadable {
            source: r.get(0)?,
            artifacts_unreadable: r.get::<_, i64>(1)?.max(0) as u64,
            unreadable_max_mtime: r.get(2)?,
        })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ReadingProvenance;

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
            "INSERT INTO events (dedup_key, source, timestamp, model, project, api_calls, input_tokens, output_tokens, cache_read_tokens, cache_write_5m_tokens, cache_write_1h_tokens, source_file, session_id, reasoning_tokens, ctx_messages, ctx_system, ctx_reasoning, ctx_toolcalls, ctx_agents, ctx_mcp, ctx_skills) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21) ON CONFLICT(dedup_key) DO UPDATE SET source           = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.source           ELSE events.source           END, input_tokens     = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.input_tokens     ELSE events.input_tokens     END, cache_read_tokens= CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.cache_read_tokens ELSE events.cache_read_tokens END, cache_write_5m_tokens = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.cache_write_5m_tokens ELSE events.cache_write_5m_tokens END, cache_write_1h_tokens = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.cache_write_1h_tokens ELSE events.cache_write_1h_tokens END, project          = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.project          ELSE events.project          END, timestamp        = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.timestamp        ELSE events.timestamp        END, model            = excluded.model, api_calls        = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.api_calls        ELSE events.api_calls        END, source_file      = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.source_file      ELSE events.source_file      END, session_id       = COALESCE(events.session_id, excluded.session_id), reasoning_tokens = COALESCE(events.reasoning_tokens, excluded.reasoning_tokens), ctx_messages  = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_messages  ELSE COALESCE(events.ctx_messages,  excluded.ctx_messages)  END, ctx_system    = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_system    ELSE COALESCE(events.ctx_system,    excluded.ctx_system)    END, ctx_reasoning = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_reasoning ELSE COALESCE(events.ctx_reasoning, excluded.ctx_reasoning) END, ctx_toolcalls = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_toolcalls ELSE COALESCE(events.ctx_toolcalls, excluded.ctx_toolcalls) END, ctx_agents    = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_agents    ELSE COALESCE(events.ctx_agents,    excluded.ctx_agents)    END, ctx_mcp       = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_mcp       ELSE COALESCE(events.ctx_mcp,       excluded.ctx_mcp)       END, ctx_skills    = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_skills    ELSE COALESCE(events.ctx_skills,    excluded.ctx_skills)    END, output_tokens    = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.output_tokens    ELSE events.output_tokens    END WHERE excluded.output_tokens >= events.output_tokens"
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

    // Unreadable-Artifact state (ADR-0017) round-trips per scan: zero counts
    // are written (so a Source whose Artifacts become readable stops marking)
    // but never loaded, and a later scan's counts replace the earlier ones.
    #[test]
    fn unreadable_artifacts_roundtrip_and_replace() {
        let (_dir, conn) = temp_db();
        let status = |source: &str, count, mtime| crate::types::SourceStatus {
            source: source.to_string(),
            events_inserted: 0,
            lines_skipped: 0,
            limit_readings: 0,
            artifacts_unreadable: count,
            unreadable_max_mtime: mtime,
            error: None,
        };

        record_unreadable(&conn, &[status("antigravity", 100, Some(1_000)), status("claude", 0, None)])
            .unwrap();
        assert_eq!(
            load_unreadable(&conn),
            vec![crate::types::SourceUnreadable {
                source: "antigravity".to_string(),
                artifacts_unreadable: 100,
                unreadable_max_mtime: Some(1_000),
            }]
        );

        record_unreadable(&conn, &[status("antigravity", 0, None)]).unwrap();
        assert_eq!(load_unreadable(&conn), vec![]);
    }

    #[test]
    fn fresh_db_has_tables_and_user_version() {
        let (_dir, conn) = temp_db();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_USER_VERSION);
        for table in ["events", "scanned_files", "prices", "price_overrides", "model_price_history", "ctx_tools", "ctx_exec", "settings", "pi_tool_owner", "unreadable_artifacts", "limit_readings"] {
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
        assert_eq!(version, CURRENT_USER_VERSION);
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
        assert_eq!(version, CURRENT_USER_VERSION);
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
    fn v8_db_gains_the_price_catalog_column_without_losing_ledger_or_prices() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        {
            let conn = Connection::open(&path).unwrap();
            for batch in [SCHEMA, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7, SCHEMA_V8] {
                conn.execute_batch(batch).unwrap();
            }
            conn.execute(
                "INSERT INTO events (dedup_key, source, timestamp, model, project, api_calls, \
                 input_tokens, output_tokens, cache_read_tokens, cache_write_5m_tokens, \
                 cache_write_1h_tokens, source_file) \
                 VALUES ('claude:old:1','claude',123,'claude-existing','/p',2,10,20,3,4,5,'f')",
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
            conn.execute(
                "INSERT INTO scanned_files (path, size, mtime, byte_offset) VALUES ('f',1,1,7)",
                [],
            ).unwrap();
        }

        let conn = open_db(&path).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, CURRENT_USER_VERSION);
        let has_catalog: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('prices') WHERE name = 'catalog'",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(has_catalog, 1, "prices must record which catalog supplied each row");

        // The Ledger, the List Price, and the Override all survive untouched.
        let usage: (i64, i64, i64, i64, i64, i64) = conn.query_row(
            "SELECT api_calls, input_tokens, output_tokens, cache_read_tokens, \
             cache_write_5m_tokens, cache_write_1h_tokens FROM events",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        ).unwrap();
        assert_eq!(usage, (2, 10, 20, 3, 4, 5));
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

        // Prices rebuild from the catalog on their own, so this migration has no
        // reason to force a re-parse. Asserted on the batch itself: sibling
        // migrations DO clear scan state, and the ladder to current now crosses
        // v12, which clears it by design to populate per-skill weights.
        assert!(
            !SCHEMA_V9.contains("scanned_files"),
            "the price-catalog migration must not clear scan state",
        );
    }

    #[test]
    fn v13_db_migrates_to_v14_creating_limit_readings_and_clearing_scan_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        // A genuine v13 database: every batch up to V13 and nothing after.
        {
            let conn = Connection::open(&path).unwrap();
            for batch in [
                SCHEMA, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7,
                SCHEMA_V8, SCHEMA_V9, SCHEMA_V10, SCHEMA_V11, SCHEMA_V12, SCHEMA_V13,
            ] {
                conn.execute_batch(batch).unwrap();
            }
            conn.execute(
                "INSERT INTO events (dedup_key, source, timestamp, model, project, api_calls, \
                 input_tokens, output_tokens, cache_read_tokens, cache_write_5m_tokens, \
                 cache_write_1h_tokens, source_file) \
                 VALUES ('codex:old:1','codex',1,'gpt-5.4',NULL,1,10,20,0,0,0,'f')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO scanned_files (path, size, mtime, byte_offset) VALUES ('f',1,1,1)",
                [],
            )
            .unwrap();
        }

        let mut conn = open_db(&path).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, CURRENT_USER_VERSION);
        // Scan state cleared, so the next scan re-parses every log once and Codex
        // limit history back-fills retroactively.
        let files: i64 = conn
            .query_row("SELECT COUNT(*) FROM scanned_files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(files, 0);
        // The Ledger is untouched by it.
        let input: i64 = conn
            .query_row("SELECT input_tokens FROM events WHERE dedup_key='codex:old:1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(input, 10);

        // The table exists and its key is the reading's own observation.
        let reading = |used_pct: f64, resets_at: i64| LimitReading {
            source: "codex".to_string(),
            window_key: "w10080".to_string(),
            window_minutes: Some(10080),
            used_pct,
            resets_at,
            observed_at: 1_786_331_779,
            via: "logs".to_string(),
            plan: Some("plus".to_string()),
            provenance: ReadingProvenance::default(),
        };
        insert_limit_readings(&mut conn, &[reading(40.0, 100), reading(40.0, 100)]).unwrap();
        assert_eq!(stored_readings(&conn), 1, "one source observation twice is one Reading");
        insert_limit_readings(&mut conn, &[reading(41.0, 100), reading(40.0, 200)]).unwrap();
        assert_eq!(
            stored_readings(&conn),
            3,
            "a new percentage and a new epoch are each their own row",
        );

        // Re-running the migration is safe: nothing here is a one-shot backfill,
        // so an intermediate dev build against the real database cannot consume it.
        conn.execute_batch(SCHEMA_V14).unwrap();
        let kept: i64 = conn
            .query_row("SELECT COUNT(*) FROM limit_readings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kept, 3);
    }

    #[test]
    fn v14_db_migrates_to_daily_price_history_without_clearing_scan_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        {
            let conn = Connection::open(&path).unwrap();
            for batch in [
                SCHEMA, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7,
                SCHEMA_V8, SCHEMA_V9, SCHEMA_V10, SCHEMA_V11, SCHEMA_V12, SCHEMA_V13,
                SCHEMA_V14,
            ] {
                conn.execute_batch(batch).unwrap();
            }
            conn.execute(
                "INSERT INTO scanned_files (path, size, mtime, byte_offset) VALUES ('f',1,1,1)",
                [],
            )
            .unwrap();
            // Isolate v15: open_db would continue to v16 and clear scan state.
            conn.execute_batch(SCHEMA_V15).unwrap();
        }

        let conn = Connection::open(&path).unwrap();
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, 15);
        let history_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='model_price_history'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(history_exists, 1);
        let files: i64 = conn.query_row("SELECT COUNT(*) FROM scanned_files", [], |r| r.get(0)).unwrap();
        assert_eq!(files, 1, "price history must not force a Source Artifact re-scan");
    }

    #[test]
    fn v15_db_migrates_to_v16_rebuilding_context_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        {
            let conn = Connection::open(&path).unwrap();
            for batch in [
                SCHEMA, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7,
                SCHEMA_V8, SCHEMA_V9, SCHEMA_V10, SCHEMA_V11, SCHEMA_V12, SCHEMA_V13,
                SCHEMA_V14, SCHEMA_V15,
            ] {
                conn.execute_batch(batch).unwrap();
            }
            conn.execute(
                "INSERT INTO events (dedup_key, source, timestamp, model, source_file) \
                 VALUES ('codex:kept:1', 'codex', 1, 'gpt-5.4', 'rollout.jsonl')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO scanned_files (path, size, mtime, byte_offset) \
                 VALUES ('rollout.jsonl', 1, 1, 1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO ctx_tools (source, source_file, name, day, est_tokens, calls) \
                 VALUES ('codex', 'rollout.jsonl', 'shell', '2026-05-03', 10, 2), \
                        ('claude', 'missing.jsonl', 'Bash', '2026-05-03', 10, 1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO ctx_exec \
                 (source, source_file, day, kind, exe, cmd, est_tokens, calls) \
                 VALUES ('claude', 'missing.jsonl', '2026-05-03', 'test', 'cargo', \
                         'cargo test', 10, 1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO ctx_skills_usage \
                 (source, source_file, name, day, est_tokens, uses) \
                 VALUES ('claude', 'missing.jsonl', 'tdd', '2026-05-03', 10, 1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO pi_tool_owner (ident, source_file) \
                 VALUES ('tool-entry', 'missing.jsonl')",
                [],
            )
            .unwrap();
        }

        let conn = open_db(&path).unwrap();
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        let events: i64 = conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0)).unwrap();
        let files: i64 =
            conn.query_row("SELECT COUNT(*) FROM scanned_files", [], |r| r.get(0)).unwrap();
        let tools: i64 = conn.query_row("SELECT COUNT(*) FROM ctx_tools", [], |r| r.get(0)).unwrap();
        let exec: i64 = conn.query_row("SELECT COUNT(*) FROM ctx_exec", [], |r| r.get(0)).unwrap();
        let skills: i64 =
            conn.query_row("SELECT COUNT(*) FROM ctx_skills_usage", [], |r| r.get(0)).unwrap();
        let owners: i64 =
            conn.query_row("SELECT COUNT(*) FROM pi_tool_owner", [], |r| r.get(0)).unwrap();

        assert_eq!(
            (version, events, files, tools, exec, skills, owners),
            (CURRENT_USER_VERSION, 1, 0, 0, 0, 0, 0)
        );
    }

    #[test]
    fn v16_db_migrates_qoder_aliases_without_deleting_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        {
            let conn = Connection::open(&path).unwrap();
            for batch in [
                SCHEMA, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7,
                SCHEMA_V8, SCHEMA_V9, SCHEMA_V10, SCHEMA_V11, SCHEMA_V12, SCHEMA_V13,
                SCHEMA_V14, SCHEMA_V15, SCHEMA_V16,
            ] {
                conn.execute_batch(batch).unwrap();
            }
            conn.execute(
                "INSERT INTO events (dedup_key, source, timestamp, model, source_file) VALUES
                 ('qoder:legacy', 'qoder', 1, 'dfmodel', 'qoder.db'),
                 ('qoder:known', 'qoder', 2, 'qwen3.8-max', 'qoder.db'),
                 ('claude:legacy', 'claude', 3, 'dfmodel', 'claude.jsonl')",
                [],
            )
            .unwrap();
        }

        let conn = open_db(&path).unwrap();
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        let events: i64 = conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0)).unwrap();
        let qoder_legacy: String = conn
            .query_row(
                "SELECT model FROM events WHERE dedup_key = 'qoder:legacy'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let qoder_known: String = conn
            .query_row(
                "SELECT model FROM events WHERE dedup_key = 'qoder:known'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let other_source: String = conn
            .query_row(
                "SELECT model FROM events WHERE dedup_key = 'claude:legacy'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        assert_eq!(version, CURRENT_USER_VERSION);
        assert_eq!(events, 3, "migration preserves every Usage Record");
        assert_eq!(qoder_legacy, "deepseek-v4-flash");
        assert_eq!(qoder_known, "qwen3.8-max");
        assert_eq!(other_source, "dfmodel", "migration is scoped to Qoder");
    }

    fn stored_readings(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM limit_readings", [], |r| r.get(0))
            .unwrap()
    }

    fn stored_provenance(conn: &Connection, observed_at: i64) -> ReadingProvenance {
        conn.query_row(
            "SELECT account_id, metering_regime, limit_id, model_scope, source_order, \
             covered_from, external_activity FROM limit_readings WHERE observed_at = ?1",
            [observed_at],
            |r| {
                Ok(ReadingProvenance {
                    account_id: r.get(0)?,
                    metering_regime: r.get(1)?,
                    limit_id: r.get(2)?,
                    model_scope: r
                        .get::<_, Option<String>>(3)?
                        .and_then(|s| ModelScope::parse(&s)),
                    source_order: r.get(4)?,
                    covered_from: r.get(5)?,
                    external_activity: r.get(6)?,
                })
            },
        )
        .unwrap()
    }

    /// A Codex Reading of the 7-day window, observed at `observed_at`.
    fn evidence_reading(used_pct: f64, observed_at: i64) -> LimitReading {
        LimitReading {
            source: "codex".to_string(),
            window_key: "w10080".to_string(),
            window_minutes: Some(10080),
            used_pct,
            resets_at: 1_786_879_486,
            observed_at,
            via: "logs".to_string(),
            plan: Some("plus".to_string()),
            provenance: ReadingProvenance::default(),
        }
    }

    #[test]
    fn v17_db_migrates_to_v18_adding_evidence_provenance_without_a_rescan() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        {
            let conn = Connection::open(&path).unwrap();
            for batch in [
                SCHEMA, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6, SCHEMA_V7,
                SCHEMA_V8, SCHEMA_V9, SCHEMA_V10, SCHEMA_V11, SCHEMA_V12, SCHEMA_V13,
                SCHEMA_V14, SCHEMA_V15, SCHEMA_V16, SCHEMA_V17,
            ] {
                conn.execute_batch(batch).unwrap();
            }
            conn.execute(
                "INSERT INTO limit_readings \
                 (source, window_key, window_minutes, used_pct, resets_at, observed_at, via, plan) \
                 VALUES ('codex','w10080',10080,40.0,1000,900,'logs','plus')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO events (dedup_key, source, timestamp, model, source_file) \
                 VALUES ('codex:legacy','codex',1,'gpt-5.4','rollout.jsonl')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO scanned_files (path, size, mtime, byte_offset) \
                 VALUES ('rollout.jsonl',1,1,1)",
                [],
            )
            .unwrap();
        }

        let conn = open_db(&path).unwrap();
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, CURRENT_USER_VERSION);

        // The stored Reading survives the rebuild verbatim, and the Limits page
        // still draws it.
        let kept: (String, String, Option<i64>, f64, i64, i64, String, Option<String>) = conn
            .query_row(
                "SELECT source, window_key, window_minutes, used_pct, resets_at, observed_at, \
                 via, plan FROM limit_readings",
                [],
                |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
                },
            )
            .unwrap();
        assert_eq!(
            kept,
            (
                "codex".to_string(),
                "w10080".to_string(),
                Some(10080),
                40.0,
                1000,
                900,
                "logs".to_string(),
                Some("plus".to_string()),
            ),
        );
        let cards = crate::queries::limits(&conn, 1_900_000_000).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].windows[0].used_pct, 40.0);
        assert_eq!(cards[0].plan.as_deref(), Some("plus"));

        // Legacy provenance stays unknown on both sides: nothing about the
        // account, meter, Limit, scope, order, coverage, or external activity of
        // a pre-migration Reading is knowable, so nothing is synthesized.
        assert_eq!(stored_provenance(&conn, 900), ReadingProvenance::default());
        let account: Option<String> = conn
            .query_row(
                "SELECT account_id FROM events WHERE dedup_key = 'codex:legacy'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(account, None, "a legacy Usage Record has no account identity");

        // Scan state is untouched: re-parsing today's Artifacts could not prove
        // an old interval's account or completeness anyway (spec hard rule).
        let files: i64 = conn
            .query_row("SELECT COUNT(*) FROM scanned_files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(files, 1);

        // Estimates are derived, never materialised (ADR-0024) — and the one
        // index the Usage-membership query needs is the only one added.
        let derived: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' \
                 AND (name LIKE '%estimate%' OR name LIKE '%limit_reading_history%')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(derived, 0);
        let index: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' \
                 AND name = 'idx_events_evidence'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(index, 1);

        // Running the migration again is safe — an intermediate dev build may —
        // and this one rebuilds a table, so prove the rebuild copies proven
        // provenance rather than the eight columns a v17 database had.
        conn.execute(
            "UPDATE limit_readings SET account_id = 'acct-9f3', covered_from = 600",
            [],
        )
        .unwrap();
        conn.execute_batch("PRAGMA user_version = 17").unwrap();
        drop(conn);
        let conn = open_db(&path).unwrap();
        assert_eq!(stored_readings(&conn), 1);
        assert_eq!(
            stored_provenance(&conn, 900),
            ReadingProvenance {
                account_id: Some("acct-9f3".to_string()),
                covered_from: Some(600),
                ..ReadingProvenance::default()
            },
        );
    }

    #[test]
    fn reading_identity_dedupes_re_ingestion_and_keeps_a_later_equal_percentage() {
        let (_dir, mut conn) = temp_db();
        // The same source observation, read twice — a re-scan of an appended
        // rollout, or the scan and the command that just ran a Companion both
        // ingesting. The second read writes nothing at all.
        assert_eq!(
            insert_limit_readings(&mut conn, &[evidence_reading(40.0, 900)]).unwrap(),
            1,
        );
        let settled = conn.total_changes();
        assert_eq!(
            insert_limit_readings(&mut conn, &[evidence_reading(40.0, 900)]).unwrap(),
            0,
            "re-reading a stored Reading is not a change to evaluate on",
        );
        assert_eq!(conn.total_changes(), settled, "and costs no write");
        assert_eq!(stored_readings(&conn), 1);

        // A genuinely later Reading at an unchanged percentage is its own: after
        // a gap it is the only anchor the next run can start from.
        insert_limit_readings(&mut conn, &[evidence_reading(40.0, 5_400)]).unwrap();
        let stamps: Vec<i64> = conn
            .prepare("SELECT observed_at FROM limit_readings ORDER BY observed_at")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(stamps, vec![900, 5_400]);
    }

    #[test]
    fn reading_provenance_round_trips_enriches_and_never_erases() {
        let (_dir, mut conn) = temp_db();
        let proven = ReadingProvenance {
            account_id: Some("acct-9f3".to_string()),
            metering_regime: Some("codex-standard".to_string()),
            limit_id: Some("codex".to_string()),
            model_scope: Some(ModelScope::All),
            source_order: Some(17),
            covered_from: Some(600),
            external_activity: None,
        };
        let mut reading = evidence_reading(40.0, 900);
        reading.provenance = proven.clone();
        insert_limit_readings(&mut conn, &[reading]).unwrap();
        assert_eq!(stored_provenance(&conn, 900), proven);

        // A later pass that proves nothing — a live Reading of the same
        // observation — must not erase what a richer one already proved, and has
        // nothing to write.
        let settled = conn.total_changes();
        assert_eq!(
            insert_limit_readings(&mut conn, &[evidence_reading(40.0, 900)]).unwrap(),
            0,
        );
        assert_eq!(conn.total_changes(), settled);
        assert_eq!(stored_provenance(&conn, 900), proven);

        // Provenance is correctable: discovering an unreadable Artifact inside
        // the covered stretch, or external activity, revises the Reading in place
        // and the next evaluation derives from the corrected fact.
        let mut corrected = evidence_reading(40.0, 900);
        corrected.provenance = ReadingProvenance {
            covered_from: Some(880),
            external_activity: Some("codex-web".to_string()),
            ..ReadingProvenance::default()
        };
        assert_eq!(insert_limit_readings(&mut conn, &[corrected]).unwrap(), 1);
        assert_eq!(
            stored_provenance(&conn, 900),
            ReadingProvenance {
                covered_from: Some(880),
                external_activity: Some("codex-web".to_string()),
                ..proven
            },
        );
        assert_eq!(stored_readings(&conn), 1, "a correction is not a new Reading");
    }

    #[test]
    fn each_evidence_identity_keeps_its_own_readings() {
        let (_dir, mut conn) = temp_db();
        let with = |observed_at: i64, provenance: ReadingProvenance| {
            let mut reading = evidence_reading(40.0, observed_at);
            reading.provenance = provenance;
            reading
        };
        let first = ReadingProvenance {
            account_id: Some("acct-a".to_string()),
            metering_regime: Some("codex-standard".to_string()),
            limit_id: Some("codex".to_string()),
            model_scope: Some(ModelScope::All),
            ..ReadingProvenance::default()
        };
        let second = ReadingProvenance {
            account_id: Some("acct-b".to_string()),
            metering_regime: Some("codex-priority".to_string()),
            limit_id: Some("codex-credits".to_string()),
            model_scope: Some(ModelScope::Models(vec!["gpt-5.4-codex".to_string()])),
            ..ReadingProvenance::default()
        };
        insert_limit_readings(
            &mut conn,
            &[with(900, first.clone()), with(1_800, second.clone())],
        )
        .unwrap();
        assert_eq!(stored_readings(&conn), 2);
        assert_eq!(stored_provenance(&conn, 900), first);
        assert_eq!(stored_provenance(&conn, 1_800), second);

        // A change of plan or of reset epoch likewise keeps its own Readings —
        // the two facts the Reading carries itself rather than in its provenance.
        let mut other_plan = evidence_reading(40.0, 2_700);
        other_plan.plan = Some("pro".to_string());
        let mut other_epoch = evidence_reading(40.0, 900);
        other_epoch.resets_at += 86_400;
        insert_limit_readings(&mut conn, &[other_plan, other_epoch]).unwrap();
        let plans: Vec<(Option<String>, i64)> = conn
            .prepare("SELECT plan, resets_at FROM limit_readings ORDER BY observed_at, resets_at")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(
            plans,
            vec![
                (Some("plus".to_string()), 1_786_879_486),
                (Some("plus".to_string()), 1_786_965_886),
                (Some("plus".to_string()), 1_786_879_486),
                (Some("pro".to_string()), 1_786_879_486),
            ],
        );
    }

    #[test]
    fn contradicting_identities_are_never_blended_into_one_reading() {
        let (_dir, mut conn) = temp_db();
        // Two Readings alike in every key column — same Source, window, epoch
        // second, observation second, percentage — but proven to different
        // accounts. This table cannot hold them apart (SCHEMA_V18's ponytail
        // note), and the wrong answer would be a row wearing one account's
        // identity over the other's completeness proof. The second is dropped
        // whole instead.
        let stored = ReadingProvenance {
            account_id: Some("acct-a".to_string()),
            covered_from: Some(600),
            ..ReadingProvenance::default()
        };
        let mut first = evidence_reading(40.0, 900);
        first.provenance = stored.clone();
        let mut other_account = evidence_reading(40.0, 900);
        other_account.provenance = ReadingProvenance {
            account_id: Some("acct-b".to_string()),
            metering_regime: Some("codex-priority".to_string()),
            ..ReadingProvenance::default()
        };
        insert_limit_readings(&mut conn, &[first]).unwrap();
        assert_eq!(
            insert_limit_readings(&mut conn, &[other_account]).unwrap(),
            0,
            "a contradicted Reading is refused, not merged",
        );
        assert_eq!(stored_readings(&conn), 1);
        assert_eq!(stored_provenance(&conn, 900), stored);

        // Source order holds to the same rule. An unknown order still fills,
        let mut ordered = evidence_reading(40.0, 900);
        ordered.provenance = ReadingProvenance {
            source_order: Some(17),
            ..ReadingProvenance::default()
        };
        insert_limit_readings(&mut conn, &[ordered]).unwrap();
        assert_eq!(stored_provenance(&conn, 900).source_order, Some(17));
        // but a second Artifact reaching this Reading does not get to move the
        // order the first gave it — an order that moves orders nothing — and the
        // refusal takes the whole update with it, coverage included.
        let mut moved = evidence_reading(40.0, 900);
        moved.provenance = ReadingProvenance {
            source_order: Some(4_096),
            covered_from: Some(1),
            ..ReadingProvenance::default()
        };
        assert_eq!(insert_limit_readings(&mut conn, &[moved]).unwrap(), 0);
        assert_eq!(stored_provenance(&conn, 900).source_order, Some(17));
        assert_eq!(stored_provenance(&conn, 900).covered_from, Some(600));
    }

    #[test]
    fn model_scope_stored_form_is_canonical() {
        assert_eq!(ModelScope::All.stored().as_deref(), Some("all"));
        // Order and repetition in the scope a Source reports are not identity:
        // the same set stores as the same string, so a Series holds together.
        let one = ModelScope::Models(vec!["opus-4.5".to_string(), "sonnet-4.5".to_string()]);
        let other = ModelScope::Models(vec![
            "sonnet-4.5".to_string(),
            "opus-4.5".to_string(),
            "opus-4.5".to_string(),
        ]);
        assert_eq!(one.stored(), other.stored());
        assert_eq!(
            ModelScope::parse(&one.stored().unwrap()),
            Some(ModelScope::Models(vec![
                "opus-4.5".to_string(),
                "sonnet-4.5".to_string(),
            ])),
        );
        // An empty set is no scope: it stores as unknown rather than as a scope
        // that matches nothing, and no stored string reads back as one.
        assert_eq!(ModelScope::Models(Vec::new()).stored(), None);
        assert_eq!(ModelScope::parse("seven_day_opus"), None);
        assert_eq!(ModelScope::parse("[]"), None);
    }

    #[test]
    fn v1_db_migrates_to_v2_preserving_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        // Build a genuine v1 database by hand (SCHEMA still writes user_version 1),
        // then prove open_db chains every migration through v9 in one shot.
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
        assert_eq!(v, CURRENT_USER_VERSION);
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
    fn keep_max_tie_updates_model_but_not_attribution() {
        let (_dir, mut conn) = temp_db();
        let mut first = sample_event("claude:rep:1", "fileA.jsonl");
        first.session_id = Some("sess-A".to_string());
        first.model = Some("glm-5.2-x".to_string());
        insert_events_keep_max_output(&mut conn, &[first]).unwrap();

        // A parser-version re-parse re-inserts the same row (equal output)
        // carrying a corrected Model name: Latest must replace the stored
        // value, or the version bump would change nothing for existing rows.
        let mut reparsed = sample_event("claude:rep:1", "fileA.jsonl");
        reparsed.session_id = Some("sess-A".to_string());
        reparsed.model = Some("glm-5.2".to_string());
        let n = insert_events_keep_max_output(&mut conn, &[reparsed]).unwrap();
        assert_eq!(n, 0);
        let (model, sid, file): (Option<String>, Option<String>, String) = conn
            .query_row(
                "SELECT model, session_id, source_file FROM events WHERE dedup_key='claude:rep:1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(model, Some("glm-5.2".to_string()), "tie takes the re-parsed model");
        assert_eq!(sid, Some("sess-A".to_string()), "attribution stays first-writer");
        assert_eq!(file, "fileA.jsonl");
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
        assert_eq!(v, CURRENT_USER_VERSION);
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
        assert_eq!(v, CURRENT_USER_VERSION);
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

        // Context is rebuildable, but only Codex rebuilds it from the file alone.
        // A vanished Claude session can never be re-read, so its drill-down must
        // outlive the file the way its events do.
        // (ctx_tools is unique on (source_file, name, day), so the two rows need
        // distinct tool names to coexist on the one vanished path.)
        let codex_row = [("shell".to_string(), 10i64, 1i64, 1_700_000_000i64)];
        let claude_row = [("Bash".to_string(), 10i64, 1i64, 1_700_000_000i64)];
        add_ctx_tool_rows(&mut conn, "codex", "/nonexistent/gone.jsonl", &codex_row).unwrap();
        add_ctx_tool_rows(&mut conn, "claude", "/nonexistent/gone.jsonl", &claude_row).unwrap();

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
        let tools: Vec<String> = {
            let mut stmt = conn.prepare("SELECT source FROM ctx_tools ORDER BY source").unwrap();
            let rows = stmt.query_map([], |r| r.get(0)).unwrap();
            rows.map(|r| r.unwrap()).collect()
        };
        assert_eq!(tools, vec!["claude".to_string()], "only Codex Context follows the file out");
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
        assert_eq!(v, CURRENT_USER_VERSION);
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

    // The Codex Context clear is session-scoped, not file-scoped: a rollout that
    // also lives under a second Session root is re-parsed under a different path,
    // and the rows the other copy left behind belong to the same session, so a
    // re-parse from byte 0 has to take them too or the drill-down doubles. Its
    // reach stops at that session and at Codex: another session's rows, and any
    // other Source's rows on the very same path, are none of its business.
    #[test]
    fn clearing_a_codex_session_reaches_every_copy_of_it_and_nothing_else() {
        let (_dir, mut conn) = temp_db();
        let ts = 1_782_907_200i64;
        let codex_event = |dedup_key: &str, source_file: &str, session_id: &str| UsageEvent {
            source: "codex".to_string(),
            session_id: Some(session_id.to_string()),
            ..sample_event(dedup_key, source_file)
        };
        add_ctx_tool_rows(&mut conn, "codex", "/a/rollout-x.jsonl", &[
            ("shell".to_string(), 100, 1, ts),
        ]).unwrap();
        add_ctx_tool_rows(&mut conn, "codex", "/b/rollout-x.jsonl", &[
            ("shell".to_string(), 100, 1, ts),
        ]).unwrap();
        add_ctx_tool_rows(&mut conn, "codex", "/c/rollout-y.jsonl", &[
            ("shell".to_string(), 40, 1, ts),
        ]).unwrap();
        add_ctx_tool_rows(&mut conn, "claude", "/b/rollout-x.jsonl", &[
            ("Bash".to_string(), 7, 1, ts),
        ]).unwrap();
        // Only events carry the session id, so the copies of a session are found
        // through them and never through the path.
        insert_events(&mut conn, &[
            codex_event("codex:rollout-x:0", "/b/rollout-x.jsonl", "rollout-x"),
            codex_event("codex:rollout-y:0", "/c/rollout-y.jsonl", "rollout-y"),
        ]).unwrap();

        clear_codex_ctx_tools_for_session(&conn, "/a/rollout-x.jsonl", "rollout-x").unwrap();

        let left: Vec<(String, String)> = {
            let mut stmt = conn
                .prepare("SELECT source, source_file FROM ctx_tools ORDER BY source, source_file")
                .unwrap();
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
            rows.collect::<rusqlite::Result<Vec<_>>>().unwrap()
        };
        assert_eq!(left, vec![
            ("claude".to_string(), "/b/rollout-x.jsonl".to_string()),
            ("codex".to_string(), "/c/rollout-y.jsonl".to_string()),
        ]);
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
        assert_eq!(v, CURRENT_USER_VERSION);
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
        assert_eq!(v, CURRENT_USER_VERSION);
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='settings'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        // v6 is not a backfill migration. Asserted on the batch itself, since
        // the ladder to current now crosses v12, which clears scan state by
        // design to force the per-skill re-parse.
        assert!(
            !SCHEMA_V6.contains("scanned_files"),
            "settings migration must not clear scan state",
        );
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
    fn ctx_skill_rows_accumulate_and_clear_per_file() {
        let (_dir, mut conn) = temp_db();
        let ts = 1_782_907_200i64;
        // Re-invoking a skill re-injects the whole body, so same day + name
        // accumulates both the tokens and the use count.
        add_ctx_skill_rows(&mut conn, "claude", "f1.jsonl", &[
            ("grilling".into(), 2400, 1, ts),
            ("grilling".into(), 2400, 1, ts + 60),
            ("playground:playground".into(), 3900, 1, ts),
        ]).unwrap();
        add_ctx_skill_rows(&mut conn, "claude", "f2.jsonl", &[
            ("grilling".into(), 100, 1, ts),
        ]).unwrap();

        let (est, uses): (i64, i64) = conn
            .query_row(
                "SELECT est_tokens, uses FROM ctx_skills_usage \
                 WHERE source_file='f1.jsonl' AND name='grilling'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((est, uses), (4800, 2), "repeat injections sum, they do not dedupe");

        // A parse from byte 0 rebuilds only this file's rows.
        clear_ctx_skills_for_file(&conn, "f1.jsonl").unwrap();
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM ctx_skills_usage", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 1, "the other file's rows are untouched");
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
