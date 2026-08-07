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
use std::fs;
use std::path::Path;

use rusqlite::Connection;
use serde_json::Value;

use super::{file_state_of, percent_decode, unchanged};
use crate::db::{replace_file_events, set_file_state};
use crate::time::iso_to_epoch;
use crate::types::{SourceScanResult, UsageEvent};

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

pub fn scan_grok(conn: &mut Connection, sessions_root: &Path) -> SourceScanResult {
    let mut result = SourceScanResult::default();
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

fn process_session(conn: &mut Connection, updates_path: &Path, result: &mut SourceScanResult) {
    let session_dir = match updates_path.parent() {
        Some(d) => d,
        None => return,
    };
    let signals_path = session_dir.join("signals.json");
    let summary_path = session_dir.join("summary.json");

    // Tokens come from updates.jsonl + signals.json; if neither changed the
    // session's events are already correct.
    let updates_state = file_state_of(updates_path);
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
// started, and the highest value seen while it ran.
struct Turn {
    baseline: i64,
    max_total: i64,
    ts: i64,
    index: usize,
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
                events.push(make_event(meta, updates_path, turn.index, delta, turn.ts));
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
            turn = Some(Turn {
                baseline: last_total.unwrap_or(0),
                max_total: last_total.unwrap_or(0),
                ts,
                index: turn_index,
            });
            turn_index += 1;
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
            turn = Some(Turn {
                baseline: last_total.unwrap_or(0),
                max_total: last_total.unwrap_or(0),
                ts,
                index: turn_index,
            });
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
    // record the whole session as one event.
    if events.is_empty() {
        if let Some(total) = last_total.filter(|&t| t > 0) {
            if last_ts > 0 {
                events.push(make_event(meta, updates_path, 0, total, last_ts));
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
    let mut event = make_event(meta, &updates_path, 0, extra, ts);
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
        ctx: Default::default(),
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
        let meta = match total {
            Some(t) => format!(r#","_meta":{{"totalTokens":{t},"eventId":"e"}}"#),
            None => String::new(),
        };
        format!(
            r#"{{"timestamp":{ts},"method":"session/update","params":{{"sessionId":"s","update":{{"sessionUpdate":"{kind}","content":{{"type":"text","text":"x"}}}}{meta}}}}}"#
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

    fn scan(root: &Path) -> (tempfile::TempDir, rusqlite::Connection, SourceScanResult) {
        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        let res = scan_grok(&mut conn, root);
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
        let first = scan_grok(&mut conn, tmp.path());
        assert_eq!(first.events_inserted, 1);

        // Same content → skipped entirely.
        let second = scan_grok(&mut conn, tmp.path());
        assert_eq!(second.events_inserted, 0);

        // Session grows (new turn, distinct mtime via size change) → rescan
        // replaces the file's events, no duplicates.
        let mut content = std::fs::read_to_string(&updates).unwrap();
        content.push_str(&update_line(200, "user_message_chunk", None));
        content.push('\n');
        content.push_str(&update_line(201, "agent_message_chunk", Some(2500)));
        content.push('\n');
        std::fs::write(&updates, content).unwrap();

        let third = scan_grok(&mut conn, tmp.path());
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
        let res = scan_grok(&mut conn, Path::new("/nonexistent/grok/sessions"));
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
        let first = scan_grok(&mut conn, tmp.path());
        assert_eq!(first.events_inserted, 1);

        std::fs::write(&updates, "not json\n").unwrap();
        let second = scan_grok(&mut conn, tmp.path());
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
        assert_eq!(scan_grok(&mut conn, tmp.path()).events_inserted, 1);

        std::fs::write(updates.parent().unwrap().join("summary.json"), "not json").unwrap();
        let summary_result = scan_grok(&mut conn, tmp.path());
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
        let signals_result = scan_grok(&mut conn, tmp.path());
        assert!(signals_result.error.is_some());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
