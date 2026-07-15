use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::Value;

use super::ctx::{self, est};
use crate::db;
use crate::time::iso_to_epoch;
use crate::types::{FileState, SourceScanResult, UsageEvent};

/// Scan all `*.jsonl` rollout files under `sessions_root` (recursively).
/// Missing directory → zero events, no error.
pub fn scan_codex(conn: &mut Connection, sessions_root: &Path) -> SourceScanResult {
    let mut result = SourceScanResult::default();
    let mut files = Vec::new();
    collect_jsonl(sessions_root, &mut files);
    for path in files {
        match scan_file(conn, &path) {
            Ok((inserted, skipped)) => {
                result.events_inserted += inserted;
                result.lines_skipped += skipped;
            }
            Err(e) => result.error = Some(e),
        }
    }
    result
}

fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return, // missing dir is not an error
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

struct ParsedCodexFile {
    events: Vec<UsageEvent>,
    tool_rows: Vec<(String, i64, i64, i64)>,
    skipped: u64,
}

// Pure parse core (no Connection): codex re-parses each changed file in full.
fn parse_file(content: &str, file_stem: &str, path_str: &str) -> ParsedCodexFile {
    let mut tool_rows: Vec<(String, i64, i64, i64)> = Vec::new();
    let mut call_names: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    let mut events: Vec<UsageEvent> = Vec::new();
    let mut skipped: u64 = 0;
    let mut model = String::from("unknown");
    let mut cwd: Option<String> = None;
    // Previous cumulative snapshot (raw, unclamped).
    let mut prev_input: i64 = 0;
    let mut prev_cached: i64 = 0;
    let mut prev_output: i64 = 0;
    let mut prev_reasoning: i64 = 0;
    // Running composition for context attribution (est. tokens, bytes/4).
    // Toolcall content is a subset of messages (schema subset rule); shares
    // normalize over known content so the unattributable system prompt is
    // absorbed proportionally and the partition sums to billed exactly.
    let mut msg_est: i64 = 0;
    let mut tool_est: i64 = 0;
    let mut reas_est: i64 = 0;

    let mut offset: usize = 0;
    for line in content.split_inclusive('\n') {
        let line_offset = offset;
        offset += line.len();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let typ = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match typ {
            "session_meta" => {
                if let Some(c) = v.pointer("/payload/cwd").and_then(|c| c.as_str()) {
                    cwd = Some(c.to_string());
                }
            }
            "turn_context" => {
                if let Some(m) = v.pointer("/payload/model").and_then(|m| m.as_str()) {
                    model = m.to_string();
                }
            }
            "response_item" => {
                let payload = match v.get("payload") {
                    Some(p) => p,
                    None => continue,
                };
                let bytes = serde_json::to_string(payload).map(|s| s.len()).unwrap_or(0);
                match payload.get("type").and_then(|t| t.as_str()) {
                    Some("message") => {
                        msg_est += est(bytes);
                        if payload.get("role").and_then(|r| r.as_str()) == Some("user") {
                            reas_est = 0; // user turn: reasoning leaves the context
                        }
                    }
                    Some("function_call") | Some("function_call_output") => {
                        msg_est += est(bytes); // subset rule: tool ⊆ messages
                        tool_est += est(bytes);
                        let ts = v.get("timestamp").and_then(|t| t.as_str()).and_then(iso_to_epoch).unwrap_or(0);
                        let is_call = payload.get("type").and_then(|t| t.as_str()) == Some("function_call");
                        let name = if is_call {
                            let n = payload.get("name").and_then(|n| n.as_str()).unwrap_or("unknown").to_string();
                            if let Some(id) = payload.get("call_id").and_then(|c| c.as_str()) {
                                call_names.insert(id.to_string(), n.clone());
                            }
                            n
                        } else {
                            payload
                                .get("call_id")
                                .and_then(|c| c.as_str())
                                .and_then(|id| call_names.get(id))
                                .cloned()
                                .unwrap_or_else(|| "unknown".to_string())
                        };
                        tool_rows.push((name, est(bytes), if is_call { 1 } else { 0 }, ts));
                    }
                    Some("reasoning") => reas_est += est(bytes),
                    _ => {}
                }
            }
            "event_msg" => {
                let payload = match v.get("payload") {
                    Some(p) => p,
                    None => continue,
                };
                if payload.get("type").and_then(|t| t.as_str()) != Some("token_count") {
                    continue;
                }
                // Skip info:null control lines.
                let info = match payload.get("info") {
                    Some(i) if !i.is_null() => i,
                    _ => continue,
                };
                let usage = match info.get("total_token_usage") {
                    Some(u) => u,
                    None => continue,
                };
                let cur_input = usage.get("input_tokens").and_then(|x| x.as_i64()).unwrap_or(0);
                let cur_cached = usage
                    .get("cached_input_tokens")
                    .and_then(|x| x.as_i64())
                    .unwrap_or(0);
                let cur_output = usage.get("output_tokens").and_then(|x| x.as_i64()).unwrap_or(0);

                let d_input = (cur_input - prev_input).max(0);
                let d_cached = (cur_cached - prev_cached).max(0);
                let d_output = (cur_output - prev_output).max(0);
                prev_input = cur_input;
                prev_cached = cur_cached;
                prev_output = cur_output;

                // cached is a subset of input; keep them mutually exclusive.
                let input = (d_input - d_cached).max(0);
                let cache_read = d_cached;
                let output = d_output;
                // Duplicate snapshots and degenerate rows produce an all-zero delta.
                // prev_reasoning is intentionally NOT advanced before this skip: a
                // reasoning-only advance on a skipped line rides along with the
                // next token-bearing event instead of being lost.
                if input == 0 && cache_read == 0 && output == 0 {
                    continue;
                }

                // reasoning_output_tokens is cumulative like the other fields.
                // Absent field => this source/build doesn't report reasoning => None.
                let reasoning = usage
                    .get("reasoning_output_tokens")
                    .and_then(|x| x.as_i64())
                    .map(|cur| {
                        let d = (cur - prev_reasoning).max(0);
                        prev_reasoning = cur;
                        d
                    });

                let ts = v
                    .get("timestamp")
                    .and_then(|t| t.as_str())
                    .and_then(iso_to_epoch)
                    .unwrap_or(0);

                let billed = input + cache_read; // codex reports no cache writes
                let total = msg_est + reas_est;
                let ctx = if total > 0 && billed > 0 {
                    let mut ctx = ctx::Composition {
                        msg: msg_est,
                        tool: tool_est,
                        reas: reas_est,
                        ..Default::default()
                    }
                    .attribute(billed);
                    // Codex logs cannot observe the system prompt, mcp, or skill
                    // content: null those categories (None vs Some(0) is
                    // load-bearing — the e2e suite asserts it).
                    ctx.system = None;
                    ctx.mcp = None;
                    ctx.skills = None;
                    ctx
                } else {
                    crate::types::CtxTokens::default()
                };

                events.push(UsageEvent {
                    dedup_key: format!("codex:{}:{}", file_stem, line_offset),
                    source: "codex".to_string(),
                    timestamp: ts,
                    model: model.clone(),
                    project: cwd.clone(),
                    api_calls: 1,
                    input_tokens: input,
                    output_tokens: output,
                    cache_read_tokens: cache_read,
                    cache_write_5m_tokens: 0,
                    cache_write_1h_tokens: 0,
                    source_file: path_str.to_string(),
                    session_id: Some(file_stem.to_string()),
                    reasoning_tokens: reasoning,
                    ctx,
                });
            }
            _ => {}
        }
    }

    ParsedCodexFile { events, tool_rows, skipped }
}

/// Returns (events_inserted, lines_skipped) for one file.
fn scan_file(conn: &mut Connection, path: &Path) -> Result<(u64, u64), String> {
    let path_str = path.to_string_lossy().to_string();
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let size = meta.len() as i64;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // Unchanged file → skip (full re-parse only on change).
    if let Ok(Some(state)) = db::get_file_state(conn, &path_str) {
        if state.size == size && state.mtime == mtime {
            return Ok((0, 0));
        }
    }

    // Codex re-parses changed files in full: replace this file's tool rows.
    db::clear_ctx_tools_for_file(conn, &path_str).map_err(|e| e.to_string())?;

    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let file_stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    let parsed = parse_file(&content, &file_stem, &path_str);

    let inserted = db::insert_events(conn, &parsed.events).map_err(|e| e.to_string())?;
    db::add_ctx_tool_rows(conn, "codex", &path_str, &parsed.tool_rows).map_err(|e| e.to_string())?;
    db::set_file_state(
        conn,
        &path_str,
        FileState {
            size,
            mtime,
            byte_offset: size,
        },
    )
    .map_err(|e| e.to_string())?;
    Ok((inserted, parsed.skipped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_db;

    fn fixture_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex")
    }

    #[test]
    fn codex_cumulative_delta_and_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();

        let r = scan_codex(&mut conn, &fixture_root());
        assert_eq!(r.error, None, "no error expected");
        assert_eq!(r.events_inserted, 2, "info:null + duplicate snapshot dropped");
        assert_eq!(r.lines_skipped, 0, "no malformed lines");

        let (n, si, sc, so): (i64, i64, i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), SUM(input_tokens), SUM(cache_read_tokens), SUM(output_tokens) \
                 FROM events WHERE source='codex'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(n, 2);
        // Adapter total equals the file's FINAL snapshot, not the naive sum.
        assert_eq!(si, 150, "input excludes cached, summed via deltas");
        assert_eq!(sc, 100, "cache_read = cached deltas");
        assert_eq!(so, 30, "output deltas");
        assert_eq!(si + sc, 250, "input+cache_read == final cumulative input_tokens");

        // Model tracked from turn_context; project from session_meta.
        let model: String = conn
            .query_row("SELECT DISTINCT model FROM events WHERE source='codex'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(model, "gpt-5.4");
        let project: String = conn
            .query_row("SELECT DISTINCT project FROM events WHERE source='codex'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(project, "/Users/dev/projects/alpha");

        // Timestamps parsed from the token_count line's ISO field.
        let (min_ts, max_ts): (i64, i64) = conn
            .query_row("SELECT MIN(timestamp), MAX(timestamp) FROM events WHERE source='codex'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(min_ts, 1777122208);
        assert_eq!(max_ts, 1777122215);

        // Re-scan is idempotent: unchanged file inserts nothing, totals stable.
        let r2 = scan_codex(&mut conn, &fixture_root());
        assert_eq!(r2.events_inserted, 0, "unchanged file skipped");
        let n2: i64 = conn
            .query_row("SELECT COUNT(*) FROM events WHERE source='codex'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n2, 2);
    }

    fn write_rollout(dir: &std::path::Path, name: &str, lines: &[&str]) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, lines.join("\n") + "\n").unwrap();
        p
    }

    #[test]
    fn codex_reasoning_deltas_and_session_id() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        write_rollout(&root, "rollout-2026-04-23-abc.jsonl", &[
            r#"{"type":"session_meta","timestamp":"2026-04-23T12:23:20.000Z","payload":{"id":"sess-1","cwd":"/Users/dev/projects/alpha"}}"#,
            r#"{"type":"turn_context","timestamp":"2026-04-23T12:23:25.000Z","payload":{"model":"gpt-5.4"}}"#,
            r#"{"type":"event_msg","timestamp":"2026-04-23T12:23:28.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":50,"reasoning_output_tokens":10,"total_tokens":150}}}}"#,
            r#"{"type":"event_msg","timestamp":"2026-04-23T12:23:35.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":200,"cached_input_tokens":60,"output_tokens":90,"reasoning_output_tokens":25,"total_tokens":290}}}}"#,
        ]);
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();
        let r = scan_codex(&mut conn, &root);
        assert_eq!(r.events_inserted, 2);

        let rows: Vec<(Option<String>, Option<i64>)> = {
            let mut stmt = conn
                .prepare("SELECT session_id, reasoning_tokens FROM events WHERE source='codex' ORDER BY timestamp")
                .unwrap();
            let it = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap();
            it.collect::<rusqlite::Result<Vec<_>>>().unwrap()
        };
        assert_eq!(rows[0], (Some("rollout-2026-04-23-abc".to_string()), Some(10)));
        assert_eq!(rows[1], (Some("rollout-2026-04-23-abc".to_string()), Some(15)));
    }

    #[test]
    fn codex_reasoning_only_snapshot_rides_along() {
        // A snapshot whose input/cached/output are unchanged but whose
        // reasoning advanced must not lose those reasoning tokens: the line is
        // skipped, prev_reasoning stays put, and the next token-bearing event
        // books the accumulated delta.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        write_rollout(&root, "rollout-2026-04-25-ghi.jsonl", &[
            r#"{"type":"event_msg","timestamp":"2026-04-25T09:00:00.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":50,"reasoning_output_tokens":10,"total_tokens":150}}}}"#,
            r#"{"type":"event_msg","timestamp":"2026-04-25T09:00:05.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":50,"reasoning_output_tokens":25,"total_tokens":150}}}}"#,
            r#"{"type":"event_msg","timestamp":"2026-04-25T09:00:10.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":150,"cached_input_tokens":0,"output_tokens":80,"reasoning_output_tokens":30,"total_tokens":230}}}}"#,
        ]);
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();
        let r = scan_codex(&mut conn, &root);
        assert_eq!(r.events_inserted, 2, "reasoning-only line still skipped as an event");
        let total: i64 = conn
            .query_row(
                "SELECT SUM(reasoning_tokens) FROM events WHERE source='codex'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(total, 30, "sum of reasoning deltas equals the final cumulative value");
    }

    #[test]
    fn codex_missing_reasoning_field_is_null() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        write_rollout(&root, "rollout-2026-04-24-def.jsonl", &[
            r#"{"type":"event_msg","timestamp":"2026-04-24T09:00:00.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":50,"total_tokens":150}}}}"#,
        ]);
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();
        let r = scan_codex(&mut conn, &root);
        assert_eq!(r.events_inserted, 1);
        let rt: Option<i64> = conn
            .query_row("SELECT reasoning_tokens FROM events WHERE source='codex'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rt, None, "absent field means not-reported, never 0");
    }

    #[test]
    fn codex_attributes_context_from_response_items() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        write_rollout(&root, "rollout-2026-05-01-ctx.jsonl", &[
            r#"{"type":"response_item","timestamp":"2026-05-01T09:00:00.000Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}}"#,
            r#"{"type":"response_item","timestamp":"2026-05-01T09:00:01.000Z","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}]}}"#,
            r#"{"type":"response_item","timestamp":"2026-05-01T09:00:02.000Z","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"ls\"]}"}}"#,
            r#"{"type":"response_item","timestamp":"2026-05-01T09:00:03.000Z","payload":{"type":"function_call_output","output":"cccccccccccccccccccccccccccccccccccccccc"}}"#,
            r#"{"type":"event_msg","timestamp":"2026-05-01T09:00:04.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":900,"cached_input_tokens":100,"output_tokens":50,"total_tokens":950}}}}"#,
        ]);
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();
        let r = scan_codex(&mut conn, &root);
        assert_eq!(r.events_inserted, 1);

        let (cm, cs, cr, ct, ca): (i64, Option<i64>, i64, i64, Option<i64>) = conn
            .query_row(
                "SELECT ctx_messages, ctx_system, ctx_reasoning, ctx_toolcalls, ctx_agents \
                 FROM events WHERE source='codex'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        // billed = Δinput(900, incl. cached) → partition exact over msg+reas.
        assert_eq!(cm + cr, 900, "messages + reasoning == billed (system NULL, absorbed)");
        assert!(cr > 0, "reasoning share attributed");
        assert!(ct > 0 && ct <= cm, "toolcalls ⊆ messages");
        assert_eq!(cs, None, "codex cannot attribute a system prompt");
        assert_eq!(ca, None, "codex has no agent concept");
    }

    #[test]
    fn codex_user_message_resets_reasoning() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        write_rollout(&root, "rollout-2026-05-02-rst.jsonl", &[
            r#"{"type":"response_item","timestamp":"2026-05-02T09:00:00.000Z","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}]}}"#,
            r#"{"type":"response_item","timestamp":"2026-05-02T09:00:01.000Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}}"#,
            r#"{"type":"event_msg","timestamp":"2026-05-02T09:00:02.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":500,"cached_input_tokens":0,"output_tokens":10,"total_tokens":510}}}}"#,
        ]);
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();
        scan_codex(&mut conn, &root);
        let cr: i64 = conn
            .query_row("SELECT ctx_reasoning FROM events WHERE source='codex'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cr, 0, "user turn strips prior reasoning from context");
    }

    #[test]
    fn codex_populates_ctx_tools_idempotently() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        write_rollout(&root, "rollout-2026-05-03-tools.jsonl", &[
            r#"{"type":"response_item","timestamp":"2026-05-03T09:00:00.000Z","payload":{"type":"function_call","call_id":"c1","name":"shell","arguments":"{\"command\":[\"ls\"]}"}}"#,
            r#"{"type":"response_item","timestamp":"2026-05-03T09:00:01.000Z","payload":{"type":"function_call_output","call_id":"c1","output":"cccccccccccccccccccc"}}"#,
            r#"{"type":"event_msg","timestamp":"2026-05-03T09:00:02.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":10,"total_tokens":110}}}}"#,
        ]);
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();
        scan_codex(&mut conn, &root);
        let (est1, calls1): (i64, i64) = conn.query_row(
            "SELECT est_tokens, calls FROM ctx_tools WHERE source='codex' AND name='shell'",
            [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert!(est1 > 0);
        assert_eq!(calls1, 1);

        // Touch the file (size/mtime change) → full re-parse must REPLACE rows.
        let fp = root.join("rollout-2026-05-03-tools.jsonl");
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(&fp).unwrap();
            writeln!(f, r#"{{"type":"event_msg","timestamp":"2026-05-03T09:00:03.000Z","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":150,"cached_input_tokens":0,"output_tokens":20,"total_tokens":170}}}}}}}}"#).unwrap();
        }
        scan_codex(&mut conn, &root);
        let (est2, calls2): (i64, i64) = conn.query_row(
            "SELECT est_tokens, calls FROM ctx_tools WHERE source='codex' AND name='shell'",
            [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((est2, calls2), (est1, calls1), "re-parse replaced, not doubled");
    }

    // ---- pure parse_file core (no DB) ----

    #[test]
    fn parse_file_cumulative_deltas_across_two_token_counts() {
        let content = [
            r#"{"type":"event_msg","timestamp":"2026-04-23T12:23:28.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":50,"total_tokens":150}}}}"#,
            r#"{"type":"event_msg","timestamp":"2026-04-23T12:23:35.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":200,"cached_input_tokens":60,"output_tokens":90,"total_tokens":290}}}}"#,
        ].join("\n") + "\n";
        let parsed = parse_file(&content, "rollout", "/p/rollout.jsonl");
        assert_eq!(parsed.events.len(), 2);
        // Line 1: Δinput 100 − Δcached 20 → input 80, cache_read 20, output 50.
        assert_eq!(
            (parsed.events[0].input_tokens, parsed.events[0].cache_read_tokens, parsed.events[0].output_tokens),
            (80, 20, 50)
        );
        // Line 2: Δinput 100 − Δcached 40 → input 60, cache_read 40, output 40.
        assert_eq!(
            (parsed.events[1].input_tokens, parsed.events[1].cache_read_tokens, parsed.events[1].output_tokens),
            (60, 40, 40)
        );
    }

    #[test]
    fn parse_file_zero_delta_line_skipped_reasoning_rides_along() {
        // Middle line's input/cached/output are unchanged (all-zero delta) but
        // reasoning advanced: it is skipped as an event, prev_reasoning stays,
        // and the next token-bearing event books the accumulated reasoning.
        let content = [
            r#"{"type":"event_msg","timestamp":"2026-04-25T09:00:00.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":50,"reasoning_output_tokens":10,"total_tokens":150}}}}"#,
            r#"{"type":"event_msg","timestamp":"2026-04-25T09:00:05.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":50,"reasoning_output_tokens":25,"total_tokens":150}}}}"#,
            r#"{"type":"event_msg","timestamp":"2026-04-25T09:00:10.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":150,"cached_input_tokens":0,"output_tokens":80,"reasoning_output_tokens":30,"total_tokens":230}}}}"#,
        ].join("\n") + "\n";
        let parsed = parse_file(&content, "rollout", "/p/rollout.jsonl");
        assert_eq!(parsed.events.len(), 2, "middle zero-delta line skipped");
        let total_reas: i64 = parsed.events.iter().filter_map(|e| e.reasoning_tokens).sum();
        assert_eq!(total_reas, 30, "reasoning-only advance rides along to the next event");
    }

    #[test]
    fn parse_file_partition_equivalence() {
        // Nonzero msg/tool/reas ests → the shared math partitions billed exactly
        // (messages + reasoning), toolcalls ⊆ messages, and the unobservable
        // categories stay NULL (not Some(0)).
        let content = [
            r#"{"type":"response_item","timestamp":"2026-05-01T09:00:00.000Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}}"#,
            r#"{"type":"response_item","timestamp":"2026-05-01T09:00:01.000Z","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}]}}"#,
            r#"{"type":"response_item","timestamp":"2026-05-01T09:00:02.000Z","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"ls\"]}"}}"#,
            r#"{"type":"response_item","timestamp":"2026-05-01T09:00:03.000Z","payload":{"type":"function_call_output","output":"cccccccccccccccccccccccccccccccccccccccc"}}"#,
            r#"{"type":"event_msg","timestamp":"2026-05-01T09:00:04.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":900,"cached_input_tokens":100,"output_tokens":50,"total_tokens":950}}}}"#,
        ].join("\n") + "\n";
        let parsed = parse_file(&content, "rollout", "/p/rollout.jsonl");
        assert_eq!(parsed.events.len(), 1);
        let ctx = parsed.events[0].ctx;
        let m = ctx.messages.unwrap();
        let r = ctx.reasoning.unwrap();
        assert_eq!(m + r, 900, "messages + reasoning == billed (system absorbed)");
        assert!(r > 0, "reasoning share attributed");
        assert!(ctx.toolcalls.unwrap() <= m, "toolcalls ⊆ messages");
        assert_eq!(ctx.system, None, "system unobservable in codex → NULL");
        assert_eq!(ctx.mcp, None);
        assert_eq!(ctx.skills, None);
    }
}
