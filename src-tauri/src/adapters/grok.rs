// TokenLedger — Grok Build adapter.
//
// Grok Build (xAI's CLI) writes JSON-RPC session updates under
// `~/.grok/sessions/<urlencoded-workspace>/<session-id>/updates.jsonl`, with
// sibling `summary.json` (cwd, model, timestamps) and `signals.json`
// (session rollups incl. compaction totals).
//
// Update lines expose only a cumulative context counter
// (`params._meta.totalTokens`) — no input/output split — so each user turn's
// positive delta is recorded as input tokens (output/cache buckets stay 0).
// After compaction the counter rewinds; the deltas lost to that rewind are
// reconciled from `signals.json` as one extra event per session.
//
// Context attribution: chunk content (message text, thought text, tool-call
// payloads) is sized transiently (est bytes/4, never stored) and each turn's
// delta splits across messages + reasoning by those weights — system stays
// NULL (unobservable), matching the pi convention.
use std::fs;
use std::path::Path;

use rusqlite::Connection;
use serde_json::Value;

use super::claude_ctx::{content_bytes, est};
use super::{file_state_of, percent_decode, unchanged};
use crate::db::{insert_limit_readings, replace_file_events, set_file_state};
use crate::limits_artifact::grok_credit_window;
use crate::time::iso_to_epoch;
use crate::types::{CtxTokens, FileState, LimitReading, SourceScanResult, UsageEvent};

// Bump to force a full re-parse of every session when the parser changes (the
// byte-offset slot carries it through `unchanged`).
// 1: attribute each turn's delta across messages/reasoning by chunk weights.
const PARSER_VERSION: i64 = 1;

const MALFORMED_ARTIFACT_WARNING: &str = "grok: malformed or unsupported Source Artifact";
const SUPPORTED_UPDATE_KINDS: &[&str] = &[
    "agent_message_chunk",
    "agent_thought_chunk",
    "current_mode_update",
    "hook_execution",
    "image_compressed",
    "plan",
    "retry_state",
    "session_recap",
    "subagent_finished",
    "subagent_spawned",
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

    // Re-reading the whole log is free: the content-keyed PK absorbs every
    // repeat, and the file only rewrites on rotation.
    let readings: Vec<LimitReading> = content
        .lines()
        .filter(|line| line.contains(BILLING_MSG))
        .filter_map(billing_reading)
        .collect();

    // A write that fails says so in its own words — reporting it as an
    // unreadable log would blame the file for the database's trouble.
    if let Err(error) = insert_limit_readings(conn, &readings) {
        result.error = Some(format!("grok: could not record Limit Readings: {error}"));
        return;
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
    })
}

fn process_session(conn: &mut Connection, updates_path: &Path, result: &mut SourceScanResult) {
    let session_dir = match updates_path.parent() {
        Some(d) => d,
        None => return,
    };
    let signals_path = session_dir.join("signals.json");
    let summary_path = session_dir.join("summary.json");

    // Tokens come from updates.jsonl + signals.json; if neither changed the
    // session's events are already correct.
    let updates_state = FileState { byte_offset: PARSER_VERSION, ..file_state_of(updates_path) };
    let signals_state = file_state_of(&signals_path);
    let summary_state = file_state_of(&summary_path);
    if unchanged(conn, updates_path, &updates_state)
        && unchanged(conn, &signals_path, &signals_state)
        && unchanged(conn, &summary_path, &summary_state)
    {
        return;
    }

    let meta = match read_session_meta(session_dir, result) {
        Some(meta) => meta,
        None => return,
    };
    let mut events = match parse_updates(updates_path, &meta, result) {
        Some(events) => events,
        None => return,
    };
    if !append_signals_reconciliation(&signals_path, &meta, &mut events) {
        record_grok_warning(result);
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
    if signals_state.size > 0 || signals_state.mtime > 0 {
        let _ = set_file_state(conn, &signals_path.to_string_lossy(), signals_state);
    }
    if summary_state.size > 0 || summary_state.mtime > 0 {
        let _ = set_file_state(conn, &summary_path.to_string_lossy(), summary_state);
    }
}

struct SessionMeta {
    session_id: String,
    model: Option<String>,
    project: Option<String>,
    fallback_ts: i64,
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
    let mut fallback_ts = 0;
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
        if let Some(ts) = ["updated_at", "created_at"].iter().find_map(|key| {
            v.get(*key)
                .and_then(Value::as_str)
                .and_then(iso_to_epoch)
        }) {
            fallback_ts = ts;
        }
    }

    Some(SessionMeta { session_id, model, project, fallback_ts })
}

// One in-flight user turn: the cumulative counter's value when the turn
// started, the highest value seen while it ran, and the est-weights (bytes/4)
// of the chunk content observed during it — sized transiently, never stored.
struct Turn {
    baseline: i64,
    max_total: i64,
    ts: i64,
    index: usize,
    msg: i64,
    reas: i64,
}

impl Turn {
    fn start(baseline: i64, ts: i64, index: usize) -> Self {
        Turn { baseline, max_total: baseline, ts, index, msg: 0, reas: 0 }
    }
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

// Split a turn's counter delta by the observed chunk weights (pi convention:
// system is unobservable for grok, and a zero reasoning share stays NULL —
// never a fabricated 0). Messages takes the remainder, so the partition over
// the two observable categories is exact.
fn attribute_turn(msg: i64, reas: i64, delta: i64) -> CtxTokens {
    let total = msg + reas;
    if total <= 0 || delta <= 0 {
        return CtxTokens::default();
    }
    let reasoning = delta * reas / total;
    CtxTokens {
        messages: Some(delta - reasoning),
        reasoning: (reasoning > 0).then_some(reasoning),
        ..Default::default()
    }
}

fn supported_update(v: &Value) -> bool {
    matches!(
        v.get("method").and_then(Value::as_str),
        Some("session/update") | Some("_x.ai/session/update")
    )
        && v.pointer("/params/sessionId")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty())
        && v.pointer("/params/update/sessionUpdate")
            .and_then(Value::as_str)
            .is_some_and(|kind| SUPPORTED_UPDATE_KINDS.contains(&kind))
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
    let mut last_total: Option<i64> = None;
    let mut last_ts = meta.fallback_ts;
    let mut turn: Option<Turn> = None;
    let mut turn_index = 0usize;
    let mut missing_timestamp = false;

    let flush = |turn: Turn, events: &mut Vec<UsageEvent>, missing_timestamp: &mut bool| {
        let delta = turn.max_total.saturating_sub(turn.baseline);
        if delta > 0 {
            if turn.ts > 0 {
                let ctx = attribute_turn(turn.msg, turn.reas, delta);
                events.push(make_event(meta, updates_path, turn.index, delta, turn.ts, ctx));
            } else {
                *missing_timestamp = true;
            }
        }
    };

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

        if !supported_update(&v) {
            result.lines_skipped += 1;
            record_grok_warning(result);
            return None;
        }

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

        if let Some(total) = v.pointer("/params/_meta/totalTokens") {
            if total.as_i64().is_none_or(|total| total < 0) {
                result.lines_skipped += 1;
                record_grok_warning(result);
                return None;
            }
        }

        if v.pointer("/params/update/sessionUpdate").and_then(Value::as_str)
            == Some("user_message_chunk")
        {
            if let Some(t) = turn.take() {
                flush(t, &mut events, &mut missing_timestamp);
            }
            turn = Some(Turn::start(last_total.unwrap_or(0), ts, turn_index));
            turn_index += 1;
        }

        // Chunk content weighs into the running turn; pre-turn chunks (resumed
        // session) have no turn to belong to and stay unattributed.
        if let (Some(t), Some(u)) = (turn.as_mut(), v.pointer("/params/update")) {
            let (msg, reas) = update_weights(u);
            t.msg += msg;
            t.reas += reas;
        }

        let total = match v.pointer("/params/_meta/totalTokens").and_then(Value::as_i64) {
            Some(t) if t >= 0 => t,
            _ => continue,
        };
        // The counter rewinds on compaction/retry; treat it as monotonic and
        // let the signals.json reconciliation recover what the rewind hides.
        if last_total.is_some_and(|prev| total < prev) {
            continue;
        }
        if turn.is_none() && last_total.is_some_and(|prev| total > prev) {
            // Counter grew outside any observed turn (e.g. resumed session
            // whose user message predates this file's first line).
            turn = Some(Turn::start(last_total.unwrap_or(0), ts, turn_index));
            turn_index += 1;
        }
        if let Some(t) = turn.as_mut() {
            if total > t.max_total {
                t.max_total = total;
                t.ts = ts;
            }
        }
        last_total = Some(total);
        last_ts = ts;
    }

    if let Some(t) = turn.take() {
        flush(t, &mut events, &mut missing_timestamp);
    }

    // No turns detected but a counter exists (very old/truncated logs):
    // record the whole session as one event, unattributed (no turn observed
    // means no chunk weights to split it by).
    if events.is_empty() {
        if let Some(total) = last_total.filter(|&t| t > 0) {
            if last_ts > 0 {
                events.push(make_event(meta, updates_path, 0, total, last_ts, CtxTokens::default()));
            } else {
                missing_timestamp = true;
            }
        }
    }

    if missing_timestamp {
        record_grok_warning(result);
        None
    } else {
        Some(events)
    }
}

// Session rollup totals survive compaction; when they exceed what the update
// deltas captured, book the difference as one extra event so long sessions
// are not under-counted.
fn append_signals_reconciliation(
    signals_path: &Path,
    meta: &SessionMeta,
    events: &mut Vec<UsageEvent>,
) -> bool {
    let content = match fs::read_to_string(signals_path) {
        Ok(c) => c,
        Err(_) if !signals_path.exists() => return true,
        Err(_) => {
            return false;
        }
    };
    let v: Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return false,
    };

    let has_known_field = [
        "totalTokensBeforeCompaction",
        "totalTokens",
        "contextTokensUsed",
    ]
    .iter()
    .any(|key| v.get(*key).is_some());
    if !v.is_object() || !has_known_field {
        return false;
    }
    for key in [
        "totalTokensBeforeCompaction",
        "totalTokens",
        "contextTokensUsed",
    ] {
        if v.get(key)
            .is_some_and(|value| value.as_i64().is_none_or(|value| value < 0))
        {
            return false;
        }
    }

    let get = |key: &str| v.get(key).and_then(Value::as_i64).unwrap_or(0).max(0);
    let before = get("totalTokensBeforeCompaction");
    let total = get("totalTokens");
    let effective = match v.get("contextTokensUsed") {
        None => before.saturating_add(total),
        Some(ctx) => total.max(before.saturating_add(ctx.as_i64().unwrap_or(0).max(0))),
    };
    if effective <= 0 {
        return true;
    }

    let counted: i64 = events.iter().map(|e| e.input_tokens).sum();
    let extra = effective.saturating_sub(counted);
    if extra <= 0 {
        return true;
    }

    // Anchor to the last update activity, not signals.json's mtime, so the
    // delta stays on the same day across rescans of a live session.
    let ts = events.iter().map(|e| e.timestamp).max().unwrap_or(meta.fallback_ts);
    if ts <= 0 {
        return false;
    }
    let updates_path = signals_path.with_file_name("updates.jsonl");
    // Unattributed: the rollup delta stands for content lost to compaction,
    // which the update chunks never showed us.
    let mut event = make_event(meta, &updates_path, 0, extra, ts, CtxTokens::default());
    event.dedup_key = format!("grok:{}:signals", meta.session_id);
    events.push(event);
    true
}

fn make_event(
    meta: &SessionMeta,
    updates_path: &Path,
    turn_index: usize,
    input_tokens: i64,
    timestamp: i64,
    ctx: CtxTokens,
) -> UsageEvent {
    UsageEvent {
        dedup_key: format!("grok:{}:{}", meta.session_id, turn_index),
        source: "grok".to_string(),
        timestamp,
        model: meta.model.clone(),
        project: meta.project.clone(),
        api_calls: 1, // logs expose turn boundaries only, not API calls
        input_tokens,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_5m_tokens: 0,
        cache_write_1h_tokens: 0,
        source_file: updates_path.to_string_lossy().to_string(),
        session_id: Some(meta.session_id.clone()),
        reasoning_tokens: None,
        ctx,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_db;
    use std::path::PathBuf;
    use tempfile::tempdir;

    // Shapes mirror real ~/.grok/sessions data (2026-07): totals live at
    // params._meta.totalTokens, turn starts are user_message_chunk updates,
    // top-level timestamp is epoch seconds.
    fn update_line(ts: i64, kind: &str, total: Option<i64>) -> String {
        update_line_text(ts, kind, total, "x")
    }

    fn update_line_text(ts: i64, kind: &str, total: Option<i64>, text: &str) -> String {
        let meta = match total {
            Some(t) => format!(r#","_meta":{{"totalTokens":{t},"eventId":"e"}}"#),
            None => String::new(),
        };
        format!(
            r#"{{"timestamp":{ts},"method":"session/update","params":{{"sessionId":"s","update":{{"sessionUpdate":"{kind}","content":{{"type":"text","text":"{text}"}}}}{meta}}}}}"#
        )
    }

    fn write_session(
        root: &Path,
        workspace: &str,
        session_id: &str,
        updates: &[String],
        summary: Option<&str>,
        signals: Option<&str>,
    ) -> PathBuf {
        let dir = root.join(workspace).join(session_id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("updates.jsonl"), updates.join("\n") + "\n").unwrap();
        if let Some(s) = summary {
            std::fs::write(dir.join("summary.json"), s).unwrap();
        }
        if let Some(s) = signals {
            std::fs::write(dir.join("signals.json"), s).unwrap();
        }
        dir.join("updates.jsonl")
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
    fn per_turn_deltas_become_input_events() {
        let tmp = tempdir().unwrap();
        write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Falpha",
            "sess-1",
            &[
                update_line(100, "user_message_chunk", None),
                update_line(101, "agent_thought_chunk", Some(2500)),
                update_line(102, "agent_message_chunk", Some(4000)),
                update_line(200, "user_message_chunk", None),
                update_line(201, "agent_message_chunk", Some(9000)),
            ],
            Some(r#"{"info":{"id":"sess-1","cwd":"/Users/dev/alpha"},"current_model_id":"grok-4.5","updated_at":"2026-07-10T20:49:57Z"}"#),
            None,
        );

        let (_app, conn, res) = scan(tmp.path());
        assert!(res.error.is_none());
        assert_eq!(res.events_inserted, 2);

        let rows: Vec<(String, i64, i64, String, Option<String>)> = conn
            .prepare("SELECT dedup_key, timestamp, input_tokens, model, project FROM events ORDER BY timestamp")
            .unwrap()
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();

        assert_eq!(rows[0].0, "grok:sess-1:0");
        assert_eq!(rows[0].1, 102); // ts of the max-total observation
        assert_eq!(rows[0].2, 4000); // 0 → 4000
        assert_eq!(rows[0].3, "grok-4.5");
        assert_eq!(rows[0].4, Some("/Users/dev/alpha".to_string()));
        assert_eq!(rows[1].0, "grok:sess-1:1");
        assert_eq!(rows[1].2, 5000); // 4000 → 9000
    }

    #[test]
    fn turn_delta_splits_across_messages_and_reasoning_by_chunk_weights() {
        let tmp = tempdir().unwrap();
        write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Falpha",
            "sess-r",
            &[
                // weights: user 8/4=2 msg, thought 32/4=8 reas, agent 24/4=6 msg
                update_line_text(100, "user_message_chunk", None, "uuuuuuuu"),
                update_line_text(101, "agent_thought_chunk", Some(2500), &"t".repeat(32)),
                update_line_text(102, "agent_message_chunk", Some(4000), &"m".repeat(24)),
                // second turn: no thinking observed → reasoning stays NULL
                update_line_text(200, "user_message_chunk", None, "uuuuuuuu"),
                update_line_text(201, "agent_message_chunk", Some(9000), &"m".repeat(24)),
            ],
            Some(r#"{"info":{"id":"sess-r","cwd":"/Users/dev/alpha"},"current_model_id":"grok-4.5","updated_at":"2026-07-10T20:49:57Z"}"#),
            None,
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

        // Turn 1: delta 4000, weights msg 8 / reas 8 → reasoning 2000,
        // messages take the remainder so the partition equals the delta.
        assert_eq!(rows[0], (Some(2_000), Some(2_000), None));
        // Turn 2: delta 5000, no thought chunks → zero share stays NULL.
        assert_eq!(rows[1], (Some(5_000), None, None));
    }

    #[test]
    fn counter_rewind_is_ignored_and_signals_reconciles() {
        let tmp = tempdir().unwrap();
        write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Falpha",
            "sess-2",
            &[
                update_line(100, "user_message_chunk", None),
                update_line(101, "agent_message_chunk", Some(10000)),
                // compaction: counter rewinds, then grows below the old max
                update_line(102, "agent_message_chunk", Some(3000)),
                update_line(103, "agent_message_chunk", Some(8000)),
            ],
            None,
            // rollup says 15000 were really consumed
            Some(r#"{"totalTokensBeforeCompaction":10000,"contextTokensUsed":5000}"#),
        );

        let (_app, conn, res) = scan(tmp.path());
        assert!(res.error.is_none());
        assert_eq!(res.events_inserted, 2); // turn + reconciliation

        let (turn_tokens,): (i64,) = conn
            .query_row(
                "SELECT input_tokens FROM events WHERE dedup_key = 'grok:sess-2:0'",
                [],
                |r| Ok((r.get(0)?,)),
            )
            .unwrap();
        assert_eq!(turn_tokens, 10000);

        let (extra,): (i64,) = conn
            .query_row(
                "SELECT input_tokens FROM events WHERE dedup_key = 'grok:sess-2:signals'",
                [],
                |r| Ok((r.get(0)?,)),
            )
            .unwrap();
        assert_eq!(extra, 5000); // 15000 rollup − 10000 counted
    }

    #[test]
    fn workspace_dir_decodes_when_summary_missing() {
        let tmp = tempdir().unwrap();
        write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Fbeta",
            "sess-3",
            &[
                update_line(100, "user_message_chunk", None),
                update_line(101, "agent_message_chunk", Some(500)),
            ],
            None,
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
                update_line(100, "user_message_chunk", None),
                update_line(101, "agent_message_chunk", Some(1000)),
            ],
            None,
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
        content.push_str(&update_line(200, "user_message_chunk", None));
        content.push('\n');
        content.push_str(&update_line(201, "agent_message_chunk", Some(2500)));
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
                update_line(100, "user_message_chunk", None),
                update_line(101, "agent_message_chunk", Some(500)),
            ],
            None,
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
    fn unsupported_update_shape_reports_a_grok_specific_warning() {
        let tmp = tempdir().unwrap();
        write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Funsupported",
            "sess-unsupported",
            &[r#"{"timestamp":100,"method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"future_update"}}}"#.to_string()],
            None,
            None,
        );

        let (_app, _conn, res) = scan(tmp.path());
        assert_eq!(res.events_inserted, 0);
        assert!(res
            .error
            .as_deref()
            .is_some_and(|error| error.contains("grok") && error.contains("unsupported")));
    }

    #[test]
    fn missing_update_timestamp_is_not_booked() {
        let tmp = tempdir().unwrap();
        write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Funtimestamped",
            "sess-untimestamped",
            &[
                update_line(0, "user_message_chunk", None),
                update_line(0, "agent_message_chunk", Some(500)),
            ],
            None,
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
    fn malformed_summary_and_signals_warn_without_deleting_history() {
        let tmp = tempdir().unwrap();
        let updates = write_session(
            tmp.path(),
            "%2FUsers%2Fdev%2Fsiblings",
            "sess-siblings",
            &[
                update_line(100, "user_message_chunk", None),
                update_line(101, "agent_message_chunk", Some(500)),
            ],
            Some(
                r#"{"info":{"id":"sess-siblings"},"current_model_id":"grok","updated_at":"2026-07-10T20:49:57Z"}"#,
            ),
            None,
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

        std::fs::write(
            updates.parent().unwrap().join("summary.json"),
            r#"{"info":{"id":"sess-siblings"},"current_model_id":"grok","updated_at":"2026-07-10T20:49:57Z"}"#,
        )
        .unwrap();
        std::fs::write(updates.parent().unwrap().join("signals.json"), "not json").unwrap();
        let signals_result = scan_sessions(&mut conn, tmp.path());
        assert!(signals_result.error.is_some());
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
        // Two requests at the same percentage are one row: the PK is the
        // reading's content, so the table holds the fill-curve, not the traffic.
        std::fs::write(
            &log,
            weekly("2026-07-10T20:49:57.123Z", Some("14")) + "\n"
                + &weekly("2026-07-10T21:49:57.123Z", Some("14")) + "\n",
        )
        .unwrap();
        let mut conn = open_db(&tmp.path().join("ledger.db")).unwrap();

        scan_grok(&mut conn, &sessions, &log);
        assert_eq!(readings(&conn).len(), 1);
        scan_grok(&mut conn, &sessions, &log);
        assert_eq!(readings(&conn).len(), 1, "a re-scan re-reads nothing and inserts nothing");
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
                update_line(100, "user_message_chunk", None),
                update_line(101, "agent_message_chunk", Some(4000)),
            ],
            None,
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


