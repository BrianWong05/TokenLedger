//! Qoder Source adapter.
//!
//! One Source covers two Artifact families:
//!
//! 1. Qoder IDE (a VS Code–based AI coding IDE) stores every chat message in a
//!    SQLite database at
//!    `~/Library/Application Support/QoderCN/SharedClientCache/cache/db/local.db`
//!    (macOS; equivalent paths on Linux/Windows). The IDE ships as two editions
//!    — QoderCN and the plain-Qoder edition (`…/Qoder/…`), which may coexist on
//!    one machine; both databases carry the same schema and are scanned. The
//!    `chat_message` table
//!    carries one row per message; assistant rows hold a `token_info` JSON blob
//!    (`prompt_tokens`, `completion_tokens`, `cached_tokens`) and a
//!    `model_info` JSON blob (`model_key`). Each usage-bearing assistant row is
//!    one Usage Record (one Request), deduplicating on the row `id`.
//!
//!    Cache semantics (ADR-0001): `prompt_tokens` **includes** the cache read
//!    (`cached_tokens`), so Input Tokens = `prompt_tokens − cached_tokens`.
//!    The Artifact exposes no cache-write or reasoning figures, and no Context
//!    tiers. `max_input_tokens` is a context-window size, never usage, and is
//!    ignored. The message `content` is never read — only numeric usage and
//!    identity fields.
//!
//!    Subagent messages (`agent_sub_*` sessions) log no `model_info`; their
//!    Model falls back to the parent session's logged Model via
//!    `chat_session.parent_session_id` — one hop away in the same Artifact.
//!    Without the link a Record stays model-less (Unattributed), never fails.
//!
//! 2. The Qoder CLI products (`~/.qoder/projects`, `~/.qoder-cli/projects`,
//!    `~/.qoder-cn/projects`) write Claude-Code-shaped JSONL transcripts. Each
//!    usage-bearing `assistant` line is one Usage Record, deduplicating on
//!    `message.id`. Token figures come from the shared `claude_shaped_usage`
//!    rule (adapters/mod.rs) — `input_tokens` is fresh input and
//!    `cache_creation` splits into the ephemeral 5m/1h buckets. The IDE row
//!    ids and the CLI `chatcmpl-*` message ids come from different issuers and
//!    different conversations, so both share the `qoder:` dedup namespace
//!    without collision (ADR-0014); a true collision would be the same
//!    request, where merging is correct. `~/.qoder-cli` carries no token
//!    usage today; a missing root is scanned quietly (ADR-0015).
//!
//! Message content is never read in either family — only numeric usage and
//! identity fields.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::adapters::{
    absolute_project, claude_shaped_usage, file_state_of, find_jsonl, normalize_epoch,
    rollup_worktree, unchanged, upsert_events_count,
};
use crate::db::{insert_events_keep_max_output, set_file_state};
use crate::types::{CtxTokens, FileState, SourceScanResult, UsageEvent};

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

// Qoder logs internal routing aliases, never the backing Model; the catalogs
// price only the real name. Translate on ingest so Cost resolves (the alias
// itself would be Unpriced forever). Mapping confirmed by the user; the
// Artifacts carry no evidence of their own.
const MODEL_ALIASES: &[(&str, &str)] = &[("qmodel_38max", "qwen3.8-max")];

fn translate_model(model: Option<String>) -> Option<String> {
    model.map(|m| {
        MODEL_ALIASES
            .iter()
            .find(|(alias, _)| *alias == m)
            .map(|(_, real)| real.to_string())
            .unwrap_or(m)
    })
}

// Bump to force a full re-parse of every Qoder CLI transcript when the parser
// changes (the byte-offset slot carries it through `unchanged`).
// 2: translate routing aliases (qmodel_38max -> qwen3.8-max).
// 3: re-parse so the keep-max Latest policy rewrites already-booked models.
const TRANSCRIPT_PARSER_VERSION: i64 = 3;

#[derive(Default)]
struct DatabaseScan {
    events: Vec<UsageEvent>,
    lines_skipped: u64,
}

/// Scan every Qoder IDE database (the QoderCN and plain-Qoder editions may
/// coexist on one machine) and every CLI transcript root, merging all Artifact
/// families into one Source result (errors join with `"; "`).
pub fn scan_qoder(
    conn: &mut Connection,
    databases: &[PathBuf],
    cli_projects: &[PathBuf],
) -> SourceScanResult {
    let mut result = SourceScanResult::default();
    for database in databases {
        merge_result(&mut result, scan_qoder_database(conn, database));
    }
    merge_result(&mut result, scan_qoder_cli(conn, cli_projects));
    result
}

fn merge_result(into: &mut SourceScanResult, other: SourceScanResult) {
    into.events_inserted += other.events_inserted;
    into.lines_skipped += other.lines_skipped;
    if let Some(error) = other.error {
        match into.error.as_mut() {
            Some(previous) => {
                previous.push_str("; ");
                previous.push_str(&error);
            }
            None => into.error = Some(error),
        }
    }
}

/// Scan the IDE's local chat database. Each usage-bearing assistant message
/// is one Usage Record; rows without `token_info` or with all-zero tokens
/// produce no Record. Idempotent: re-scanning a stable database books nothing
/// new (dedup on `qoder:<row id>`).
fn scan_qoder_database(conn: &mut Connection, database: &Path) -> SourceScanResult {
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

/// Scan the CLI transcript roots (`~/.qoder/projects`, `~/.qoder-cli/projects`,
/// `~/.qoder-cn/projects`). Missing roots are scanned quietly; each
/// usage-bearing `assistant` line is one Usage Record, deduplicating on
/// `qoder:<message.id>`.
fn scan_qoder_cli(conn: &mut Connection, roots: &[PathBuf]) -> SourceScanResult {
    let mut result = SourceScanResult::default();
    let mut files = Vec::new();
    for root in roots {
        find_jsonl(root, &mut files);
    }
    files.sort();
    for path in files {
        let mut file_result = SourceScanResult::default();
        match scan_file(conn, &path) {
            Ok((inserted, skipped)) => {
                file_result.events_inserted = inserted;
                file_result.lines_skipped = skipped;
            }
            Err(error) => file_result.error = Some(error),
        }
        merge_result(&mut result, file_result);
    }
    result
}

fn scan_file(conn: &mut Connection, path: &Path) -> Result<(u64, u64), String> {
    let state = FileState {
        byte_offset: TRANSCRIPT_PARSER_VERSION,
        ..file_state_of(path)
    };
    if unchanged(conn, path, &state) {
        return Ok((0, 0));
    }

    let source_file = path.to_string_lossy().to_string();
    let content = fs::read_to_string(path)
        .map_err(|error| format!("{SOURCE}: read {}: {error}", path.display()))?;
    let parsed = parse_file(&content, path, &source_file);
    // A changed file is reparsed from the top; keep_max_output re-inserts the
    // same dedup keys without double-booking and raises output on a conflict.
    let inserted = insert_events_keep_max_output(conn, &parsed.events)
        .map_err(|error| format!("{SOURCE}: insert {}: {error}", path.display()))?;
    set_file_state(conn, &source_file, state)
        .map_err(|error| format!("{SOURCE}: metadata {}: {error}", path.display()))?;
    Ok((inserted, parsed.lines_skipped))
}

struct ParsedTranscript {
    events: Vec<UsageEvent>,
    lines_skipped: u64,
}

fn parse_file(content: &str, path: &Path, source_file: &str) -> ParsedTranscript {
    // Consume only complete newline-terminated lines; a trailing partial line
    // is left for the next scan (it is not malformed, just incomplete).
    let consumed = content.rfind('\n').map(|i| i + 1).unwrap_or(0);

    // ~/.qoder*/projects/<encoded-dir>/<session>.jsonl: the encoded dir is the
    // file's parent basename. Used verbatim (never decoded — provably lossy)
    // as the project fallback when a line has no `cwd` (claude precedent).
    let encoded_dir = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file_stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut events = Vec::new();
    let mut lines_skipped: u64 = 0;
    for line in content[..consumed].lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                lines_skipped += 1;
                continue;
            }
        };
        if let Some(ev) = parse_line_event(&v, source_file, &encoded_dir, &file_stem) {
            events.push(ev);
        }
    }

    ParsedTranscript {
        events,
        lines_skipped,
    }
}

fn parse_line_event(
    v: &Value,
    source_file: &str,
    encoded_dir: &str,
    file_stem: &str,
) -> Option<UsageEvent> {
    if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
        return None;
    }
    let msg = &v["message"];

    let usage = claude_shaped_usage(msg)?;

    let id = msg.get("id").and_then(|i| i.as_str()).filter(|i| !i.is_empty())?;
    let dedup_key = format!("{SOURCE}:{id}");

    let timestamp = v
        .get("timestamp")
        .and_then(|t| t.as_str())
        .and_then(crate::time::iso_to_epoch)?;

    let model = translate_model(
        msg.get("model")
            .and_then(|m| m.as_str())
            .filter(|m| !m.is_empty())
            .map(str::to_owned),
    );

    let project = match v.get("cwd").and_then(|c| c.as_str()) {
        Some(cwd) => Some(rollup_worktree(cwd)),
        // fallback: raw dash-encoded dir name, verbatim (not decoded)
        None if !encoded_dir.is_empty() => Some(encoded_dir.to_string()),
        None => None,
    };

    let session_id = v
        .get("sessionId")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| (!file_stem.is_empty()).then(|| file_stem.to_string()));

    Some(UsageEvent {
        dedup_key,
        source: SOURCE.to_string(),
        timestamp,
        model,
        project,
        api_calls: 1,
        input_tokens: usage.input,
        output_tokens: usage.output,
        cache_read_tokens: usage.cache_read,
        cache_write_5m_tokens: usage.cache_write_5m,
        cache_write_1h_tokens: usage.cache_write_1h,
        source_file: source_file.to_string(),
        session_id,
        reasoning_tokens: None,
        // No Context attribution: the catalog reports `context: false`.
        ctx: CtxTokens::default(),
    })
}

fn scan_database(path: &Path) -> Result<DatabaseScan, String> {
    let source_file = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let database = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("{SOURCE}: database open failed: {error}"))?;
    let _ = database.busy_timeout(std::time::Duration::from_millis(5000));
    ensure_schema(&database)?;

    let mut scan = DatabaseScan::default();
    // Subagent messages log no model_info; their parent session's does.
    let model_fallback = subagent_model_fallbacks(&database);
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

        let session = session_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);

        let model = translate_model(
            model_info
                .as_deref()
                .and_then(parse_model)
                .filter(|m| !m.is_empty())
                .or_else(|| {
                    session
                        .as_deref()
                        .and_then(|s| model_fallback.get(s))
                        .cloned()
                }),
        );

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

/// The fallback Model for sessions whose messages log no `model_info`: Qoder
/// omits it on subagent conversations (`agent_sub_*`), but every such session
/// carries a `parent_session_id` whose own messages DO log it — one hop away,
/// in the same Artifact. Keyed by subagent session id. Best-effort: a missing
/// `chat_session` table or column yields an empty map rather than an error,
/// and the Records simply stay model-less as before.
fn subagent_model_fallbacks(
    database: &Connection,
) -> std::collections::HashMap<String, String> {
    // Each session's own logged Model, first observation wins. ORDER BY id
    // makes that deterministic and matches the main scan's row order.
    let mut session_model: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    if let Ok(mut stmt) = database.prepare(
        "SELECT session_id, model_info FROM chat_message \
         WHERE model_info IS NOT NULL AND model_info != '' \
         ORDER BY id",
    ) {
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                ))
            })
            .into_iter()
            .flatten()
            .flatten();
        for (session_id, model_info) in rows {
            if let (Some(id), Some(model)) = (session_id, parse_model(&model_info)) {
                session_model.entry(id).or_insert(model);
            }
        }
    }

    // Subagent session -> its parent's Model.
    let mut fallback = std::collections::HashMap::new();
    if let Ok(mut stmt) = database.prepare(
        "SELECT session_id, parent_session_id FROM chat_session \
         WHERE parent_session_id IS NOT NULL AND parent_session_id != ''",
    ) {
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .into_iter()
            .flatten()
            .flatten();
        for (session_id, parent_id) in rows {
            if let Some(model) = session_model.get(&parent_id) {
                fallback.insert(session_id, model.clone());
            }
        }
    }
    fallback
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
/// `model_key`; `translate_model` then maps routing aliases to real names.
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
        let result = scan_qoder(&mut ledger, std::slice::from_ref(&db_path), &[]);
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
        assert_eq!(rows[0].1.as_deref(), Some("qwen3.8-max")); // alias translated
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
        let result = scan_qoder(&mut ledger, std::slice::from_ref(&db_path), &[]);
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
        let first = scan_qoder(&mut ledger, std::slice::from_ref(&db_path), &[]);
        assert_eq!(first.events_inserted, 1);
        let second = scan_qoder(&mut ledger, std::slice::from_ref(&db_path), &[]);
        assert_eq!(second.events_inserted, 0, "rescan books nothing new");

        let model: Option<String> = ledger
            .query_row("SELECT model FROM events WHERE source = 'qoder'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(model, None, "missing model_info yields no model");
    }

    #[test]
    fn subagent_messages_inherit_the_parent_sessions_model() {
        // Qoder logs no model_info on agent_sub conversations; the parent
        // session's messages carry it, so the Model is one hop away.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("local.db");
        create_database(&db_path);
        let db = Connection::open(&db_path).unwrap();
        db.execute_batch(
            "CREATE TABLE chat_session (
                session_id VARCHAR(64) PRIMARY KEY,
                parent_session_id VARCHAR(64)
            );",
        )
        .unwrap();
        // Parent quest session logs the alias; subagent session does not.
        insert_message(
            &db,
            "p1",
            Some("task-parent.session.execution"),
            "assistant",
            Some(
                r#"{"prompt_tokens":100,"completion_tokens":10,"cached_tokens":0,"max_input_tokens":1000000}"#,
            ),
            Some(r#"{"model_key":"qmodel_38max"}"#),
            1_786_112_276_027i64,
        );
        insert_message(
            &db,
            "s1",
            Some("agent-sub-1"),
            "assistant",
            Some(
                r#"{"prompt_tokens":50,"completion_tokens":5,"cached_tokens":0,"max_input_tokens":1000000}"#,
            ),
            None, // no model_info on subagent messages
            1_786_112_276_028i64,
        );
        // Orphan session: no parent link -> stays model-less.
        insert_message(
            &db,
            "o1",
            Some("orphan-session"),
            "assistant",
            Some(
                r#"{"prompt_tokens":30,"completion_tokens":3,"cached_tokens":0,"max_input_tokens":1000000}"#,
            ),
            None,
            1_786_112_276_029i64,
        );
        db.execute(
            "INSERT INTO chat_session (session_id, parent_session_id) VALUES \
             ('agent-sub-1', 'task-parent.session.execution'), \
             ('task-parent.session.execution', NULL)",
            [],
        )
        .unwrap();
        drop(db);

        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        let result = scan_qoder(&mut ledger, std::slice::from_ref(&db_path), &[]);
        assert_eq!(result.events_inserted, 3);
        assert!(result.error.is_none());

        let model_of = |key: &str| -> Option<String> {
            ledger
                .query_row(
                    "SELECT model FROM events WHERE dedup_key = ?1",
                    [key],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert_eq!(model_of("qoder:p1").as_deref(), Some("qwen3.8-max"));
        assert_eq!(
            model_of("qoder:s1").as_deref(),
            Some("qwen3.8-max"),
            "subagent inherits the parent's Model, translated"
        );
        assert_eq!(model_of("qoder:o1"), None, "no parent link: model-less");
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

        let result = scan_qoder(&mut ledger, std::slice::from_ref(&db_path), &[]);
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
        let result = scan_qoder(&mut ledger, &[tmp.path().join("does-not-exist.db")], &[]);
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
        scan_qoder(&mut ledger, std::slice::from_ref(&db_path), &[]);

        let durable = fs::read(tmp.path().join("ledger.db")).unwrap();
        assert!(!durable
            .windows("QODER_PRIVATE_PROMPT_MARKER".len())
            .any(|w| w == b"QODER_PRIVATE_PROMPT_MARKER"));
    }

    // --- CLI transcripts (Claude-Code-shaped JSONL) ---

    /// A real-shape assistant line: Anthropic-style usage with the ephemeral
    /// cache-write split, `chatcmpl-*` message id, ISO timestamp, cwd.
    fn cli_assistant_line(id: &str, session: &str, input: i64, cache_read: i64, output: i64) -> String {
        format!(
            r#"{{"type":"assistant","uuid":"u-{id}","timestamp":"2026-08-07T16:53:21.465Z","message":{{"id":"{id}","model":"qmodel_38max","role":"assistant","usage":{{"input_tokens":{input},"output_tokens":{output},"cache_read_input_tokens":{cache_read},"cache_creation_input_tokens":7,"cache_creation":{{"ephemeral_5m_input_tokens":4,"ephemeral_1h_input_tokens":3}}}}}},"cwd":"/Users/dev/projects/alpha","sessionId":"{session}"}}"#,
        )
    }

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn cli_assistant_lines_become_records_with_cache_split() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("projects");
        write(
            &root,
            "-Users-dev-projects-alpha/qcli-sess.jsonl",
            &format!(
                "{}\n{}\n",
                cli_assistant_line("chatcmpl-q1", "qcli-sess", 420, 24618, 470),
                // Non-usage line types never become Records.
                r#"{"type":"user","uuid":"u1","timestamp":"2026-08-07T16:53:20.000Z","message":{"role":"user","content":"hi"},"sessionId":"qcli-sess"}"#,
            ),
        );
        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        let result = scan_qoder(&mut ledger, &[], std::slice::from_ref(&root));
        assert_eq!(result.events_inserted, 1);
        assert!(result.error.is_none());

        let row: (Option<String>, Option<String>, Option<String>, i64, i64, i64, i64, i64) =
            ledger
                .query_row(
                    "SELECT model, project, session_id, input_tokens, output_tokens, \
                            cache_read_tokens, cache_write_5m_tokens, cache_write_1h_tokens \
                     FROM events WHERE dedup_key = 'qoder:chatcmpl-q1'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)),
                )
                .unwrap();
        assert_eq!(row.0.as_deref(), Some("qwen3.8-max")); // alias translated
        assert_eq!(row.1.as_deref(), Some("/Users/dev/projects/alpha"));
        assert_eq!(row.2.as_deref(), Some("qcli-sess"));
        assert_eq!(row.3, 420); // fresh input; cache read is separate
        assert_eq!(row.4, 470);
        assert_eq!(row.5, 24618);
        assert_eq!(row.6, 4); // ephemeral 5m/1h split preserved
        assert_eq!(row.7, 3);

        let ts: i64 = ledger
            .query_row(
                "SELECT timestamp FROM events WHERE dedup_key = 'qoder:chatcmpl-q1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ts, 1786121601); // 2026-08-07T16:53:21Z
    }

    #[test]
    fn zero_usage_and_non_assistant_lines_produce_no_records() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("projects");
        write(
            &root,
            "proj/s.jsonl",
            concat!(
                // All-zero usage (a synthetic placeholder) is not a Record.
                r#"{"type":"assistant","timestamp":"2026-08-07T16:53:21.465Z","message":{"id":"chatcmpl-zero","model":"qmodel_38max","usage":{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
                "\n",
                r#"{"type":"thinking","timestamp":"2026-08-07T16:53:22.000Z","message":{"id":"t1"}}"#,
                "\n",
            ),
        );
        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        let result = scan_qoder(&mut ledger, &[], &[root]);
        assert_eq!(result.events_inserted, 0);
        assert_eq!(result.lines_skipped, 0, "well-formed lines are never 'skipped'");
        assert!(result.error.is_none());
    }

    #[test]
    fn session_falls_back_to_file_stem_and_project_to_encoded_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("projects");
        // No sessionId and no cwd on the line.
        write(
            &root,
            "-Users-dev-projects-gamma/stem-sess.jsonl",
            r#"{"type":"assistant","timestamp":"2026-08-07T16:53:21.465Z","message":{"id":"chatcmpl-fb","model":"qmodel_38max","usage":{"input_tokens":10,"output_tokens":2,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}
"#,
        );
        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        let result = scan_qoder(&mut ledger, &[], &[root]);
        assert_eq!(result.events_inserted, 1);

        let (project, session): (Option<String>, Option<String>) = ledger
            .query_row(
                "SELECT project, session_id FROM events WHERE dedup_key = 'qoder:chatcmpl-fb'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        // The encoded dir is kept verbatim (never decoded); the session falls
        // back to the file stem.
        assert_eq!(project.as_deref(), Some("-Users-dev-projects-gamma"));
        assert_eq!(session.as_deref(), Some("stem-sess"));
    }

    #[test]
    fn cli_rescan_is_idempotent_and_malformed_lines_warn_without_failing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("projects");
        write(
            &root,
            "proj/s.jsonl",
            &format!(
                "{}\nnot-json\n",
                cli_assistant_line("chatcmpl-r1", "s", 100, 0, 10),
            ),
        );
        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        let first = scan_qoder(&mut ledger, &[], std::slice::from_ref(&root));
        assert_eq!(first.events_inserted, 1);
        assert_eq!(first.lines_skipped, 1);
        assert!(first.error.is_none());

        let second = scan_qoder(&mut ledger, &[], &[root]);
        assert_eq!(second.events_inserted, 0, "rescan books nothing new");
        assert_eq!(second.lines_skipped, 0);
    }

    #[test]
    fn missing_cli_roots_are_scanned_quietly() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        let result = scan_qoder(
            &mut ledger,
            &[],
            &[tmp.path().join("does-not-exist")],
        );
        assert_eq!(result.events_inserted, 0);
        assert!(result.error.is_none());
    }

    #[test]
    fn database_editions_and_cli_family_merge_into_one_source_result() {
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
            Some(r#"{"model_key":"qmodel_38max"}"#),
            1_786_112_276_027i64,
        );
        drop(db);
        // The plain-Qoder edition keeps an identically shaped second database.
        let edition_path = tmp.path().join("edition/local.db");
        create_database(&edition_path);
        let edition_db = Connection::open(&edition_path).unwrap();
        insert_message(
            &edition_db,
            "edition-m1",
            Some("s"),
            "assistant",
            Some(
                r#"{"prompt_tokens":80,"completion_tokens":5,"cached_tokens":0,"max_input_tokens":1000000}"#,
            ),
            Some(r#"{"model_key":"qmodel_38max"}"#),
            1_786_112_276_028i64,
        );
        drop(edition_db);
        let root = tmp.path().join("projects");
        write(
            &root,
            "proj/s.jsonl",
            &format!(
                "{}\n",
                cli_assistant_line("chatcmpl-m1", "s", 50, 0, 5),
            ),
        );

        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        let databases = [db_path, edition_path];
        let result = scan_qoder(&mut ledger, &databases, &[root]);
        assert_eq!(result.events_inserted, 3, "one Record per Artifact family");
        assert!(result.error.is_none());
        let by_key: i64 = ledger
            .query_row(
                "SELECT COUNT(DISTINCT dedup_key) FROM events WHERE source = 'qoder'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(by_key, 3);
    }

    #[test]
    fn cli_privacy_markers_never_enter_the_ledger() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("projects");
        write(
            &root,
            "proj/s.jsonl",
            r#"{"type":"assistant","timestamp":"2026-08-07T16:53:21.465Z","message":{"id":"chatcmpl-p1","model":"qmodel_38max","content":[{"type":"text","text":"QODER_CLI_PRIVATE_PROMPT_MARKER"}],"usage":{"input_tokens":10,"output_tokens":2,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
        );
        let mut ledger = crate::db::open_db(&tmp.path().join("ledger.db")).unwrap();
        scan_qoder(&mut ledger, &[], &[root]);

        let durable = fs::read(tmp.path().join("ledger.db")).unwrap();
        assert!(!durable
            .windows("QODER_CLI_PRIVATE_PROMPT_MARKER".len())
            .any(|w| w == b"QODER_CLI_PRIVATE_PROMPT_MARKER"));
    }
}
