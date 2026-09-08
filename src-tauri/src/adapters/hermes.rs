// TokenLedger — Hermes adapter.
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use super::{file_state_of, unchanged};
use crate::db::{delete_events, set_file_state, upsert_events};
use crate::source_catalog;
use crate::types::{FileState, SourceScanResult, UsageEvent};

/// Bump to force a re-read of every Hermes store on the next scan. Stored in
/// the file-state's otherwise-unused `byte_offset` (this adapter re-reads whole
/// databases, so it never tracks a real offset), which makes the re-scan
/// self-clearing: the mismatch fires once, then the new version persists. A
/// size+mtime skip alone would leave an idle store on whatever the previous
/// parser booked for it, forever.
/// v1 = usage read per (Model, billing route, task) from `session_model_usage`
/// instead of the `sessions` rollup, which cannot see auxiliary calls, carries
/// only a Session's first Model, and dates everything to `started_at`.
const PARSER_VERSION: i64 = 1;

pub fn scan_hermes(conn: &mut Connection, hermes_db: &Path) -> SourceScanResult {
    let mut result = SourceScanResult::default();
    let mut errors = Vec::new();
    for database in discover_databases(hermes_db) {
        let scanned = scan_hermes_database(conn, &database);
        result.events_inserted += scanned.events_inserted;
        result.lines_skipped += scanned.lines_skipped;
        if let Some(error) = scanned.error {
            errors.push(error);
        }
    }
    result.error = (!errors.is_empty()).then(|| errors.join("; "));
    result
}

fn scan_hermes_database(conn: &mut Connection, hermes_db: &Path) -> SourceScanResult {
    // Whole-DB skip when neither the main file nor its WAL moved since the
    // last scan; without this every scan re-upserts every session, which
    // keeps the ledger churning (and the frontend reloading) forever. A
    // missing DB is a quiet no-op; an existing malformed DB reports an error.
    let db_state = FileState { byte_offset: PARSER_VERSION, ..file_state_of(hermes_db) };
    let wal_path = hermes_db.with_extension("db-wal");
    let wal_state = FileState { byte_offset: PARSER_VERSION, ..file_state_of(&wal_path) };
    let db_exists = hermes_db.is_file();
    if !db_exists {
        return SourceScanResult::default();
    }
    if unchanged(conn, hermes_db, &db_state) && unchanged(conn, &wal_path, &wal_state) {
        return SourceScanResult::default();
    }

    let ro = match super::open_sqlite_artifact("hermes", hermes_db) {
        Ok(c) => c,
        Err(e) => {
            // Lock/open failure: keep prior events, report staleness.
            return SourceScanResult {
                error: Some(e),
                ..Default::default()
            };
        }
    };

    // `session_model_usage` is where Hermes' accounting actually lives, and the
    // `sessions` rollup cannot substitute for it in three ways, all
    // load-bearing:
    //   * auxiliary calls (compression, title generation, background review)
    //     are recorded there *without* touching the rollup — Hermes says so in
    //     `record_auxiliary_usage` — so reading `sessions` silently drops every
    //     one of them;
    //   * the rollup carries one (Model, provider) pair, the session's FIRST
    //     accounted route, so a mid-session model switch prices every token of
    //     the session at the wrong Model;
    //   * `last_seen` is stamped on each per-call delta, so a Session resumed
    //     weeks after it started books on the day it was used instead of on
    //     `started_at`.
    // The table arrived in Hermes schema v20 and gained `task` in v22. Either
    // absence falls back to the rollup rather than failing the Source, so an
    // install too old to have it keeps exactly the behaviour it has today.
    let usage_columns = artifact_columns(&ro, "session_model_usage");
    let per_model = !usage_columns.is_empty();
    let sql = if per_model {
        // Pre-v22 stores have the table without the column; there are no
        // per-task rows in them to lose.
        let task = if usage_columns.iter().any(|name| name == "task") { "m.task" } else { "''" };
        // The dedup key is built in SQL because it is exactly this table's own
        // primary key, every part of it is NOT NULL there, and nothing outside
        // the key needs the parts.
        format!(
            "SELECT 'hermes:' || m.session_id || '|' || m.model || '|' || m.billing_provider \
                              || '|' || m.billing_base_url || '|' || m.billing_mode || '|' || {task}, \
                    m.session_id, m.model, \
                    COALESCE(m.last_seen, m.first_seen, s.started_at), \
                    m.input_tokens, m.output_tokens, m.cache_read_tokens, \
                    m.cache_write_tokens, m.reasoning_tokens, m.api_call_count, s.cwd \
               FROM session_model_usage m LEFT JOIN sessions s ON s.id = m.session_id"
        )
    } else {
        "SELECT 'hermes:' || s.id, s.id, s.model, s.started_at, \
                s.input_tokens, s.output_tokens, s.cache_read_tokens, \
                s.cache_write_tokens, s.reasoning_tokens, s.api_call_count, s.cwd \
           FROM sessions s"
            .to_string()
    };

    let mut stmt = match ro.prepare(&sql) {
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
            r.get::<_, String>(0)?,                    // dedup_key
            r.get::<_, String>(1)?,                    // session id
            r.get::<_, Option<String>>(2)?,            // model
            r.get::<_, Option<f64>>(3)?,               // timestamp (REAL epoch secs)
            r.get::<_, Option<i64>>(4)?.unwrap_or(0),  // input_tokens
            r.get::<_, Option<i64>>(5)?.unwrap_or(0),  // output_tokens
            r.get::<_, Option<i64>>(6)?.unwrap_or(0),  // cache_read_tokens
            r.get::<_, Option<i64>>(7)?.unwrap_or(0),  // cache_write_tokens
            r.get::<_, Option<i64>>(8)?.unwrap_or(0),  // reasoning_tokens
            r.get::<_, Option<i64>>(9)?.unwrap_or(0),  // api_call_count
            r.get::<_, Option<String>>(10)?,           // cwd
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
        let (dedup_key, id, model, timestamp, input, output, cache_read, cache_write, reasoning, api_call_count, cwd) =
            match row {
                Ok(v) => v,
                Err(_) => {
                    skipped += 1;
                    continue;
                }
            };

        // Nothing in the row's lineage dates it: the tokens are real but
        // unplaceable, and a Record the Ledger cannot date shows in no window.
        // Same call goose makes when a session carries neither timestamp.
        let Some(timestamp) = timestamp.filter(|value| *value > 0.0) else {
            skipped += 1;
            continue;
        };

        let total = input
            .saturating_add(output)
            .saturating_add(cache_read)
            .saturating_add(cache_write)
            .saturating_add(reasoning);
        if input < 0
            || output < 0
            || cache_read < 0
            || cache_write < 0
            || reasoning < 0
            || total <= 0
        {
            skipped += 1; // zero-token observations are not usage records
            continue;
        }

        // api_call_count is authoritative; force at least 1 when tokens exist.
        let api_calls = if api_call_count > 0 { api_call_count } else { 1 };

        let project = match cwd {
            Some(p) if !p.is_empty() => Some(p),
            _ => None,
        };

        events.push(UsageEvent {
            dedup_key,
            source: "hermes".to_string(),
            timestamp: timestamp as i64,           // truncate fractional seconds
            // `unknown` is Hermes' own placeholder for a route it could not
            // name, so it is an absent Model, not a Model named "unknown"
            // (same reading codex.rs takes).
            model: model.filter(|model| {
                let model = model.trim();
                !model.is_empty() && !model.eq_ignore_ascii_case("unknown")
            }),
            project,
            api_calls,
            input_tokens: input,
            output_tokens: output.saturating_add(reasoning), // reasoning folds into output
            cache_read_tokens: cache_read,
            cache_write_5m_tokens: cache_write,     // single Hermes bucket -> 5m
            cache_write_1h_tokens: 0,
            source_file: hermes_db.display().to_string(),
            session_id: Some(id),
            reasoning_tokens: Some(reasoning),
            ctx: Default::default(),
        });
    }

    // The per-Session rollup this adapter used to book stands under
    // `hermes:<id>`, which the per-(Model, task) keys above no longer collide
    // with — so it would double-count beside them. Dropped before the insert,
    // so a failure here leaves the old rows and books no new ones (and leaves
    // the file state unwritten, so the next scan retries) rather than counting
    // a session twice. Only Sessions this scan actually books are superseded: a
    // Session Hermes has since purged keeps the history nothing can re-derive.
    if per_model {
        let superseded: Vec<String> = events
            .iter()
            .filter_map(|event| event.session_id.as_deref())
            .map(|id| format!("hermes:{id}"))
            .collect();
        if let Err(e) = delete_events(conn, &superseded) {
            return SourceScanResult {
                error: Some(format!("hermes: superseding the per-session rollup failed: {e}")),
                ..Default::default()
            };
        }
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

    SourceScanResult { events_inserted: inserted, lines_skipped: skipped, ..Default::default() }
}

/// The column names of `table` in an opened Artifact, empty when the table is
/// absent. Hermes heals its own schema forward, so which columns exist is a
/// fact about the install's version, not about this store being broken.
fn artifact_columns(ro: &Connection, table: &str) -> Vec<String> {
    let Ok(mut stmt) = ro.prepare("SELECT name FROM pragma_table_info(?1)") else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([table], |r| r.get::<_, String>(0)) else {
        return Vec::new();
    };
    rows.flatten().collect()
}

fn discover_databases(primary: &Path) -> Vec<PathBuf> {
    let mut databases = Vec::new();
    let mut roots = Vec::new();
    let state_filename = source_catalog::artifact_filename("hermes", "state");

    add_unique_path(&mut databases, primary.to_path_buf());
    if let Some(home) = primary.parent() {
        add_unique_path(&mut roots, home.to_path_buf());
        if home.parent().and_then(Path::file_name) == Some(OsStr::new("profiles")) {
            if let Some(root) = home.parent().and_then(Path::parent) {
                add_unique_path(&mut roots, root.to_path_buf());
            }
        }
    }

    for root in roots {
        add_unique_path(&mut databases, root.join(&state_filename));
        let profiles_root = root.join("profiles");
        let mut profiles = std::fs::read_dir(profiles_root)
            .ok()
            .into_iter()
            .flat_map(|entries| entries.filter_map(Result::ok))
            .filter_map(|entry| {
                entry
                    .file_type()
                    .ok()
                    .filter(|kind| kind.is_dir())
                    .map(|_| entry.path())
            })
            .collect::<Vec<_>>();
        profiles.sort();
        for profile in profiles {
            add_unique_path(&mut databases, profile.join(&state_filename));
        }
    }

    databases
}

fn add_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    let normalized = std::fs::canonicalize(&path).unwrap_or_else(|_| normalize_path(&path));
    if !paths.iter().any(|existing| existing == &normalized) {
        paths.push(normalized);
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => match normalized.components().next_back() {
                Some(std::path::Component::Normal(_)) => {
                    normalized.pop();
                }
                _ => normalized.push(component.as_os_str()),
            },
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
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
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
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
    fn discovers_profile_databases_and_preserves_profile_boundaries() {
        let hermes_dir = tempdir().unwrap();
        let hermes_db = hermes_dir.path().join("state.db");
        let profile_db = hermes_dir.path().join("profiles/coder/state.db");
        build_hermes_db(&hermes_db);
        build_hermes_db(&profile_db);
        Connection::open(&profile_db)
            .unwrap()
            .execute("UPDATE sessions SET id = 'coder-' || id", [])
            .unwrap();

        let app_dir = tempdir().unwrap();
        let mut conn = open_db(&app_dir.path().join("tokenledger.db")).unwrap();

        let res = scan_hermes(&mut conn, &hermes_db);
        assert!(res.error.is_none(), "unexpected error: {:?}", res.error);
        assert_eq!(res.events_inserted, 4);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events WHERE source = 'hermes'", [], |r| r.get::<_, i64>(0)).unwrap(),
            4,
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(DISTINCT source_file) FROM events WHERE source = 'hermes'", [], |r| r.get::<_, i64>(0)).unwrap(),
            2,
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(DISTINCT source) FROM events WHERE source = 'hermes'", [], |r| r.get::<_, i64>(0)).unwrap(),
            1,
        );

        // A path alias that resolves to the same root cannot manufacture new
        // Usage Records or re-scan the profile databases.
        let alias = hermes_dir.path().join("profiles/../state.db");
        let res = scan_hermes(&mut conn, &alias);
        assert!(res.error.is_none(), "unexpected error: {:?}", res.error);
        assert_eq!(res.events_inserted, 0);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events WHERE source = 'hermes'", [], |r| r.get::<_, i64>(0)).unwrap(),
            4,
        );
    }

    #[test]
    fn null_model_and_zero_token_observations_are_conservative() {
        let hermes_dir = tempdir().unwrap();
        let hermes_db = hermes_dir.path().join("state.db");
        build_hermes_db(&hermes_db);
        {
            let src = Connection::open(&hermes_db).unwrap();
            src.execute(
                "INSERT INTO sessions VALUES ('unattributed',NULL,1780310783.7,100,20,0,0,0,4,'')",
                [],
            )
            .unwrap();
            src.execute(
                "INSERT INTO sessions VALUES ('zero-but-called','qwen-35b',1780310784.7,0,0,0,0,0,4,'')",
                [],
            )
            .unwrap();
        }

        let app_dir = tempdir().unwrap();
        let mut conn = open_db(&app_dir.path().join("tokenledger.db")).unwrap();
        let res = scan_hermes(&mut conn, &hermes_db);
        assert!(res.error.is_none(), "unexpected error: {:?}", res.error);
        assert_eq!(res.events_inserted, 3);
        let (model, calls): (Option<String>, i64) = conn
            .query_row(
                "SELECT model, api_calls FROM events WHERE dedup_key = 'hermes:unattributed'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(model, None);
        assert_eq!(calls, 4);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events WHERE dedup_key = 'hermes:zero-but-called'", [], |r| r.get::<_, i64>(0)).unwrap(),
            0,
        );
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
    fn missing_db_is_a_quiet_empty_source() {
        let app_dir = tempdir().unwrap();
        let mut conn = open_db(&app_dir.path().join("tokenledger.db")).unwrap();

        let res = scan_hermes(&mut conn, Path::new("/nonexistent/hermes/state.db"));
        assert!(res.error.is_none());
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

    /// Add the per-(Model, task) usage table Hermes schema v20 introduced, with
    /// the `task` column v22 added. Column list and PK match the store's own
    /// DDL so the fixture cannot drift into a shape Hermes never writes.
    fn add_model_usage_table(path: &Path, with_task: bool) {
        let task_column = if with_task { "task TEXT NOT NULL DEFAULT ''," } else { "" };
        let task_key = if with_task { ", task" } else { "" };
        Connection::open(path)
            .unwrap()
            .execute_batch(&format!(
                "CREATE TABLE session_model_usage (
                    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                    model TEXT NOT NULL,
                    billing_provider TEXT NOT NULL DEFAULT '',
                    billing_base_url TEXT NOT NULL DEFAULT '',
                    billing_mode TEXT NOT NULL DEFAULT '',
                    {task_column}
                    api_call_count INTEGER NOT NULL DEFAULT 0,
                    input_tokens INTEGER NOT NULL DEFAULT 0,
                    output_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
                    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                    first_seen REAL,
                    last_seen REAL,
                    PRIMARY KEY (session_id, model, billing_provider, billing_base_url,
                                 billing_mode{task_key})
                );"
            ))
            .unwrap();
    }

    /// s1's real accounting: the main loop switched Model mid-session, and a
    /// compression call spent tokens the `sessions` rollup never sees. s1's
    /// `started_at` is 1780287300 — every `last_seen` here is later.
    fn seed_model_usage(path: &Path) {
        let src = Connection::open(path).unwrap();
        src.execute_batch(
            "INSERT INTO session_model_usage
                 (session_id, model, billing_provider, billing_base_url, billing_mode, task,
                  api_call_count, input_tokens, output_tokens, cache_read_tokens,
                  cache_write_tokens, reasoning_tokens, first_seen, last_seen)
             VALUES
                 ('s1','model-a','custom','http://x/v1','','',
                  20, 1000, 100, 0, 0, 0, 1780287300.5, 1781000000.75),
                 ('s1','model-b','custom','http://x/v1','','',
                  5, 500, 50, 0, 0, 0, 1780290000.0, 1780290000.0),
                 ('s1','model-c','custom','http://x/v1','','compression',
                  1, 7, 3, 0, 0, 0, 1781000500.0, 1781000500.0);",
        )
        .unwrap();
    }

    #[test]
    fn per_model_usage_books_auxiliary_tokens_at_their_own_last_seen() {
        let hermes_dir = tempdir().unwrap();
        let hermes_db = hermes_dir.path().join("state.db");
        build_hermes_db(&hermes_db);
        add_model_usage_table(&hermes_db, true);
        seed_model_usage(&hermes_db);

        let app_dir = tempdir().unwrap();
        let mut conn = open_db(&app_dir.path().join("tokenledger.db")).unwrap();
        let res = scan_hermes(&mut conn, &hermes_db);
        assert!(res.error.is_none(), "unexpected error: {:?}", res.error);

        // Three Records for one Session: two main-loop Models plus the
        // auxiliary call. The rollup could only ever have said "one".
        let (rows, models, tokens): (i64, i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), COUNT(DISTINCT model),
                        SUM(input_tokens + output_tokens + cache_read_tokens
                            + cache_write_5m_tokens + cache_write_1h_tokens)
                   FROM events WHERE session_id = 's1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((rows, models), (3, 3));
        assert_eq!(tokens, 1000 + 100 + 500 + 50 + 7 + 3);

        // The date fix: the busiest Model's Record lands on its last_seen, not
        // on the Session's started_at (1780287300) — that is the whole bug.
        let ts: i64 = conn
            .query_row(
                "SELECT timestamp FROM events WHERE session_id = 's1' AND model = 'model-a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ts, 1781000000);

        // The auxiliary Record is a Record in its own right, dated its own way.
        let (aux_ts, aux_out, aux_calls): (i64, i64, i64) = conn
            .query_row(
                "SELECT timestamp, output_tokens, api_calls
                   FROM events WHERE session_id = 's1' AND model = 'model-c'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((aux_ts, aux_out, aux_calls), (1781000500, 3, 1));
    }

    #[test]
    fn per_model_rows_supersede_the_rollup_they_replace() {
        let hermes_dir = tempdir().unwrap();
        let hermes_db = hermes_dir.path().join("state.db");
        build_hermes_db(&hermes_db);

        let app_dir = tempdir().unwrap();
        let mut conn = open_db(&app_dir.path().join("tokenledger.db")).unwrap();

        // A Ledger built before this adapter read the per-Model table.
        scan_hermes(&mut conn, &hermes_db);
        let legacy: i64 = conn
            .query_row("SELECT COUNT(*) FROM events WHERE dedup_key = 'hermes:s1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(legacy, 1, "precondition: the rollup Record is booked");

        // Hermes migrates the store forward; the next scan sees the real rows.
        add_model_usage_table(&hermes_db, true);
        seed_model_usage(&hermes_db);
        bump_mtime(&hermes_db);
        let res = scan_hermes(&mut conn, &hermes_db);
        assert!(res.error.is_none(), "unexpected error: {:?}", res.error);

        // s1's rollup Record is gone, so its tokens are counted once, not twice.
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events WHERE dedup_key = 'hermes:s1'", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0,
        );
        assert_eq!(
            conn.query_row(
                "SELECT SUM(input_tokens + output_tokens + cache_read_tokens
                            + cache_write_5m_tokens + cache_write_1h_tokens)
                   FROM events WHERE session_id = 's1'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1000 + 100 + 500 + 50 + 7 + 3,
        );

        // s3 has tokens in the rollup and no per-Model row: its Record stays.
        // Superseding it would delete history nothing can re-derive.
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events WHERE dedup_key = 'hermes:s3'", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1,
        );
    }

    #[test]
    fn a_store_predating_the_task_column_still_books_per_model() {
        let hermes_dir = tempdir().unwrap();
        let hermes_db = hermes_dir.path().join("state.db");
        build_hermes_db(&hermes_db);
        add_model_usage_table(&hermes_db, false);
        Connection::open(&hermes_db)
            .unwrap()
            .execute_batch(
                "INSERT INTO session_model_usage
                     (session_id, model, billing_provider, billing_base_url, billing_mode,
                      api_call_count, input_tokens, output_tokens, cache_read_tokens,
                      cache_write_tokens, reasoning_tokens, first_seen, last_seen)
                 VALUES ('s1','model-a','custom','http://x/v1','',
                         20, 1000, 100, 0, 0, 0, 1780287300.5, 1781000000.75);",
            )
            .unwrap();

        let app_dir = tempdir().unwrap();
        let mut conn = open_db(&app_dir.path().join("tokenledger.db")).unwrap();
        let res = scan_hermes(&mut conn, &hermes_db);
        assert!(res.error.is_none(), "unexpected error: {:?}", res.error);

        let (model, ts): (Option<String>, i64) = conn
            .query_row(
                "SELECT model, timestamp FROM events WHERE session_id = 's1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((model, ts), (Some("model-a".to_string()), 1781000000));
    }

    #[test]
    fn an_undatable_usage_row_is_skipped_not_booked_at_the_epoch() {
        let hermes_dir = tempdir().unwrap();
        let hermes_db = hermes_dir.path().join("state.db");
        build_hermes_db(&hermes_db);
        add_model_usage_table(&hermes_db, true);
        // No first_seen, no last_seen, and an orphan session_id so `started_at`
        // cannot supply one either.
        Connection::open(&hermes_db)
            .unwrap()
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                 INSERT INTO session_model_usage
                     (session_id, model, billing_provider, billing_base_url, billing_mode, task,
                      api_call_count, input_tokens, output_tokens, cache_read_tokens,
                      cache_write_tokens, reasoning_tokens, first_seen, last_seen)
                 VALUES ('ghost','model-a','','','','', 3, 900, 90, 0, 0, 0, NULL, NULL);",
            )
            .unwrap();

        let app_dir = tempdir().unwrap();
        let mut conn = open_db(&app_dir.path().join("tokenledger.db")).unwrap();
        let res = scan_hermes(&mut conn, &hermes_db);
        assert!(res.error.is_none(), "unexpected error: {:?}", res.error);
        assert_eq!(res.events_inserted, 0);
        assert_eq!(res.lines_skipped, 1);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events WHERE source = 'hermes'", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "a Record with no date would sit at the epoch, inside no window",
        );
    }

    #[test]
    fn hermes_own_unknown_route_placeholder_is_an_absent_model() {
        let hermes_dir = tempdir().unwrap();
        let hermes_db = hermes_dir.path().join("state.db");
        build_hermes_db(&hermes_db);
        add_model_usage_table(&hermes_db, true);
        Connection::open(&hermes_db)
            .unwrap()
            .execute_batch(
                "INSERT INTO session_model_usage
                     (session_id, model, billing_provider, billing_base_url, billing_mode, task,
                      api_call_count, input_tokens, output_tokens, cache_read_tokens,
                      cache_write_tokens, reasoning_tokens, first_seen, last_seen)
                 VALUES ('s1','unknown','','','','', 2, 800, 80, 0, 0, 0, 1781000000.0, 1781000000.0);",
            )
            .unwrap();

        let app_dir = tempdir().unwrap();
        let mut conn = open_db(&app_dir.path().join("tokenledger.db")).unwrap();
        assert!(scan_hermes(&mut conn, &hermes_db).error.is_none());
        assert_eq!(
            conn.query_row(
                "SELECT model FROM events WHERE session_id = 's1'",
                [],
                |r| r.get::<_, Option<String>>(0)
            )
            .unwrap(),
            None,
            "an unnamed route must not be priced as a Model called \"unknown\"",
        );
    }

    #[test]
    fn a_parser_version_bump_rereads_a_store_that_never_changed() {
        let hermes_dir = tempdir().unwrap();
        let hermes_db = hermes_dir.path().join("state.db");
        build_hermes_db(&hermes_db);
        add_model_usage_table(&hermes_db, true);
        seed_model_usage(&hermes_db);

        let app_dir = tempdir().unwrap();
        let mut conn = open_db(&app_dir.path().join("tokenledger.db")).unwrap();
        assert_eq!(scan_hermes(&mut conn, &hermes_db).events_inserted, 3);

        // A Ledger the previous parser booked: same store, byte-for-byte, with
        // the same mtime — only the stored version stamp is older. Nothing here
        // touches the Hermes file, or the size+mtime check would fire and the
        // stamp would carry no weight in this test.
        conn.execute("DELETE FROM events WHERE source = 'hermes'", []).unwrap();
        conn.execute("UPDATE scanned_files SET byte_offset = 0", []).unwrap();

        let res = scan_hermes(&mut conn, &hermes_db);
        assert!(res.error.is_none(), "unexpected error: {:?}", res.error);
        assert_eq!(
            res.events_inserted, 3,
            "an unchanged store must be re-read on a version bump, not skipped",
        );
    }
}
