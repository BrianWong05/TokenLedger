// TokenLedger — Grok Build adapter.
//
// Grok Build (xAI's CLI) writes JSON-RPC session updates under
// `~/.grok/sessions/<urlencoded-workspace>/<session-id>/updates.jsonl`, with
// sibling `summary.json` (cwd, model).
//
// A Turn ends on a `turn_completed` update, and that line carries the Turn's own
// billed usage rollup in `update.usage`: inputTokens (the whole prompt),
// outputTokens, cachedReadTokens, cacheCreationTokens, reasoningTokens, and
// modelCalls. One such rollup is one Usage Record; ADR-0001's exclusive buckets
// come out of it by subtraction, because cache reads and writes sit inside
// inputTokens.
//
// The sibling `params._meta.totalTokens` counter is deliberately not read: it
// measures how large the context grew, not what was billed to grow it. A Turn
// re-sends its context once per model call, so the counter runs roughly 35x
// below the rollup — and its rewind on compaction, which once needed a
// signals.json reconciliation to bound, stops mattering entirely.
//
// Context attribution: chunk content (message text, thought text, tool-call
// payloads) is sized transiently (est bytes/4, never stored) and each Turn's
// billed input splits across messages + reasoning by those weights — system
// stays NULL (unobservable), matching the pi convention. That is the Context
// category. The bucket breakdown's own System, which `queries` derives from a
// Session's first Cache Write across every Source, is a different surface and
// not this adapter's to fill or withhold.
use std::fs;
use std::path::Path;

use rusqlite::Connection;
use serde_json::Value;

use super::claude_ctx::{content_bytes, est};
use super::{file_state_of, percent_decode, unchanged};
use crate::db::{insert_limit_readings, replace_file_events, set_file_state};
use crate::limits_artifact::grok_credit_window;
use crate::time::iso_to_epoch;
use crate::types::{
    CtxTokens, FileState, LimitReading, ReadingProvenance, SourceScanResult, UsageEvent,
};

// Bump to force a full re-parse of every session when the parser changes (the
// byte-offset slot carries it through `unchanged`).
// 1: attribute each turn's delta across messages/reasoning by chunk weights.
// 2: bill from turn_completed's usage rollup, not the context counter.
const PARSER_VERSION: i64 = 2;

const MALFORMED_ARTIFACT_WARNING: &str = "grok: malformed or unsupported Source Artifact";
// Kinds whose lines are trusted to feed the rollup and chunk weights. A kind
// not listed here skips quietly (lines_skipped) — never a warning, so a Grok
// Build release adding telemetry cannot red-flag the Source.
const SUPPORTED_UPDATE_KINDS: &[&str] = &[
    "agent_message_chunk",
    "agent_thought_chunk",
    "auto_compact_completed",
    "auto_compact_started",
    "compaction_checkpoint",
    "current_mode_update",
    "hook_execution",
    "image_compressed",
    "image_dropped",
    "plan",
    "retry_state",
    "session_recap",
    "subagent_finished",
    "subagent_spawned",
    "task_backgrounded",
    "task_completed",
    "tool_call",
    "tool_call_update",
    "turn_completed",
    "user_message_chunk",
];

fn record_grok_warning(result: &mut SourceScanResult) {
    match &mut result.error {
        Some(existing) if !existing.contains(MALFORMED_ARTIFACT_WARNING) => {
            existing.push_str("; ");
            existing.push_str(MALFORMED_ARTIFACT_WARNING);
        }
        Some(_) => {}
        None => result.error = Some(MALFORMED_ARTIFACT_WARNING.to_string()),
    }
}

pub fn scan_grok(conn: &mut Connection, sessions_root: &Path, unified_log: &Path) -> SourceScanResult {
    let mut result = SourceScanResult::default();
    capture_limits(conn, unified_log, &mut result);
    if sessions_root.is_file() {
        process_session(conn, sessions_root, &mut result);
        return result;
    }
    let workspaces = match fs::read_dir(sessions_root) {
        Ok(rd) => rd,
        Err(_) => return result, // missing dir → zero events, no error
    };
    for ws in workspaces.flatten() {
        let ws_path = ws.path();
        if !ws_path.is_dir() {
            continue; // e.g. session_search.sqlite
        }
        let sessions = match fs::read_dir(&ws_path) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for session in sessions.flatten() {
            let updates = session.path().join("updates.jsonl");
            if updates.is_file() {
                process_session(conn, &updates, &mut result);
            }
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Limit Readings — the weekly credit pool, out of the CLI's own log (#126)
// ---------------------------------------------------------------------------

/// The log line that carries one. Every other message kind in this file is
/// somebody else's telemetry.
const BILLING_MSG: &str = "billing: fetched credits config";

/// Grok Build writes every credits reading it fetches into
/// `~/.grok/logs/unified.jsonl`, so its Limit is passive like Codex's: no
/// credential is read, no request is made, and ADR-0019's Companion machinery is
/// never invoked. That is not only the cheaper path but the safe one — xAI
/// rotates refresh tokens and serialises the rotation under its own lock, so an
/// exchange would corrupt the CLI's own session. Bound 1's hazard, real here.
///
/// A line this cannot read is skipped rather than counted against the artifact,
/// and a log that yields nothing at all leaves the card empty — the ordinary
/// absent-Source path, since there is nothing here to sign into and nothing to
/// explain. Two things make quiet the right answer rather than the lazy one: the
/// log is written through a staging file, so a truncated final line is ordinary;
/// and it is a shared telemetry log, so most of it was never ours to parse.
fn capture_limits(conn: &mut Connection, unified_log: &Path, result: &mut SourceScanResult) {
    let state = file_state_of(unified_log);
    if unchanged(conn, unified_log, &state) {
        return;
    }
    let Ok(content) = fs::read_to_string(unified_log) else {
        return; // a log we cannot read is not a Source in trouble
    };

    // Re-reading the whole log is free: each line carries the stamp it was
    // observed at, so a repeat lands on the Reading already stored, and the file
    // only rewrites on rotation.
    let readings: Vec<LimitReading> = content
        .lines()
        .filter(|line| line.contains(BILLING_MSG))
        .filter_map(billing_reading)
        .collect();

    // A write that fails says so in its own words — reporting it as an
    // unreadable log would blame the file for the database's trouble.
    match insert_limit_readings(conn, &readings) {
        Ok(written) => result.limit_readings += written,
        Err(error) => {
            result.error = Some(format!("grok: could not record Limit Readings: {error}"));
            return;
        }
    }
    let _ = set_file_state(conn, &unified_log.to_string_lossy(), state);
}

/// One logged credits reading → a Limit Reading, or None when it names no window
/// this card can place. The window maths lives in `limits_artifact::grok_credit_window`,
/// shared with the live Companion because both see the identical `config` shape;
/// this path adds only what the log envelope carries — the observation time, the
/// plan, and the `logs` provenance.
fn billing_reading(line: &str) -> Option<LimitReading> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("msg").and_then(Value::as_str) != Some(BILLING_MSG) {
        return None;
    }
    let ctx = v.get("ctx")?;
    let config = ctx.get("config")?;

    // The envelope stamp, never a filename date: this is when the CLI was told.
    let observed_at = v.get("ts").and_then(Value::as_str).and_then(iso_to_epoch)?;
    let window = grok_credit_window(config)?;

    Some(LimitReading {
        source: "grok".to_string(),
        window_key: window.key,
        window_minutes: window.window_minutes,
        used_pct: window.used_pct,
        resets_at: window.resets_at,
        observed_at,
        via: "logs".to_string(),
        // The CLI merges the plan in from its cached settings before logging, so
        // on this path the label costs nothing.
        plan: ctx
            .get("subscriptionTier")
            .and_then(Value::as_str)
            .map(str::to_string),
        provenance: ReadingProvenance::default(),
    })
}

fn process_session(conn: &mut Connection, updates_path: &Path, result: &mut SourceScanResult) {
    let session_dir = match updates_path.parent() {
        Some(d) => d,
        None => return,
    };
    let summary_path = session_dir.join("summary.json");

    // Tokens come from updates.jsonl alone; summary.json carries the Model and
    // the Project, so a change to either is a reason to re-parse.
    let updates_state = FileState { byte_offset: PARSER_VERSION, ..file_state_of(updates_path) };
    let summary_state = file_state_of(&summary_path);
    if unchanged(conn, updates_path, &updates_state)
        && unchanged(conn, &summary_path, &summary_state)
    {
        return;
    }

    let meta = match read_session_meta(session_dir, result) {
        Some(meta) => meta,
        None => return,
    };
    let events = match parse_updates(updates_path, &meta, result) {
        Some(events) => events,
        None => return,
    };
    // Nothing parsed is not the same as nothing consumed: a live session whose
    // Turn has not completed yet, or a log written before Grok Build carried
    // rollups, would have the replace below delete records this parser can no
    // longer re-derive. Leaving the file unstamped re-parses it next Scan.
    if events.is_empty() {
        return;
    }

    let path_str = updates_path.to_string_lossy().to_string();
    let n = events.len() as u64;
    if replace_file_events(conn, &path_str, &events).is_err() {
        result.error = Some(format!("failed to write events for {path_str}"));
        return;
    }
    result.events_inserted += n;
    let _ = set_file_state(conn, &path_str, updates_state);
    if summary_state.size > 0 || summary_state.mtime > 0 {
        let _ = set_file_state(conn, &summary_path.to_string_lossy(), summary_state);
    }
}

struct SessionMeta {
    session_id: String,
    model: Option<String>,
    project: Option<String>,
}

fn read_session_meta(
    session_dir: &Path,
    result: &mut SourceScanResult,
) -> Option<SessionMeta> {
    let session_id = session_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    // Workspace from the percent-encoded parent dir name; summary.json's
    // info.cwd is authoritative when present.
    // ponytail: grok worktree cwds (~/.grok/worktrees/<slug>/<run>) stay
    // verbatim — the parent repo path is not recoverable from the path alone;
    // resolve via summary.json git_remotes if rollup ever matters.
    let mut project = session_dir
        .parent()
        .and_then(|d| d.file_name())
        .and_then(|n| n.to_str())
        .map(percent_decode)
        .filter(|p| p.starts_with('/'));

    // Never a sentinel: usage without a reliable Model stays Unattributed
    // (ADR-0008), so a missing summary or current_model_id means None.
    let mut model: Option<String> = None;
    let summary_path = session_dir.join("summary.json");
    if summary_path.exists() {
        let content = match fs::read_to_string(&summary_path) {
            Ok(content) => content,
            Err(_) => {
                record_grok_warning(result);
                return None;
            }
        };
        let v = match serde_json::from_str::<Value>(&content) {
            Ok(v) if v.is_object() && v.as_object().is_some_and(|object| {
                object.contains_key("info")
                    || object.contains_key("current_model_id")
                    || object.contains_key("updated_at")
                    || object.contains_key("created_at")
            }) => v,
            _ => {
                record_grok_warning(result);
                return None;
            }
        };
        if let Some(m) = v.get("current_model_id").and_then(Value::as_str) {
            if !m.is_empty() {
                model = Some(m.to_string());
            }
        }
        if let Some(cwd) = v.pointer("/info/cwd").and_then(Value::as_str) {
            if !cwd.is_empty() {
                project = Some(cwd.to_string());
            }
        }
    }

    Some(SessionMeta { session_id, model, project })
}

// One Turn's billed usage, in TokenLedger's four exclusive buckets. `billed` is
// the whole prompt (Input + Cache Read + Cache Write) and so is what the Context
// categories must partition.
struct Usage {
    billed: i64,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    // None = the rollup did not say, which is not the same as none generated.
    reasoning: Option<i64>,
    calls: i64,
}

// One `update.usage` object → the buckets. None means a rollup this parser
// cannot trust, which is a malformed Artifact rather than a quiet skip: these
// field names *are* the Usage Record now, so a rename has to be loud — and so
// does a rollup that contradicts itself. Booking zeros is precisely how the
// counter era under-reported this Source by 35x without a single warning, and
// clamping a figure into range is that same silence wearing a plausible number.
fn parse_usage(v: &Value) -> Option<Usage> {
    if !v.is_object() {
        return None;
    }
    // Absent is zero; present-but-not-a-count is a rollup we do not understand.
    let optional = |key: &str| -> Option<i64> {
        match v.get(key) {
            None => Some(0),
            Some(value) => value.as_i64().filter(|&count| count >= 0),
        }
    };
    // Required, because each carries a claim nothing can stand in for: the
    // prompt total is what Context partitions, and modelCalls is what makes this
    // Source's Requests exact rather than a floor (CONTEXT.md, Request).
    let required = |key: &str| -> Option<i64> {
        v.get(key)?.as_i64().filter(|&count| count >= 0)
    };
    let billed = required("inputTokens")?;
    let calls = required("modelCalls")?;
    let output = optional("outputTokens")?;
    let cache_read = optional("cachedReadTokens")?;
    let cache_write = optional("cacheCreationTokens")?;
    let reasoning = match v.get("reasoningTokens") {
        None => None,
        Some(value) => Some(value.as_i64().filter(|&count| count >= 0)?),
    };
    // xAI reports cache inside the prompt total and reasoning inside output, and
    // a rollup exists only because calls were made. One that breaks any of those
    // has had its shape moved under us — it is not one to bend into place.
    if cache_read.saturating_add(cache_write) > billed
        || reasoning.is_some_and(|reasoning| reasoning > output)
        || calls < 1
    {
        return None;
    }
    Some(Usage {
        billed,
        // ADR-0001: Input excludes what the cache served or stored.
        input: billed - cache_read - cache_write,
        output,
        cache_read,
        cache_write,
        reasoning,
        calls,
    })
}

// The (messages, reasoning) est-weight of one update's chunk content.
fn update_weights(u: &Value) -> (i64, i64) {
    let text = u
        .pointer("/content/text")
        .map(|t| est(content_bytes(t)))
        .unwrap_or(0);
    match u.get("sessionUpdate").and_then(Value::as_str) {
        Some("agent_thought_chunk") => (0, text),
        Some("user_message_chunk") | Some("agent_message_chunk") => (text, 0),
        Some("tool_call") | Some("tool_call_update") => {
            let payloads = ["rawInput", "rawOutput", "content"]
                .iter()
                .filter_map(|k| u.get(*k))
                .map(|v| est(content_bytes(v)))
                .sum();
            (payloads, 0)
        }
        _ => (0, 0),
    }
}

// Split a Turn's billed input by the observed chunk weights (pi convention:
// system is unobservable for grok, and a zero reasoning share stays NULL —
// never a fabricated 0). Messages takes the remainder, so the partition over
// the two observable categories is exact.
fn attribute_turn(msg: i64, reas: i64, billed: i64) -> CtxTokens {
    let total = msg + reas;
    if total <= 0 || billed <= 0 {
        return CtxTokens::default();
    }
    let reasoning = billed * reas / total;
    CtxTokens {
        messages: Some(billed - reasoning),
        reasoning: (reasoning > 0).then_some(reasoning),
        ..Default::default()
    }
}

// The update node of a line this parser trusts, or None for one to skip.
fn supported_update(v: &Value) -> Option<&Value> {
    if !matches!(
        v.get("method").and_then(Value::as_str),
        Some("session/update") | Some("_x.ai/session/update")
    ) {
        return None;
    }
    v.pointer("/params/sessionId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())?;
    let update = v.pointer("/params/update")?;
    update
        .get("sessionUpdate")
        .and_then(Value::as_str)
        .filter(|kind| SUPPORTED_UPDATE_KINDS.contains(kind))?;
    Some(update)
}

fn parse_updates(
    updates_path: &Path,
    meta: &SessionMeta,
    result: &mut SourceScanResult,
) -> Option<Vec<UsageEvent>> {
    use std::io::{BufRead, BufReader};

    let file = match fs::File::open(updates_path) {
        Ok(f) => f,
        Err(_) => {
            record_grok_warning(result);
            return None;
        }
    };

    let mut events = Vec::new();
    // Chunk weights for the Turn now running: everything seen since the last
    // turn_completed, so a file that opens mid-Turn still weighs what it shows.
    let (mut msg, mut reas) = (0i64, 0i64);

    for line in BufReader::new(file).lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => {
                record_grok_warning(result);
                return None;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                result.lines_skipped += 1;
                record_grok_warning(result);
                return None;
            }
        };

        // A kind (or method) this parser has never seen is a Grok Build release's
        // new telemetry, not a malformed Artifact — skip the line and count it,
        // which surfaces it as the Overview's informational notice. A renamed
        // `turn_completed` lands here too: counted and visible, rather than a
        // Source that silently stops growing.
        // (task_backgrounded, image_dropped, and auto_compact_* each red-flagged
        // the whole Source under the old reject-unknown policy.)
        let Some(update) = supported_update(&v) else {
            result.lines_skipped += 1;
            continue;
        };

        let ts = match v
            .get("timestamp")
            .and_then(Value::as_i64)
            .filter(|&t| t > 0)
        {
            Some(ts) => ts,
            None => {
                result.lines_skipped += 1;
                record_grok_warning(result);
                return None;
            }
        };

        let (line_msg, line_reas) = update_weights(update);
        msg += line_msg;
        reas += line_reas;

        if update.get("sessionUpdate").and_then(Value::as_str) != Some("turn_completed") {
            continue;
        }

        // The Turn ends here whatever it reported, so the next Turn's weights
        // start clean even when this one booked nothing.
        let (turn_msg, turn_reas) = (msg, reas);
        msg = 0;
        reas = 0;

        // No rollup, so nothing billed that this can see. Usually a Turn the
        // reader cancelled — but the two do not line up as neatly as that: the
        // wild holds a cancelled Turn that did carry a rollup (booked below,
        // like any other) and a completed one that carried none. Either way a
        // zero-token observation is not a Usage Record, and there is nothing
        // here to invent from a Turn the Source itself did not account for.
        let Some(usage) = update.get("usage") else {
            continue;
        };
        let Some(usage) = parse_usage(usage) else {
            result.lines_skipped += 1;
            record_grok_warning(result);
            return None;
        };
        if usage.billed == 0 && usage.output == 0 {
            continue;
        }

        // prompt_id keys the Record to the Turn itself, so a rotated or
        // truncated log cannot shift one Record's identity onto another's. It is
        // part of the shape this parser depends on, so a Turn that bills without
        // one is malformed rather than keyed by its position in the file.
        let Some(key) = update
            .get("prompt_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            result.lines_skipped += 1;
            record_grok_warning(result);
            return None;
        };
        let ctx = attribute_turn(turn_msg, turn_reas, usage.billed);
        events.push(make_event(meta, updates_path, key, ts, usage, ctx));
    }

    Some(events)
}
fn make_event(
    meta: &SessionMeta,
    updates_path: &Path,
    turn_key: &str,
    timestamp: i64,
    usage: Usage,
    ctx: CtxTokens,
) -> UsageEvent {
    UsageEvent {
        dedup_key: format!("grok:{}:{}", meta.session_id, turn_key),
        source: "grok".to_string(),
        timestamp,
        // Still summary.json's current_model_id: the rollup's own modelUsage key
        // names the Turn's real Model (`grok-4.6-build`), which no pricing
        // catalog carries, and an Unpriced Model would trade Cost for accuracy.
        model: meta.model.clone(),
        project: meta.project.clone(),
        api_calls: usage.calls,
        input_tokens: usage.input,
        output_tokens: usage.output,
        cache_read_tokens: usage.cache_read,
        // ponytail: the rollup names no TTL, so the cheaper bucket takes the
        // write; split it when a rollup ever distinguishes the two.
        cache_write_5m_tokens: usage.cache_write,
        cache_write_1h_tokens: 0,
        source_file: updates_path.to_string_lossy().to_string(),
        session_id: Some(meta.session_id.clone()),
        reasoning_tokens: usage.reasoning,
        ctx,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_db;
    use std::path::PathBuf;
    use tempfile::tempdir;

    // Shapes mirror real ~/.grok/sessions data (2026-08): a Turn's billed usage
    // arrives on its own turn_completed line, the chunk lines carry the content
    // the Context split is weighed by, `_meta.totalTokens` is the context
    // counter this parser ignores, and the top-level timestamp is epoch seconds.
    fn update_line(ts: i64, kind: &str) -> String {
        update_line_text(ts, kind, "x", None)
    }

    fn update_line_text(ts: i64, kind: &str, text: &str, total: Option<i64>) -> String {
        let total = total
            .map(|t| format!(r#""totalTokens":{t},"#))
            .unwrap_or_default();
        format!(
            r#"{{"timestamp":{ts},"method":"session/update","params":{{"sessionId":"s","update":{{"sessionUpdate":"{kind}","content":{{"type":"text","text":"{text}"}}}},"_meta":{{{total}"eventId":"e"}}}}}}"#
        )
    }

    // One completed Turn. `input` is the whole prompt: `cache_read` sits inside
    // it, exactly as xAI reports it.
    fn turn_completed(ts: i64, prompt: &str, input: i64, output: i64, cache_read: i64) -> String {
        rollup(
            ts,
            prompt,
            &format!(
                r#""inputTokens":{input},"outputTokens":{output},"totalTokens":{},"cachedReadTokens":{cache_read},"cacheCreationTokens":0,"reasoningTokens":0,"modelCalls":1"#,
                input + output
            ),
        )
    }

    fn rollup(ts: i64, prompt: &str, usage: &str) -> String {
        format!(
            r#"{{"timestamp":{ts},"method":"_x.ai/session/update","params":{{"sessionId":"s","update":{{"sessionUpdate":"turn_completed","prompt_id":"{prompt}","stop_reason":"end_turn","usage":{{{usage}}}}},"_meta":{{"eventId":"e"}}}}}}"#
        )
    }

    // A Turn the reader interrupted: Grok Build writes the line with no usage.
    fn cancelled_turn(ts: i64, prompt: &str) -> String {
        format!(
            r#"{{"timestamp":{ts},"method":"_x.ai/session/update","params":{{"sessionId":"s","update":{{"sessionUpdate":"turn_completed","prompt_id":"{prompt}","stop_reason":"cancelled"}},"_meta":{{"eventId":"e"}}}}}}"#
        )
    }

    fn write_session(
        root: &Path,
        workspace: &str,
        session_id: &str,
        updates: &[String],
        summary: Option<&str>,
    ) -> PathBuf {
        let dir = root.join(workspace).join(session_id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("updates.jsonl"), updates.join("\n") + "\n").unwrap();
        if let Some(s) = summary {
            std::fs::write(dir.join("summary.json"), s).unwrap();
        }
        dir.join("updates.jsonl")
    }

    fn summary(id: &str, cwd: &str, model: &str) -> String {
        format!(
            r#"{{"info":{{"id":"{id}","cwd":"{cwd}"}},"current_model_id":"{model}","updated_at":"2026-08-22T12:00:00Z"}}"#
        )
    }

    // The sessions half on its own: a Source with no unified log beside it.
    fn no_log(root: &Path) -> PathBuf {
        root.join("no-such-log.jsonl")
    }

    fn scan_sessions(conn: &mut Connection, root: &Path) -> SourceScanResult {
        scan_grok(conn, root, &no_log(root))
    }

    fn scan(root: &Path) -> (tempfile::TempDir, rusqlite::Connection, SourceScanResult) {
        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        let res = scan_sessions(&mut conn, root);
        (app, conn, res)
    }

    #[test]
    fn each_completed_turn_books_its_rollup_in_exclusive_buckets() {
        let tmp = tempdir().unwrap();
        write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Falpha",
            "sess-1",
            &[
                update_line(100, "user_message_chunk"),
                update_line(101, "agent_message_chunk"),
                // 4000 prompt, 3000 of it served from cache, 500 generated
                turn_completed(102, "p-1", 4000, 500, 3000),
                update_line(200, "user_message_chunk"),
                rollup(
                    201,
                    "p-2",
                    r#""inputTokens":9000,"outputTokens":700,"totalTokens":9700,"cachedReadTokens":6000,"cacheCreationTokens":1000,"reasoningTokens":250,"modelCalls":22"#,
                ),
            ],
            Some(&summary("sess-1", "/Users/dev/alpha", "grok-4.6")),
        );

        let (_app, conn, res) = scan(tmp.path());
        assert!(res.error.is_none());
        assert_eq!(res.events_inserted, 2);

        type Row = (String, i64, i64, i64, i64, i64, i64, Option<i64>, String, Option<String>);
        let rows: Vec<Row> = conn
            .prepare(
                "SELECT dedup_key, timestamp, input_tokens, output_tokens, cache_read_tokens, \
                 cache_write_5m_tokens, api_calls, reasoning_tokens, model, project \
                 FROM events ORDER BY timestamp",
            )
            .unwrap()
            .query_map([], |r| {
                Ok((
                    r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?,
                    r.get(7)?, r.get(8)?, r.get(9)?,
                ))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();

        // Keyed by the Turn's own prompt_id, stamped when the Turn completed.
        assert_eq!(rows[0].0, "grok:sess-1:p-1");
        assert_eq!(rows[0].1, 102);
        assert_eq!(rows[0].2, 1_000); // 4000 prompt − 3000 cache read (ADR-0001)
        assert_eq!(rows[0].3, 500);
        assert_eq!(rows[0].4, 3_000);
        assert_eq!(rows[0].5, 0);
        assert_eq!(rows[0].6, 1);
        assert_eq!(rows[0].7, Some(0));
        assert_eq!(rows[0].8, "grok-4.6");
        assert_eq!(rows[0].9, Some("/Users/dev/alpha".to_string()));

        assert_eq!(rows[1].0, "grok:sess-1:p-2");
        assert_eq!(rows[1].2, 2_000); // 9000 − 6000 read − 1000 written
        assert_eq!(rows[1].3, 700);
        assert_eq!(rows[1].4, 6_000);
        assert_eq!(rows[1].5, 1_000);
        assert_eq!(rows[1].6, 22); // Requests are modelCalls, not one per Turn
        assert_eq!(rows[1].7, Some(250));

        // The four buckets partition the rollup with no overlap, so the Cache
        // Hit Rate they define is the Source's own.
        let (billed, read): (i64, i64) = conn
            .query_row(
                "SELECT SUM(input_tokens + cache_read_tokens + cache_write_5m_tokens), \
                 SUM(cache_read_tokens) FROM events",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(billed, 13_000); // 4000 + 9000, the two prompts
        assert_eq!(read, 9_000);
    }

    #[test]
    fn the_context_counter_is_never_billed() {
        // The counter measures how big the context grew; the rollup measures
        // what was billed to grow it. Only the rollup is a Usage Record — and a
        // counter that dwarfs it (or rewinds under compaction) changes nothing.
        let tmp = tempdir().unwrap();
        write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Fcounter",
            "sess-counter",
            &[
                update_line(100, "user_message_chunk"),
                update_line_text(101, "agent_message_chunk", "x", Some(400_000)),
                // compaction rewinds the counter mid-Turn
                update_line_text(102, "agent_message_chunk", "x", Some(13_000)),
                turn_completed(103, "p-1", 4_000, 500, 3_000),
            ],
            Some(&summary("sess-counter", "/Users/dev/counter", "grok-4.6")),
        );

        let (_app, conn, res) = scan(tmp.path());
        assert_eq!(res.error, None, "{:?}", res.error);
        assert_eq!(res.events_inserted, 1);
        let (input, read): (i64, i64) = conn
            .query_row(
                "SELECT input_tokens, cache_read_tokens FROM events",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((input, read), (1_000, 3_000));
    }

    #[test]
    fn billed_input_splits_across_messages_and_reasoning_by_chunk_weights() {
        let tmp = tempdir().unwrap();
        write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Falpha",
            "sess-r",
            &[
                // weights: user 8/4=2 msg, thought 32/4=8 reas, agent 24/4=6 msg
                update_line_text(100, "user_message_chunk", "uuuuuuuu", None),
                update_line_text(101, "agent_thought_chunk", &"t".repeat(32), None),
                update_line_text(102, "agent_message_chunk", &"m".repeat(24), None),
                turn_completed(103, "p-1", 4000, 100, 1000),
                // second turn: no thinking observed → reasoning stays NULL
                update_line_text(200, "user_message_chunk", "uuuuuuuu", None),
                update_line_text(201, "agent_message_chunk", &"m".repeat(24), None),
                turn_completed(202, "p-2", 5000, 100, 0),
            ],
            Some(&summary("sess-r", "/Users/dev/alpha", "grok-4.6")),
        );

        let (_app, conn, res) = scan(tmp.path());
        assert!(res.error.is_none());

        let rows: Vec<(Option<i64>, Option<i64>, Option<i64>)> = conn
            .prepare("SELECT ctx_messages, ctx_reasoning, ctx_system FROM events ORDER BY timestamp")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();

        // Turn 1: billed 4000 (cache read included — Context counts the whole
        // prompt), weights msg 8 / reas 8 → reasoning 2000, messages the rest.
        assert_eq!(rows[0], (Some(2_000), Some(2_000), None));
        // Turn 2: billed 5000, no thought chunks → a zero share stays NULL.
        assert_eq!(rows[1], (Some(5_000), None, None));

        // The primary partition is exact against the billed buckets — the
        // cross-Source invariant, asserted here where it is produced.
        crate::invariants::assert_partition_exact(&conn);
    }

    #[test]
    fn a_cancelled_turn_books_nothing_and_keeps_the_next_turn_clean() {
        let tmp = tempdir().unwrap();
        write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Fcancelled",
            "sess-cancelled",
            &[
                update_line_text(100, "user_message_chunk", &"u".repeat(400), None),
                update_line_text(101, "agent_thought_chunk", &"t".repeat(400), None),
                cancelled_turn(102, "p-1"),
                // A fresh Turn: its Context must not inherit the abandoned
                // Turn's weights, which were all over reasoning.
                update_line_text(200, "user_message_chunk", "uuuuuuuu", None),
                turn_completed(201, "p-2", 5000, 100, 0),
            ],
            Some(&summary("sess-cancelled", "/Users/dev/cancelled", "grok-4.6")),
        );

        let (_app, conn, res) = scan(tmp.path());
        assert_eq!(res.error, None, "{:?}", res.error);
        assert_eq!(res.events_inserted, 1);
        let (key, msg, reas): (String, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT dedup_key, ctx_messages, ctx_reasoning FROM events",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(key, "grok:sess-cancelled:p-2");
        assert_eq!((msg, reas), (Some(5_000), None));
    }

    #[test]
    fn a_renamed_usage_field_warns_instead_of_booking_zeros() {
        // The counter era under-reported this Source 35x for weeks because a
        // shape it did not understand read as zero. A rollup whose prompt total
        // this parser cannot find is a malformed Artifact, loudly. Every other
        // field stays valid, so only the rename can be what fires.
        let tmp = tempdir().unwrap();
        write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Frenamed",
            "sess-renamed",
            &[
                update_line(100, "user_message_chunk"),
                rollup(
                    101,
                    "p-1",
                    r#""promptTokens":4000,"outputTokens":500,"cachedReadTokens":3000,"modelCalls":1"#,
                ),
            ],
            Some(&summary("sess-renamed", "/Users/dev/renamed", "grok-4.6")),
        );

        let (_app, conn, res) = scan(tmp.path());
        assert_eq!(res.events_inserted, 0);
        assert!(res
            .error
            .as_deref()
            .is_some_and(|error| error.contains("grok") && error.contains("malformed")));
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    // The rollup fields this parser refuses to guess at or bend into range.
    // Each fixture leaves every other field valid, so only the named defect can
    // be what fires — and each defect must fire, never clamp and carry on.
    #[test]
    fn a_rollup_this_parser_cannot_trust_warns_rather_than_being_repaired() {
        let cases = [
            // Cache cannot exceed the prompt it is said to sit inside. Clamping
            // this would book input=0, cache_read=everything, in silence.
            (
                "cache read outside the prompt total",
                r#""inputTokens":4000,"outputTokens":500,"cachedReadTokens":9000,"modelCalls":1"#,
            ),
            (
                "cache read plus write outside the prompt total",
                r#""inputTokens":4000,"outputTokens":500,"cachedReadTokens":3000,"cacheCreationTokens":2000,"modelCalls":1"#,
            ),
            // Reasoning is generated, so it cannot exceed what was generated.
            (
                "reasoning outside output",
                r#""inputTokens":4000,"outputTokens":500,"reasoningTokens":900,"modelCalls":1"#,
            ),
            // Requests are exact for this Source, so a missing count is not a
            // floor of one — it is a rollup shape this parser does not know.
            (
                "no modelCalls",
                r#""inputTokens":4000,"outputTokens":500,"cachedReadTokens":3000"#,
            ),
            (
                "a rollup claiming no calls made it",
                r#""inputTokens":4000,"outputTokens":500,"modelCalls":0"#,
            ),
        ];

        for (defect, usage) in cases {
            let tmp = tempdir().unwrap();
            write_session(
                tmp.path(),
                "%2FUsers%2Fdev%2Funtrusted",
                "sess-untrusted",
                &[
                    update_line(100, "user_message_chunk"),
                    rollup(101, "p-1", usage),
                ],
                Some(&summary("sess-untrusted", "/Users/dev/untrusted", "grok-4.6")),
            );

            let (_app, conn, res) = scan(tmp.path());
            assert_eq!(res.events_inserted, 0, "{defect}: booked something");
            assert!(
                res.error
                    .as_deref()
                    .is_some_and(|error| error.contains("grok") && error.contains("malformed")),
                "{defect}: no warning, error={:?}",
                res.error
            );
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                0,
                "{defect}: wrote a Record anyway"
            );
        }
    }

    #[test]
    fn a_turn_that_bills_without_a_prompt_id_warns_rather_than_keying_by_position() {
        let tmp = tempdir().unwrap();
        write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Fnoid",
            "sess-noid",
            &[
                update_line(100, "user_message_chunk"),
                r#"{"timestamp":101,"method":"_x.ai/session/update","params":{"sessionId":"s","update":{"sessionUpdate":"turn_completed","stop_reason":"end_turn","usage":{"inputTokens":4000,"outputTokens":500,"modelCalls":1}},"_meta":{"eventId":"e"}}}"#.to_string(),
            ],
            Some(&summary("sess-noid", "/Users/dev/noid", "grok-4.6")),
        );

        let (_app, conn, res) = scan(tmp.path());
        assert_eq!(res.events_inserted, 0);
        assert!(res
            .error
            .as_deref()
            .is_some_and(|error| error.contains("grok") && error.contains("malformed")));
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn an_unreported_reasoning_figure_stays_null_rather_than_zero() {
        // A rollup that omits reasoningTokens did not say none was generated,
        // and the Ledger must not answer a question the Source left open.
        let tmp = tempdir().unwrap();
        write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Fnoreas",
            "sess-noreas",
            &[
                update_line(100, "user_message_chunk"),
                rollup(101, "p-silent", r#""inputTokens":4000,"outputTokens":500,"modelCalls":1"#),
                update_line(200, "user_message_chunk"),
                rollup(201, "p-zero", r#""inputTokens":4000,"outputTokens":500,"reasoningTokens":0,"modelCalls":1"#),
            ],
            Some(&summary("sess-noreas", "/Users/dev/noreas", "grok-4.6")),
        );

        let (_app, conn, res) = scan(tmp.path());
        assert_eq!(res.error, None, "{:?}", res.error);
        let reported: Vec<Option<i64>> = conn
            .prepare("SELECT reasoning_tokens FROM events ORDER BY timestamp")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        // Silent stays NULL; a reported zero is a figure the Source did give.
        assert_eq!(reported, vec![None, Some(0)]);
    }

    #[test]
    fn a_cancelled_turn_that_did_bill_is_booked_like_any_other() {
        // stop_reason and the rollup do not line up: the wild holds a cancelled
        // Turn carrying full usage. What is booked follows the rollup, never the
        // label — the Source charged for that work.
        let tmp = tempdir().unwrap();
        write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Fcancelledbill",
            "sess-cancelled-bill",
            &[
                update_line(100, "user_message_chunk"),
                r#"{"timestamp":101,"method":"_x.ai/session/update","params":{"sessionId":"s","update":{"sessionUpdate":"turn_completed","prompt_id":"p-1","stop_reason":"cancelled","usage":{"inputTokens":4000,"outputTokens":500,"cachedReadTokens":3000,"modelCalls":4}},"_meta":{"eventId":"e"}}}"#.to_string(),
            ],
            Some(&summary("sess-cancelled-bill", "/Users/dev/cancelledbill", "grok-4.6")),
        );

        let (_app, conn, res) = scan(tmp.path());
        assert_eq!(res.error, None, "{:?}", res.error);
        assert_eq!(res.events_inserted, 1);
        let (key, input, read, calls): (String, i64, i64, i64) = conn
            .query_row(
                "SELECT dedup_key, input_tokens, cache_read_tokens, api_calls FROM events",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(key, "grok:sess-cancelled-bill:p-1");
        assert_eq!((input, read, calls), (1_000, 3_000, 4));
    }

    #[test]
    fn a_log_with_no_rollup_keeps_the_records_it_already_has() {
        // The counter-era parser booked Records from `_meta.totalTokens`. This
        // one finds no rollup in that same file — nor in a live session whose
        // Turn has not completed yet. replace_file_events deletes the file's
        // rows before it writes, so a parse that yields nothing must not write.
        let tmp = tempdir().unwrap();
        let updates = write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Fcounteronly",
            "sess-old",
            &[
                update_line(100, "user_message_chunk"),
                update_line_text(101, "agent_message_chunk", "x", Some(4000)),
            ],
            Some(&summary("sess-old", "/Users/dev/counteronly", "grok-4.6")),
        );

        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        // What the previous parser version left in the Ledger for this file.
        conn.execute(
            "INSERT INTO events (dedup_key, source, timestamp, model, api_calls, \
             input_tokens, source_file) \
             VALUES ('grok:sess-old:0', 'grok', 100, 'grok-4.6', 1, 4000, ?1)",
            [updates.to_string_lossy().to_string()],
        )
        .unwrap();

        let res = scan_sessions(&mut conn, tmp.path());
        assert_eq!(res.error, None, "{:?}", res.error);
        assert_eq!(res.events_inserted, 0);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1,
            "a Record this parser cannot re-derive must survive the scan"
        );
    }

    #[test]
    fn workspace_dir_decodes_when_summary_missing() {
        let tmp = tempdir().unwrap();
        write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Fbeta",
            "sess-3",
            &[
                update_line(100, "user_message_chunk"),
                turn_completed(101, "p-1", 500, 50, 0),
            ],
            None,
        );

        let (_app, conn, _res) = scan(tmp.path());
        let (model, project): (Option<String>, Option<String>) = conn
            .query_row("SELECT model, project FROM events", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(model, None); // no summary.json → Unattributed, never "unknown"
        assert_eq!(project, Some("/Users/dev/beta".to_string()));
    }

    #[test]
    fn unchanged_files_are_skipped_and_growth_rescans() {
        let tmp = tempdir().unwrap();
        let updates = write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Falpha",
            "sess-4",
            &[
                update_line(100, "user_message_chunk"),
                turn_completed(101, "p-1", 1000, 100, 0),
            ],
            None,
        );

        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        let first = scan_sessions(&mut conn, tmp.path());
        assert_eq!(first.events_inserted, 1);

        // Same content → skipped entirely.
        let second = scan_sessions(&mut conn, tmp.path());
        assert_eq!(second.events_inserted, 0);

        // Session grows (new turn, distinct mtime via size change) → rescan
        // replaces the file's events, no duplicates.
        let mut content = std::fs::read_to_string(&updates).unwrap();
        content.push_str(&update_line(200, "user_message_chunk"));
        content.push('\n');
        content.push_str(&turn_completed(201, "p-2", 2500, 200, 0));
        content.push('\n');
        std::fs::write(&updates, content).unwrap();

        let third = scan_sessions(&mut conn, tmp.path());
        assert_eq!(third.events_inserted, 2);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn missing_root_is_quiet() {
        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        let res = scan_grok(&mut conn, Path::new("/nonexistent/grok/sessions"), Path::new("/nonexistent/grok/logs/unified.jsonl"));
        assert_eq!(res.events_inserted, 0);
        assert!(res.error.is_none());
    }

    #[test]
    fn malformed_updates_report_a_grok_specific_warning() {
        let tmp = tempdir().unwrap();
        write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Fmalformed",
            "sess-malformed",
            &["not json".to_string()],
            None,
        );

        let (_app, _conn, res) = scan(tmp.path());
        assert_eq!(res.events_inserted, 0);
        assert!(res
            .error
            .as_deref()
            .is_some_and(|error| error.contains("grok") && error.contains("malformed")));
    }

    #[test]
    fn malformed_rescan_warns_without_deleting_existing_history() {
        let tmp = tempdir().unwrap();
        let updates = write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Fhistory",
            "sess-history",
            &[
                update_line(100, "user_message_chunk"),
                turn_completed(101, "p-1", 500, 50, 0),
            ],
            None,
        );

        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        let first = scan_sessions(&mut conn, tmp.path());
        assert_eq!(first.events_inserted, 1);

        std::fs::write(&updates, "not json\n").unwrap();
        let second = scan_sessions(&mut conn, tmp.path());
        assert_eq!(second.events_inserted, 0);
        assert!(second.error.is_some());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn unknown_update_kinds_are_skipped_without_rejecting_the_session() {
        // A new Grok Build release's update kind (auto_compact_* was the third
        // such incident) is skipped and counted — the session's real turns must
        // still book, and no warning fires.
        let tmp = tempdir().unwrap();
        write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Funsupported",
            "sess-unsupported",
            &[
                update_line(100, "user_message_chunk"),
                r#"{"timestamp":101,"method":"_x.ai/session/update","params":{"sessionId":"s","update":{"sessionUpdate":"future_update"}}}"#.to_string(),
                turn_completed(102, "p-1", 4000, 400, 0),
            ],
            None,
        );

        let (_app, conn, res) = scan(tmp.path());
        assert_eq!(res.error, None, "{:?}", res.error);
        assert_eq!(res.events_inserted, 1);
        assert_eq!(res.lines_skipped, 1);
        let tokens: i64 = conn
            .query_row(
                "SELECT input_tokens FROM events WHERE dedup_key = 'grok:sess-unsupported:p-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tokens, 4000);
    }

    #[test]
    fn a_session_of_only_unknown_updates_is_quiet() {
        // Nothing recognisable, but the summary.json shape check still stands
        // guard for a wholesale format change — this alone is not one.
        let tmp = tempdir().unwrap();
        write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Funsupported",
            "sess-only-unknown",
            &[r#"{"timestamp":100,"method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"future_update"}}}"#.to_string()],
            None,
        );

        let (_app, _conn, res) = scan(tmp.path());
        assert_eq!(res.events_inserted, 0);
        assert_eq!(res.error, None);
    }

    fn task_lifecycle_line(ts: i64, kind: &str) -> String {
        format!(
            r#"{{"timestamp":{ts},"method":"_x.ai/session/update","params":{{"sessionId":"s","update":{{"sessionUpdate":"{kind}","task_id":"t1"}},"_meta":{{"eventId":"e"}}}}}}"#
        )
    }

    fn image_dropped_line(ts: i64) -> String {
        format!(
            r#"{{"timestamp":{ts},"method":"_x.ai/session/update","params":{{"sessionId":"s","update":{{"sessionUpdate":"image_dropped","notes":["dropped"]}},"_meta":{{"eventId":"e"}}}}}}"#
        )
    }

    #[test]
    fn task_lifecycle_updates_do_not_reject_the_session() {
        // Background bash / monitor emits task_backgrounded then task_completed
        // on `_x.ai/session/update` with no usage. A session that also has a
        // real turn must still book that turn — not abort as malformed.
        let tmp = tempdir().unwrap();
        write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Ftasks",
            "sess-tasks",
            &[
                update_line(100, "user_message_chunk"),
                task_lifecycle_line(101, "task_backgrounded"),
                task_lifecycle_line(102, "task_completed"),
                turn_completed(103, "p-1", 4000, 400, 0),
            ],
            Some(&summary("sess-tasks", "/Users/dev/tasks", "grok-4.6")),
        );

        let (_app, conn, res) = scan(tmp.path());
        assert_eq!(res.error, None, "{:?}", res.error);
        assert_eq!(res.events_inserted, 1);
        let tokens: i64 = conn
            .query_row(
                "SELECT input_tokens FROM events WHERE dedup_key = 'grok:sess-tasks:p-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tokens, 4000);
    }

    #[test]
    fn image_dropped_updates_do_not_reject_the_session() {
        // Grok Build emits image_dropped on `_x.ai/session/update`, same family
        // as image_compressed. A session that also has a real turn must still
        // book that turn — not abort as malformed.
        let tmp = tempdir().unwrap();
        write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Fimages",
            "sess-images",
            &[
                update_line(100, "user_message_chunk"),
                image_dropped_line(101),
                turn_completed(102, "p-1", 4000, 400, 0),
            ],
            Some(&summary("sess-images", "/Users/dev/images", "grok-4.6")),
        );

        let (_app, conn, res) = scan(tmp.path());
        assert_eq!(res.error, None, "{:?}", res.error);
        assert_eq!(res.events_inserted, 1);
        let tokens: i64 = conn
            .query_row(
                "SELECT input_tokens FROM events WHERE dedup_key = 'grok:sess-images:p-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tokens, 4000);
    }

    #[test]
    fn auto_compaction_telemetry_is_recognised_not_skipped() {
        // Grok Build's 500K-context auto-compaction writes three telemetry
        // lines around the counter rewind. They carry no usage and no chunk
        // content, so they must pass through weightless — and, being known
        // kinds, must not raise the "unrecognized log lines" notice.
        let tmp = tempdir().unwrap();
        write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Fcompact",
            "sess-compact",
            &[
                update_line(100, "user_message_chunk"),
                r#"{"timestamp":102,"method":"_x.ai/session/update","params":{"sessionId":"s","update":{"sessionUpdate":"auto_compact_started","tokens_used":400934,"context_window":500000,"percentage":80,"reason":"Context window 80% full"},"_meta":{"eventId":"e"}}}"#.to_string(),
                r#"{"timestamp":103,"method":"_x.ai/session/update","params":{"sessionId":"s","update":{"sessionUpdate":"compaction_checkpoint","checkpoint_id":"c1","prompt_index_at_compaction":14,"schema_version":1},"_meta":{"eventId":"e"}}}"#.to_string(),
                r#"{"timestamp":104,"method":"_x.ai/session/update","params":{"sessionId":"s","update":{"sessionUpdate":"auto_compact_completed","tokens_before":400934,"tokens_after":13024,"elapsed_ms":119760,"summary_preview":null},"_meta":{"eventId":"e"}}}"#.to_string(),
                turn_completed(105, "p-1", 4000, 400, 0),
            ],
            Some(&summary("sess-compact", "/Users/dev/compact", "grok-4.6")),
        );

        let (_app, conn, res) = scan(tmp.path());
        assert_eq!(res.error, None, "{:?}", res.error);
        assert_eq!(res.lines_skipped, 0);
        assert_eq!(res.events_inserted, 1);
        let tokens: i64 = conn
            .query_row(
                "SELECT input_tokens FROM events WHERE dedup_key = 'grok:sess-compact:p-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tokens, 4000);
    }

    #[test]
    fn missing_update_timestamp_is_not_booked() {
        let tmp = tempdir().unwrap();
        write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Funtimestamped",
            "sess-untimestamped",
            &[
                update_line(0, "user_message_chunk"),
                turn_completed(0, "p-1", 500, 50, 0),
            ],
            None,
        );

        let (_app, conn, res) = scan(tmp.path());
        assert_eq!(res.events_inserted, 0);
        assert!(res.error.is_some());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn malformed_summary_warns_without_deleting_history() {
        let tmp = tempdir().unwrap();
        let updates = write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Fsiblings",
            "sess-siblings",
            &[
                update_line(100, "user_message_chunk"),
                turn_completed(101, "p-1", 500, 50, 0),
            ],
            Some(&summary("sess-siblings", "/Users/dev/siblings", "grok")),
        );

        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        assert_eq!(scan_sessions(&mut conn, tmp.path()).events_inserted, 1);

        std::fs::write(updates.parent().unwrap().join("summary.json"), "not json").unwrap();
        let summary_result = scan_sessions(&mut conn, tmp.path());
        assert!(summary_result.error.is_some());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    // -----------------------------------------------------------------------
    // Limit Readings out of the unified log (#126)
    // -----------------------------------------------------------------------

    // The envelope and payload as they sit in a real ~/.grok/logs/unified.jsonl:
    // period bounds carry microseconds and an explicit offset, the envelope stamp
    // milliseconds, and `subscriptionTier` is merged in by the CLI before logging.
    const WEEK_START: &str = "2026-07-05T00:00:00.000000+00:00";
    const WEEK_END: &str = "2026-07-12T00:00:00.000000+00:00";

    fn billing_line(ts: &str, percent: Option<&str>, period: &str) -> String {
        let percent = percent.map(|p| format!(r#""creditUsagePercent":{p},"#)).unwrap_or_default();
        format!(
            r#"{{"ts":"{ts}","src":"shell","pid":1,"lvl":"info","msg":"billing: fetched credits config",
                "ctx":{{"config":{{{percent}{period}"onDemandCap":{{"val":0}},"isUnifiedBillingUser":true,
                "billingPeriodStart":"{WEEK_START}","billingPeriodEnd":"{WEEK_END}","historyLen":0}},
                "onDemandEnabled":null,"subscriptionTier":"SuperGrok"}}}}"#
        )
        .replace('\n', "")
    }

    fn weekly(ts: &str, percent: Option<&str>) -> String {
        billing_line(
            ts,
            percent,
            &format!(
                r#""currentPeriod":{{"type":"USAGE_PERIOD_TYPE_WEEKLY","start":"{WEEK_START}","end":"{WEEK_END}"}},"#
            ),
        )
    }

    fn scan_log(lines: &[String]) -> (tempfile::TempDir, rusqlite::Connection, SourceScanResult) {
        let tmp = tempdir().unwrap();
        let log = tmp.path().join("unified.jsonl");
        std::fs::write(&log, lines.join("\n") + "\n").unwrap();
        let mut conn = open_db(&tmp.path().join("ledger.db")).unwrap();
        let res = scan_grok(&mut conn, &tmp.path().join("sessions"), &log);
        (tmp, conn, res)
    }

    type Row = (String, Option<i64>, f64, i64, i64, String, Option<String>);

    fn readings(conn: &Connection) -> Vec<Row> {
        conn.prepare(
            "SELECT window_key, window_minutes, used_pct, resets_at, observed_at, via, plan \
             FROM limit_readings WHERE source = 'grok' ORDER BY observed_at",
        )
        .unwrap()
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
    }

    #[test]
    fn logged_credits_become_weekly_readings_and_an_absent_percent_is_zero() {
        // proto3 omits zero-valued scalars, so the 0% row arrives with no
        // `creditUsagePercent` at all — the start of every window looks like this.
        let (_tmp, conn, res) = scan_log(&[
            r#"{"ts":"2026-07-10T20:00:00.000Z","src":"shell","lvl":"info","msg":"something else"}"#.to_string(),
            weekly("2026-07-10T20:49:57.123Z", None),
            weekly("2026-07-10T21:49:57.123Z", Some("14")),
        ]);
        assert!(res.error.is_none());

        let rows = readings(&conn);
        assert_eq!(rows.len(), 2, "one row per distinct figure; the other line is not one");
        assert_eq!(rows[0].0, "w10080");
        assert_eq!(rows[0].1, Some(10_080), "the axis is measured from the period's own bounds");
        assert_eq!(rows[0].2, 0.0, "an absent percentage is 0% used, never malformed");
        assert_eq!(rows[0].3, iso_to_epoch(WEEK_END).unwrap());
        assert_eq!(
            rows[0].4,
            iso_to_epoch("2026-07-10T20:49:57.123Z").unwrap(),
            "observed_at is the envelope stamp, never a filename date",
        );
        assert_eq!(rows[0].5, "logs", "no credential is read and no request is made");
        assert_eq!(rows[0].6.as_deref(), Some("SuperGrok"));
        assert_eq!(rows[1].2, 14.0);
    }

    #[test]
    fn the_window_is_keyed_off_the_period_type_not_the_measured_duration() {
        // A 28-day February measures 40,320 minutes, which falls OUTSIDE the
        // canonical 43200 ±5% band — classifying by duration would split one
        // card's history into two keys once a year.
        let february = billing_line(
            "2027-03-01T00:00:00.000Z",
            Some("31"),
            r#""currentPeriod":{"type":"USAGE_PERIOD_TYPE_MONTHLY",
               "start":"2027-02-01T00:00:00.000000+00:00","end":"2027-03-01T00:00:00.000000+00:00"},"#,
        )
        .replace('\n', "");
        let (_tmp, conn, res) = scan_log(&[february]);
        assert!(res.error.is_none());

        let rows = readings(&conn);
        assert_eq!(rows[0].0, "w43200", "the vendor names the period; we do not infer it");
        assert_eq!(rows[0].1, Some(40_320), "but the tick's axis is the real month");
    }

    #[test]
    fn a_period_this_card_cannot_place_yields_no_reading_and_no_trouble() {
        // No reset instant, and a period type nobody has seen: neither can be put
        // on a bar. An honest blank beats a mislabelled bar, and there is nothing
        // here for a person to fix, so the card stays quiet rather than alarmed.
        for payload in [
            // Neither the current period nor its deprecated mirror names an end.
            r#"{"ts":"2026-07-10T20:49:57.123Z","lvl":"info","msg":"billing: fetched credits config",
               "ctx":{"config":{"creditUsagePercent":14,"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY"}},
               "subscriptionTier":"SuperGrok"}}"#
                .replace('\n', ""),
            billing_line("2026-07-10T20:49:57.123Z", Some("14"), r#""currentPeriod":{"type":"USAGE_PERIOD_TYPE_FORTNIGHTLY","start":"2026-07-05T00:00:00.000000+00:00","end":"2026-07-19T00:00:00.000000+00:00"},"#),
        ] {
            let (_tmp, conn, res) = scan_log(&[payload]);
            assert_eq!(res.error, None);
            assert!(readings(&conn).is_empty());
        }
    }

    #[test]
    fn a_reading_falls_back_to_the_deprecated_reset_mirror() {
        let (_tmp, conn, _res) = scan_log(&[billing_line(
            "2026-07-10T20:49:57.123Z",
            Some("14"),
            r#""currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","start":"2026-07-05T00:00:00.000000+00:00"},"#,
        )]);
        assert_eq!(readings(&conn)[0].3, iso_to_epoch(WEEK_END).unwrap());
    }

    #[test]
    fn rescanning_the_log_changes_nothing() {
        let tmp = tempdir().unwrap();
        let log = tmp.path().join("unified.jsonl");
        let sessions = tmp.path().join("sessions");
        // An hour apart at the same percentage is two Readings: each is its own
        // observation, and after a gap the later one is the only anchor evidence
        // can start from. Re-reading either of them is not a third.
        std::fs::write(
            &log,
            weekly("2026-07-10T20:49:57.123Z", Some("14")) + "\n"
                + &weekly("2026-07-10T21:49:57.123Z", Some("14")) + "\n",
        )
        .unwrap();
        let mut conn = open_db(&tmp.path().join("ledger.db")).unwrap();

        scan_grok(&mut conn, &sessions, &log);
        assert_eq!(readings(&conn).len(), 2);
        scan_grok(&mut conn, &sessions, &log);
        assert_eq!(readings(&conn).len(), 2, "a re-scan re-reads nothing and inserts nothing");
    }

    #[test]
    fn the_log_and_the_sessions_are_read_independently() {
        // ADR-0015: a log this cannot read must not stop the sessions beside it
        // being counted, and neither one's absence is a Source in trouble.
        let tmp = tempdir().unwrap();
        write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Falpha",
            "sess-log",
            &[
                update_line(100, "user_message_chunk"),
                turn_completed(101, "p-1", 4000, 400, 0),
            ],
            None,
        );
        let log = tmp.path().join("unified.jsonl");
        // A truncated final line is ordinary here — the log is written through a
        // staging file — so it must cost the sessions nothing.
        std::fs::write(&log, weekly("2026-07-10T20:49:57.123Z", Some("14")) + "\n{\"ts\":\"2026").unwrap();

        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        let res = scan_grok(&mut conn, tmp.path(), &log);
        assert_eq!(res.error, None);
        assert_eq!(res.events_inserted, 1, "the sessions still counted");
        assert_eq!(readings(&conn).len(), 1, "and the whole line before it still read");
    }
}


