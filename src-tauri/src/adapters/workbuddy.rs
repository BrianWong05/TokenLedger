//! WorkBuddy / CodeBuddy shared transcript parser (ADR-0016).
//!
//! Both Sources write the same Claude-Code-like JSONL transcript shape to
//! different roots (`~/.workbuddy/projects/**/*.jsonl` and
//! `~/.codebuddy/projects/**/*.jsonl`). One parser serves both, keyed by the
//! Source identity, in the same way Oh My Pi shares pi's parser.
//!
//! A Usage Record is derived from a line that carries non-zero usage and whose
//! `type` is `function_call`, `message`, or `summary`. `reasoning`,
//! `function_call_result`, `file-history-snapshot`, and `ai-title` lines never
//! become Records; a zero-token `summary` is not a Record either. Each
//! usage-bearing line is one Request (`usage.requests`, 1 in the observed
//! Artifacts) and deduplicates on
//! the line `id`. Subagent transcripts (`<session>/subagents/agent-*.jsonl`)
//! are scanned as additional Usage Records in the same Session as their parent.
//!
//! Cache semantics: the normalized `inputTokens` **includes** cache reads, so
//! Input Tokens = `inputTokens − cacheRead`. The cache-read figure prefers the
//! OpenAI-style `prompt_cache_hit_tokens`, falling back to the Anthropic-style
//! `cache_read_input_tokens` (nested `message.usage` or `rawUsage`). Cache
//! Write books from `cache_creation_input_tokens` / `prompt_cache_write_tokens`
//! into the single 5-minute bucket (the Artifact cannot prove a TTL split).
//!
//! The logged billed `credit` is ignored: Cost resolves from the catalog by the
//! raw Model name elsewhere (ADR-0002).

use serde_json::Value;
use std::path::Path;

use super::{absolute_project, file_state_of, find_jsonl, normalize_epoch, unchanged};
use crate::db::{insert_events_keep_max_output, set_file_state};
use crate::types::{CtxTokens, FileState, SourceScanResult, UsageEvent};
use rusqlite::Connection;

// Bump to force a full re-parse of every WorkBuddy/CodeBuddy Session when the
// parser changes (the byte-offset slot carries it through `unchanged`).
const TRANSCRIPT_PARSER_VERSION: i64 = 1;

pub fn scan_workbuddy(conn: &mut Connection, projects_root: &Path) -> SourceScanResult {
    scan_transcript(conn, projects_root, "workbuddy")
}

/// Shared scan entry for both Sources. CodeBuddy reuses this with its own key
/// and root (Ticket 02); the Source identity flavours the dedup key only.
pub(crate) fn scan_transcript(
    conn: &mut Connection,
    projects_root: &Path,
    source: &str,
) -> SourceScanResult {
    let mut result = SourceScanResult::default();
    let mut files = Vec::new();
    find_jsonl(projects_root, &mut files);
    files.sort();
    for path in files {
        match scan_file(conn, &path, source) {
            Ok((inserted, skipped)) => {
                result.events_inserted += inserted;
                result.lines_skipped += skipped;
            }
            Err(error) => match result.error.as_mut() {
                Some(previous) => {
                    previous.push_str("; ");
                    previous.push_str(&error);
                }
                None => result.error = Some(error),
            },
        }
    }
    result
}

fn scan_file(
    conn: &mut Connection,
    path: &Path,
    source: &str,
) -> Result<(u64, u64), String> {
    let state = FileState {
        byte_offset: TRANSCRIPT_PARSER_VERSION,
        ..file_state_of(path)
    };
    if unchanged(conn, path, &state) {
        return Ok((0, 0));
    }

    let source_file = path.to_string_lossy().to_string();
    let content = std::fs::read_to_string(path)
        .map_err(|error| format!("{source}: read {}: {error}", path.display()))?;
    let parsed = parse_file(&content, path, source, &source_file);
    // A changed file is reparsed from the top; keep_max_output re-inserts the
    // same dedup keys without double-booking and raises output on a conflict.
    let inserted = insert_events_keep_max_output(conn, &parsed.events)
        .map_err(|error| format!("{source}: insert {}: {error}", path.display()))?;
    set_file_state(conn, &source_file, state)
        .map_err(|error| format!("{source}: metadata {}: {error}", path.display()))?;
    Ok((inserted, parsed.lines_skipped))
}

struct ParsedTranscript {
    events: Vec<UsageEvent>,
    lines_skipped: u64,
}

fn parse_file(
    content: &str,
    path: &Path,
    source: &str,
    source_file: &str,
) -> ParsedTranscript {
    // Some(parent-session) marks a subagent transcript: its Records join the
    // parent Session per ADR-0016. None means a main transcript.
    let parent_session = parent_session_from_path(path);

    let mut events = Vec::new();
    let mut lines_skipped: u64 = 0;

    for line in content.lines() {
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
        if let Some(ev) = parse_line_event(&v, source, source_file, parent_session.as_deref()) {
            events.push(ev);
        }
    }

    ParsedTranscript {
        events,
        lines_skipped,
    }
}

/// The parent Session id for a subagent transcript: the directory component
/// that contains the `subagents/` dir (e.g. `<root>/<project>/<session>/subagents/…`).
/// None for a main transcript. Per ADR-0016 subagent Records join the parent
/// Session, never a Session of their own derived from a line-level field.
fn parent_session_from_path(path: &Path) -> Option<String> {
    let segs = normalize_segments(path);
    for (i, s) in segs.iter().enumerate() {
        if s == "subagents" {
            return i.checked_sub(1).map(|p| segs[p].clone());
        }
    }
    None
}

/// The path's directory segments with `\` normalized to `/` and the file name
/// kept last, so component logic is separator-agnostic.
fn normalize_segments(path: &Path) -> Vec<String> {
    path.to_string_lossy()
        .replace('\\', "/")
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

fn parse_line_event(
    v: &Value,
    source: &str,
    source_file: &str,
    parent_session: Option<&str>,
) -> Option<UsageEvent> {
    let ty = v.get("type").and_then(|t| t.as_str())?;
    if !matches!(ty, "function_call" | "message" | "summary") {
        return None;
    }

    let usage = extract_usage(v)?; // None when the line carries no non-zero usage

    let id = v.get("id").and_then(|i| i.as_str())?;
    if id.is_empty() {
        return None;
    }

    let timestamp = v
        .get("timestamp")
        .and_then(|t| t.as_i64())
        .map(normalize_epoch)
        .filter(|ts| *ts > 0)?;

    let pd = v.get("providerData").and_then(|p| p.as_object());
    let model = pd
        .and_then(|p| p.get("model"))
        .and_then(|m| m.as_str())
        .filter(|m| !m.is_empty())
        .or_else(|| {
            pd.and_then(|p| p.get("requestModelId"))
                .and_then(|m| m.as_str())
                .filter(|m| !m.is_empty())
        })
        .map(str::to_owned);

    let project = absolute_project(v.get("cwd").and_then(|c| c.as_str()));

    // Subagent Records join the parent Session (ADR-0016); a main transcript
    // uses its line-level sessionId and never invents one from the file stem.
    let session_id = match parent_session {
        Some(parent) => Some(parent.to_string()),
        None => v
            .get("sessionId")
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
    };

    Some(UsageEvent {
        dedup_key: format!("{source}:{id}"),
        source: source.to_string(),
        timestamp,
        model,
        project,
        api_calls: usage.requests,
        input_tokens: usage.input,
        output_tokens: usage.output,
        cache_read_tokens: usage.cache_read,
        // The Artifact cannot prove a cache-write TTL split; the whole write
        // books into the 5-minute bucket (ADR-0016).
        cache_write_5m_tokens: usage.cache_write,
        cache_write_1h_tokens: 0,
        source_file: source_file.to_string(),
        session_id,
        reasoning_tokens: usage.reasoning,
        // No Context attribution: the transcripts expose no trustworthy Context
        // tiers; the catalog reports `context: false`.
        ctx: CtxTokens::default(),
    })
}

struct ExtractedUsage {
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    requests: i64,
    reasoning: Option<i64>,
}

/// Resolve the usage on a line across its three representations:
/// `providerData.usage` (normalized), nested `message.usage` (Anthropic-style),
/// and `providerData.rawUsage` (OpenAI-style). Returns None when the line
/// carries no non-zero usage at all — a zero-token observation is not a Usage
/// Record. `inputTokens`/`prompt_tokens` include cache reads, so Input is
/// derived by subtracting the resolved cache-read figure (ADR-0001, ADR-0016).
fn extract_usage(v: &Value) -> Option<ExtractedUsage> {
    let pd = v.get("providerData").and_then(|p| p.as_object());
    let normalized = pd
        .and_then(|p| p.get("usage"))
        .and_then(|u| u.as_object());
    let anthropic = v
        .get("message")
        .and_then(|m| m.as_object())
        .and_then(|m| m.get("usage"))
        .and_then(|u| u.as_object());
    let raw = pd
        .and_then(|p| p.get("rawUsage"))
        .and_then(|u| u.as_object());

    let num = |obj: Option<&serde_json::Map<String, Value>>, key: &str| -> Option<i64> {
        obj.and_then(|o| o.get(key)).and_then(|x| x.as_i64())
    };

    // Input total, preferred in order: normalized → Anthropic → raw.
    let input_total = num(normalized, "inputTokens")
        .or_else(|| num(anthropic, "input_tokens"))
        .or_else(|| num(raw, "prompt_tokens"))
        .unwrap_or(0);
    let output = num(normalized, "outputTokens")
        .or_else(|| num(anthropic, "output_tokens"))
        .or_else(|| num(raw, "completion_tokens"))
        .unwrap_or(0);

    // Cache read: OpenAI-style `prompt_cache_hit_tokens` primary, Anthropic
    // `cache_read_input_tokens` (nested message.usage, then rawUsage) fallback.
    // The writer populates exactly one convention per line type and leaves the
    // other at a placeholder zero (verified across real Artifacts), so a
    // present-but-zero figure means "not this convention", never "no cache":
    // the first non-zero candidate in priority order wins (ADR-0016).
    let cache_read = [
        num(raw, "prompt_cache_hit_tokens"),
        num(anthropic, "cache_read_input_tokens"),
        num(raw, "cache_read_input_tokens"),
    ]
    .into_iter()
    .flatten()
    .find(|n| *n > 0)
    .unwrap_or(0);

    // Cache write: both write fields are alternative representations of the
    // same write; book whichever is non-zero (never sum both).
    let cw_creation = num(raw, "cache_creation_input_tokens").unwrap_or(0);
    let cw_openai = num(raw, "prompt_cache_write_tokens").unwrap_or(0);
    let cache_write = cw_creation.max(cw_openai);

    // A zero-token observation is not a Usage Record (glossary).
    if input_total == 0 && output == 0 && cache_read == 0 && cache_write == 0 {
        return None;
    }

    let requests = num(normalized, "requests").unwrap_or(1).max(1);
    let reasoning = num(raw, "completion_thinking_tokens").filter(|n| *n > 0);

    Some(ExtractedUsage {
        input: (input_total - cache_read).max(0),
        output,
        cache_read,
        cache_write,
        requests,
        reasoning,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_db;
    use rusqlite::Connection;
    use std::fs;

    const PRIVATE_PROMPT: &str = "WORKBUDDY_PRIVATE_PROMPT_MARKER";
    const PRIVATE_RESPONSE: &str = "WORKBUDDY_PRIVATE_RESPONSE_MARKER";

    /// Read every WorkBuddy event back as a UsageEvent (test-only reshape).
    fn all_events(conn: &Connection) -> rusqlite::Result<Vec<UsageEvent>> {
        let mut stmt = conn.prepare(
            "SELECT dedup_key, source, timestamp, model, project, api_calls, \
                    input_tokens, output_tokens, cache_read_tokens, \
                    cache_write_5m_tokens, cache_write_1h_tokens, \
                    source_file, session_id, reasoning_tokens \
             FROM events WHERE source = 'workbuddy' ORDER BY dedup_key",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(UsageEvent {
                dedup_key: row.get(0)?,
                source: row.get(1)?,
                timestamp: row.get(2)?,
                model: row.get(3)?,
                project: row.get(4)?,
                api_calls: row.get(5)?,
                input_tokens: row.get(6)?,
                output_tokens: row.get(7)?,
                cache_read_tokens: row.get(8)?,
                cache_write_5m_tokens: row.get(9)?,
                cache_write_1h_tokens: row.get(10)?,
                source_file: row.get(11)?,
                session_id: row.get(12)?,
                reasoning_tokens: row.get(13)?,
                ctx: CtxTokens::default(),
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// OpenAI-style usage on a function_call line: `inputTokens` includes the
    /// cache read, reported in rawUsage as `prompt_cache_hit_tokens`.
    fn fc_line(id: &str, session: &str, input_total: i64, cache_hit: i64, output: i64) -> String {
        format!(
            r#"{{"type":"function_call","id":"{id}","sessionId":"{session}","timestamp":1786091399000,"cwd":"/Users/dev/projects/alpha","providerData":{{"model":"deepseek-v4-flash","requestModelId":"deepseek-v4-flash","usage":{{"requests":1,"inputTokens":{input_total},"outputTokens":{output},"totalTokens":{total}}},"rawUsage":{{"prompt_tokens":{input_total},"completion_tokens":{output},"total_tokens":{total},"prompt_cache_hit_tokens":{cache_hit},"prompt_cache_miss_tokens":{miss},"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"prompt_cache_write_tokens":0,"completion_thinking_tokens":0,"credit":0.93,"cached_tokens":0}}}}}}"#,
            id = id,
            session = session,
            input_total = input_total,
            output = output,
            total = input_total + output,
            cache_hit = cache_hit,
            miss = input_total - cache_hit,
        )
    }

    /// Anthropic-style usage on a message line: nested `message.usage` with
    /// `input_tokens` including `cache_read_input_tokens`.
    fn msg_line(id: &str, session: &str, input_total: i64, cache_read: i64, output: i64) -> String {
        format!(
            r#"{{"type":"message","id":"{id}","sessionId":"{session}","timestamp":1786091399000,"cwd":"/Users/dev/projects/alpha","message":{{"usage":{{"input_tokens":{input_total},"output_tokens":{output},"total_tokens":{total},"cache_read_input_tokens":{cache_read}}}}}}}"#,
            id = id,
            session = session,
            input_total = input_total,
            output = output,
            total = input_total + output,
            cache_read = cache_read,
        )
    }

    fn parse(content: &str, path: &Path) -> Vec<UsageEvent> {
        parse_file(content, path, "workbuddy", "/fixtures/main.jsonl").events
    }

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn scan_root(conn: &mut Connection, root: &Path) -> SourceScanResult {
        scan_transcript(conn, root, "workbuddy")
    }

    #[test]
    fn one_record_per_usage_bearing_line_with_model_project_session() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            root,
            "Users-dev-projects-alpha/sess-a.jsonl",
            &format!(
                "{}\n{}\n",
                fc_line("c1", "sess-a", 1000, 300, 50),
                msg_line("m1", "sess-a", 500, 0, 20),
            ),
        );
        let mut conn = open_db(&root.join("ledger.db")).unwrap();
        let result = scan_root(&mut conn, &root.join("Users-dev-projects-alpha"));
        assert_eq!(result.events_inserted, 2);
        assert!(result.error.is_none());

        let events: Vec<UsageEvent> = all_events(&conn).unwrap();
        assert_eq!(events.len(), 2);
        let fc = events.iter().find(|e| e.dedup_key == "workbuddy:c1").unwrap();
        assert_eq!(fc.source, "workbuddy");
        assert_eq!(fc.model.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(fc.project.as_deref(), Some("/Users/dev/projects/alpha"));
        assert_eq!(fc.session_id.as_deref(), Some("sess-a"));
        assert_eq!(fc.api_calls, 1);
        assert_eq!(fc.input_tokens, 700); // 1000 − 300 cache read
        assert_eq!(fc.cache_read_tokens, 300);
        assert_eq!(fc.output_tokens, 50);
        let m = events.iter().find(|e| e.dedup_key == "workbuddy:m1").unwrap();
        assert_eq!(m.input_tokens, 500);
        assert_eq!(m.cache_read_tokens, 0);
    }

    #[test]
    fn cache_splits_both_conventions() {
        // OpenAI-style primary on function_call.
        let events = parse(
            &fc_line("a1", "sess-a", 32221, 32000, 198),
            Path::new("/f.jsonl"),
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].input_tokens, 221);
        assert_eq!(events[0].cache_read_tokens, 32000);
        assert_eq!(events[0].output_tokens, 198);

        // Anthropic-style fallback on message lines.
        let events = parse(
            &msg_line("b1", "sess-a", 37247, 36224, 454),
            Path::new("/f.jsonl"),
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].input_tokens, 1023);
        assert_eq!(events[0].cache_read_tokens, 36224);
    }

    #[test]
    fn present_but_zero_primary_does_not_defeat_the_fallback() {
        // A line carrying OpenAI-style `prompt_cache_hit_tokens: 0` alongside a
        // populated Anthropic-style `cache_read_input_tokens` must still book
        // the cache read (ADR-0016): the first non-zero candidate in priority
        // order wins rather than a present-but-zero primary.
        let line = r#"{"type":"function_call","id":"z1","sessionId":"sess-a","timestamp":1786091399000,"cwd":"/Users/dev/projects/alpha","message":{"usage":{"input_tokens":37247,"output_tokens":454,"total_tokens":37701,"cache_read_input_tokens":36224}},"providerData":{"model":"deepseek-v4-flash","usage":{"requests":1,"inputTokens":37247,"outputTokens":454,"totalTokens":37701},"rawUsage":{"prompt_tokens":37247,"completion_tokens":454,"total_tokens":37701,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":37247,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"prompt_cache_write_tokens":0}}}"#;
        let events = parse(line, Path::new("/f.jsonl"));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].cache_read_tokens, 36224);
        assert_eq!(events[0].input_tokens, 1023);
    }

    #[test]
    fn parent_and_subagent_are_additive_without_double_count() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Parent session: one usage-bearing call.
        write(
            root,
            "proj/parent-sess.jsonl",
            &fc_line("p1", "parent-sess", 66104, 0, 2629),
        );
        // Subagent transcript under <session>/subagents/: its own usage lines,
        // joined to the parent Session per ADR-0016.
        write(
            root,
            "proj/parent-sess/subagents/agent-1.jsonl",
            &format!(
                "{}\n{}\n",
                fc_line("s1", "sub-sess", 1_000_000, 0, 5000),
                msg_line("s2", "sub-sess", 2_000_000, 0, 8000),
            ),
        );
        let mut conn = open_db(&root.join("ledger.db")).unwrap();
        let result = scan_root(&mut conn, &root.join("proj"));
        assert_eq!(result.events_inserted, 3, "parent + 2 subagent Records");
        assert!(result.error.is_none());

        let events: Vec<UsageEvent> = all_events(&conn).unwrap();
        assert_eq!(events.len(), 3);
        let parent = events.iter().find(|e| e.dedup_key == "workbuddy:p1").unwrap();
        assert_eq!(parent.session_id.as_deref(), Some("parent-sess"));
        for sub in events.iter().filter(|e| e.dedup_key != "workbuddy:p1") {
            assert_eq!(
                sub.session_id.as_deref(),
                Some("parent-sess"),
                "subagent Records join the parent Session"
            );
        }
        // No double count: exactly 3 distinct line ids.
        let keys: std::collections::HashSet<_> =
            events.iter().map(|e| e.dedup_key.clone()).collect();
        assert_eq!(keys.len(), 3);
    }

    #[test]
    fn non_usage_line_types_never_become_records_and_zero_summary_is_not_a_record() {
        let lines = [
            r#"{"type":"reasoning","id":"r1","timestamp":1786091399000,"providerData":{"model":"deepseek-v4-flash"}}"#,
            r#"{"type":"function_call_result","id":"fr1","timestamp":1786091399000,"output":{"type":"text","text":"result"}}"#,
            r#"{"type":"file-history-snapshot","id":"fh1","timestamp":1786091399000,"snapshot":{}}"#,
            r#"{"type":"ai-title","id":"at1","timestamp":1786091399000}"#,
            // Zero-token summary: not a Record.
            r#"{"type":"summary","id":"sm0","timestamp":1786091399000,"providerData":{"usage":{"requests":1,"inputTokens":0,"outputTokens":0,"totalTokens":0}}}"#,
        ];
        let events = parse(&lines.join("\n"), Path::new("/f.jsonl"));
        assert!(events.is_empty(), "expected no Records from non-usage lines");

        // Non-zero summary: a Record.
        let events = parse(
            r#"{"type":"summary","id":"sm1","timestamp":1786091399000,"cwd":"/x","providerData":{"model":"hy3","usage":{"requests":1,"inputTokens":100,"outputTokens":5,"totalTokens":105}}}"#,
            Path::new("/f.jsonl"),
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].input_tokens, 100);
    }

    #[test]
    fn credit_is_ignored_and_cost_resolves_from_catalog() {
        let events = parse(
            &fc_line("c1", "sess-a", 1000, 300, 50),
            Path::new("/f.jsonl"),
        );
        assert_eq!(events.len(), 1);
        // No credit field exists on a Usage Event: Cost resolves downstream.
        assert_eq!(events[0].model.as_deref(), Some("deepseek-v4-flash"));
    }

    #[test]
    fn rescan_books_nothing_and_malformed_lines_warn_without_failing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            root,
            "proj/sess.jsonl",
            &format!("{}\nnot-json\n{}\n", fc_line("c1", "sess-a", 100, 0, 10), msg_line("m1", "sess-a", 50, 0, 5)),
        );
        let mut conn = open_db(&root.join("ledger.db")).unwrap();
        let first = scan_root(&mut conn, &root.join("proj"));
        assert_eq!(first.events_inserted, 2);
        assert_eq!(first.lines_skipped, 1, "malformed line counted, scan still green");
        assert!(first.error.is_none());

        let second = scan_root(&mut conn, &root.join("proj"));
        assert_eq!(second.events_inserted, 0, "rescan books nothing new");
        assert_eq!(second.lines_skipped, 0);
        let totals: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(totals, 2);
    }

    #[test]
    fn privacy_markers_never_enter_the_ledger() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Content fields carry the markers; only numeric usage may be read.
        let line = format!(
            r#"{{"type":"function_call","id":"c1","sessionId":"s","timestamp":1786091399000,"cwd":"/Users/dev/projects/alpha","content":[{{"type":"text","text":"{PRIVATE_PROMPT}"}}],"output":{{"type":"text","text":"{PRIVATE_RESPONSE}"}},"providerData":{{"model":"m","usage":{{"requests":1,"inputTokens":100,"outputTokens":10}}}}}}"#,
            PRIVATE_PROMPT = PRIVATE_PROMPT,
            PRIVATE_RESPONSE = PRIVATE_RESPONSE,
        );
        write(root, "proj/sess.jsonl", &line);
        let mut conn = open_db(&root.join("ledger.db")).unwrap();
        scan_root(&mut conn, &root.join("proj"));

        let db_content: String = conn
            .query_row(
                "SELECT group_concat(input_tokens || ':' || output_tokens || ':' || COALESCE(model,'')) FROM events",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!db_content.contains(PRIVATE_PROMPT));
        assert!(!db_content.contains(PRIVATE_RESPONSE));
        assert!(db_content.contains("100:10:m"));
    }

    #[test]
    fn missing_root_is_scanned_quietly() {
        let tmp = tempfile::tempdir().unwrap();
        let mut conn = open_db(&tmp.path().join("ledger.db")).unwrap();
        let result = scan_root(&mut conn, &tmp.path().join("does-not-exist"));
        assert_eq!(result.events_inserted, 0);
        assert!(result.error.is_none());
    }

    #[test]
    fn subagent_path_detection_is_platform_agnostic() {
        assert_eq!(
            parent_session_from_path(Path::new(
                "/root/proj/parent-sess/subagents/agent-1.jsonl"
            )),
            Some("parent-sess".to_string())
        );
        assert_eq!(
            parent_session_from_path(Path::new(
                r"C:\root\proj\parent-sess\subagents\agent-1.jsonl"
            )),
            Some("parent-sess".to_string())
        );
        assert_eq!(
            parent_session_from_path(Path::new("/root/proj/parent-sess.jsonl")),
            None
        );
    }
}
