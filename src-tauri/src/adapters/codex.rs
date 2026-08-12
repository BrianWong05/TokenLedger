use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::Value;

use super::ctx::{self, est};
use super::{find_jsonl_by_file_identity, unchanged};
use crate::db;
use crate::limits_artifact::window_key;
use crate::time::iso_to_epoch;
use crate::types::{FileState, LimitReading, SourceScanResult, UsageEvent};

const PARSER_VERSION: i64 = 1;

/// One `rate_limits` slot → a Reading, or None when that window does not exist.
/// A null slot is an absent window, never a window at zero; a slot missing the
/// duration or the reset instant cannot be keyed or placed on a time axis, so it
/// is likewise no window. The key comes from the duration and never from the
/// `primary`/`secondary` slot: the slot is a position, not a window — in the
/// local corpus `primary` carries the 7-day window in 88% of observations and
/// the 5-hour one in the rest (#104). ponytail: `resets_in_seconds` (Codex
/// ≤ 0.47) is not read — reading it as an epoch would date the window to 1970.
/// Convert it here if pre-0.48 Artifacts ever turn up.
fn slot_reading(slot: Option<&Value>, observed_at: i64, plan: Option<&str>) -> Option<LimitReading> {
    let slot = slot.filter(|s| !s.is_null())?;
    let used_pct = slot.get("used_percent").and_then(|v| v.as_f64())?;
    let window_minutes = slot.get("window_minutes").and_then(|v| v.as_i64())?;
    // Absolute unix seconds since Codex 0.48; a 0.48.0 pre-release wrote the
    // same field as an RFC3339 string.
    let resets_at = slot
        .get("resets_at")
        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(iso_to_epoch)))?;
    Some(LimitReading {
        source: "codex".to_string(),
        window_key: window_key(window_minutes),
        window_minutes: Some(window_minutes),
        used_pct,
        resets_at,
        observed_at,
        via: "logs".to_string(),
        plan: plan.map(str::to_string),
    })
}

/// Scan all `*.jsonl` rollout files under the ordered Session roots.
/// Missing directories → zero events, no error. Only the default root donates
/// Limit Readings; later roots donate Usage and Context only.
pub fn scan_codex(conn: &mut Connection, session_roots: &[PathBuf]) -> SourceScanResult {
    let mut result = SourceScanResult::default();
    let mut aliases = Vec::new();
    let mut seen = HashSet::new();
    let mut winners = HashSet::new();
    for (index, root) in session_roots.iter().enumerate() {
        let mut files = Vec::new();
        find_jsonl_by_file_identity(root, &mut files, &mut aliases, &mut seen);
        for path in files {
            winners.insert(path.clone());
            match scan_file(conn, &path, index == 0) {
                Ok((inserted, skipped)) => {
                    result.events_inserted += inserted;
                    result.lines_skipped += skipped;
                }
                Err(e) => result.error = Some(e),
            }
        }
    }
    for path in aliases {
        if winners.contains(&path) {
            continue;
        }
        let source_file = path.to_string_lossy();
        if let Err(error) = db::clear_file_state(conn, &source_file)
            .and_then(|_| db::clear_ctx_tools_for_file(conn, &source_file))
        {
            result.error = Some(error.to_string());
        }
    }
    result
}

struct ParsedCodexFile {
    events: Vec<UsageEvent>,
    tool_rows: Vec<(String, i64, i64, i64)>,
    readings: Vec<LimitReading>,
    skipped: u64,
}

// Pure parse core (no Connection): codex re-parses each changed file in full.
fn parse_file(content: &str, file_stem: &str, path_str: &str) -> ParsedCodexFile {
    let mut tool_rows: Vec<(String, i64, i64, i64)> = Vec::new();
    let mut call_names: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    let mut events: Vec<UsageEvent> = Vec::new();
    let mut readings: Vec<LimitReading> = Vec::new();
    let mut skipped: u64 = 0;
    let mut model = String::from("unknown");
    let mut cwd: Option<String> = None;
    // Forked and subagent rollouts replay their parent's history under rewritten
    // envelope timestamps (#104), so a replayed window could present itself as
    // current. The content-keyed PK absorbs the duplicate rows; this guard is
    // what keeps `observed_at` honest. Usage events are unaffected — they have
    // always been read from these files.
    let mut replay = false;
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
                replay = v.pointer("/payload/forked_from_id").is_some_and(|f| !f.is_null())
                    || v.pointer("/payload/parent_thread_id").is_some_and(|p| !p.is_null())
                    || v.pointer("/payload/thread_source").and_then(|s| s.as_str())
                        == Some("subagent");
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

                // `rate_limits` is a sibling of `info`, so it must be read before
                // any of the token-count skips below: a snapshot whose token
                // deltas are all zero still carries a perfectly good Limit
                // Reading, and the `info: null` control lines carry them too.
                if !replay {
                    if let Some(limits) = payload.get("rate_limits").filter(|l| !l.is_null()) {
                        // A `limit_id` other than "codex" is a different
                        // entitlement: `"premium"` is the fingerprint of a
                        // refused 429 carrying an empty snapshot (#104), and
                        // taking it as "the newest reading" would blank a gauge
                        // that had good data one line earlier.
                        if limits.get("limit_id").and_then(|i| i.as_str()) == Some("codex") {
                            let observed_at = v
                                .get("timestamp")
                                .and_then(|t| t.as_str())
                                .and_then(iso_to_epoch)
                                .unwrap_or(0);
                            let plan = limits.get("plan_type").and_then(|p| p.as_str());
                            readings.extend(
                                [limits.get("primary"), limits.get("secondary")]
                                    .into_iter()
                                    .flat_map(|slot| slot_reading(slot, observed_at, plan)),
                            );
                        }
                    }
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
                    model: Some(model.clone()),
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

    ParsedCodexFile { events, tool_rows, readings, skipped }
}

/// Returns (events_inserted, lines_skipped) for one file.
fn scan_file(
    conn: &mut Connection,
    path: &Path,
    include_limit_readings: bool,
) -> Result<(u64, u64), String> {
    let path_str = path.to_string_lossy().to_string();
    let parser_repair = db::get_file_state(conn, &path_str)
        .map_err(|e| e.to_string())?
        .is_some_and(|previous| previous.byte_offset != PARSER_VERSION);
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let size = meta.len() as i64;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // Unchanged file → skip (full re-parse only on change).
    let state = FileState {
        size,
        mtime,
        byte_offset: PARSER_VERSION,
    };
    if unchanged(conn, path, &state) {
        return Ok((0, 0));
    }

    // Codex re-parses changed or parser-versioned files in full.
    db::clear_ctx_tools_for_file(conn, &path_str).map_err(|e| e.to_string())?;

    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let file_stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    let parsed = parse_file(&content, &file_stem, &path_str);

    let inserted = if parser_repair {
        db::replace_file_events(conn, &path_str, &parsed.events).map_err(|e| e.to_string())?;
        parsed.events.len() as u64
    } else {
        db::insert_events(conn, &parsed.events).map_err(|e| e.to_string())?
    };
    if include_limit_readings {
        db::insert_limit_readings(conn, &parsed.readings).map_err(|e| e.to_string())?;
    }
    db::add_ctx_tool_rows(conn, "codex", &path_str, &parsed.tool_rows).map_err(|e| e.to_string())?;
    db::set_file_state(conn, &path_str, state).map_err(|e| e.to_string())?;
    Ok((inserted, parsed.skipped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_db;
    #[cfg(unix)]
    use crate::queries::{ctx_tools, Filters};

    fn fixture_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex")
    }

    #[test]
    fn codex_cumulative_delta_and_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();

        let r = scan_codex(&mut conn, &[fixture_root()]);
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
        let r2 = scan_codex(&mut conn, &[fixture_root()]);
        assert_eq!(r2.events_inserted, 0, "unchanged file skipped");
        let n2: i64 = conn
            .query_row("SELECT COUNT(*) FROM events WHERE source='codex'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n2, 2);

        // A parser upgrade replaces stale rows from this file.
        let source_file = fixture_root().join("rollout-fixture.jsonl");
        let source_file = source_file.to_string_lossy();
        conn.execute(
            "INSERT INTO events (dedup_key, source, timestamp, model, source_file) \
             VALUES ('codex:stale-replay', 'codex', 1, 'unknown', ?1)",
            [&source_file],
        )
        .unwrap();
        let old_offset = std::fs::metadata(source_file.as_ref()).unwrap().len() as i64;
        conn.execute(
            "UPDATE scanned_files SET byte_offset = ?2 WHERE path = ?1",
            rusqlite::params![source_file.as_ref(), old_offset],
        )
        .unwrap();

        let repaired = scan_codex(&mut conn, &[fixture_root()]);
        assert_eq!(repaired.events_inserted, 2);
        let (rows, stale): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), COUNT(*) FILTER (WHERE dedup_key = 'codex:stale-replay') \
                 FROM events WHERE source = 'codex'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((rows, stale), (2, 0));
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
        let r = scan_codex(&mut conn, std::slice::from_ref(&root));
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
        let r = scan_codex(&mut conn, std::slice::from_ref(&root));
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
        let r = scan_codex(&mut conn, std::slice::from_ref(&root));
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
        let r = scan_codex(&mut conn, std::slice::from_ref(&root));
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
        scan_codex(&mut conn, std::slice::from_ref(&root));
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
        scan_codex(&mut conn, std::slice::from_ref(&root));
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
        scan_codex(&mut conn, std::slice::from_ref(&root));
        let (est2, calls2): (i64, i64) = conn.query_row(
            "SELECT est_tokens, calls FROM ctx_tools WHERE source='codex' AND name='shell'",
            [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((est2, calls2), (est1, calls1), "re-parse replaced, not doubled");
    }

    #[cfg(unix)]
    #[test]
    fn codex_counts_a_hard_linked_rollout_once() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        let rollout = write_rollout(&root, "rollout-b.jsonl", &[
            r#"{"type":"response_item","timestamp":"2026-05-03T09:00:00.000Z","payload":{"type":"function_call","call_id":"c1","name":"shell","arguments":"{\"command\":[\"ls\"]}"}}"#,
            r#"{"type":"response_item","timestamp":"2026-05-03T09:00:01.000Z","payload":{"type":"function_call_output","call_id":"c1","output":"done"}}"#,
            r#"{"type":"event_msg","timestamp":"2026-05-03T09:00:02.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":10,"total_tokens":110}}}}"#,
        ]);
        let alias = root.join("rollout-a.jsonl");
        std::fs::hard_link(&rollout, &alias).unwrap();

        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();
        let scan = scan_codex(&mut conn, std::slice::from_ref(&root));
        let tools = ctx_tools(&conn, &Filters::default()).unwrap();
        let source_file: String = conn
            .query_row("SELECT source_file FROM events WHERE source='codex'", [], |row| row.get(0))
            .unwrap();

        assert_eq!((scan.events_inserted, tools.len(), tools[0].calls), (1, 1, 1));
        assert_eq!(source_file, alias.to_string_lossy());
    }

    #[cfg(unix)]
    #[test]
    fn codex_replaces_context_when_alias_winner_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        let rollout = write_rollout(&root, "rollout-b.jsonl", &[
            r#"{"type":"response_item","timestamp":"2026-05-03T09:00:00.000Z","payload":{"type":"function_call","call_id":"c1","name":"shell","arguments":"{\"command\":[\"ls\"]}"}}"#,
            r#"{"type":"response_item","timestamp":"2026-05-03T09:00:01.000Z","payload":{"type":"function_call_output","call_id":"c1","output":"done"}}"#,
            r#"{"type":"event_msg","timestamp":"2026-05-03T09:00:02.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":10,"total_tokens":110}}}}"#,
        ]);
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();

        scan_codex(&mut conn, std::slice::from_ref(&root));
        let alias = root.join("rollout-a.jsonl");
        std::fs::hard_link(&rollout, &alias).unwrap();
        scan_codex(&mut conn, std::slice::from_ref(&root));

        let tools = ctx_tools(&conn, &Filters::default()).unwrap();
        assert_eq!((tools.len(), tools[0].calls), (1, 1));

        std::fs::remove_file(alias).unwrap();
        scan_codex(&mut conn, std::slice::from_ref(&root));
        db::prune_missing_files(&conn).unwrap();

        let tools = ctx_tools(&conn, &Filters::default()).unwrap();
        assert_eq!((tools.len(), tools[0].calls), (1, 1));
    }

    #[cfg(unix)]
    #[test]
    fn codex_scans_ordered_roots_without_duplicate_usage_or_extra_limits() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let default_root = tmp.path().join("default/sessions");
        let relocated_root = tmp.path().join("relocated/sessions");
        let relocated_link = tmp.path().join("configured-sessions");

        let default_rollout = write_rollout(
            &default_root,
            "rollout-default.jsonl",
            &[REAL_BLOCK],
        );
        std::fs::create_dir_all(&relocated_root).unwrap();
        std::fs::hard_link(
            &default_rollout,
            relocated_root.join("rollout-overlap.jsonl"),
        )
        .unwrap();
        let extra_limit = limits_line(
            r#"{"limit_id":"codex","primary":{"used_percent":42.0,"window_minutes":10080,"resets_at":1786879486},"secondary":null,"plan_type":"plus"}"#,
            "2026-08-10T03:20:19.385Z",
        );
        write_rollout(
            &relocated_root,
            "rollout-extra.jsonl",
            &[
                r#"{"type":"response_item","timestamp":"2026-08-10T03:20:17.000Z","payload":{"type":"function_call","call_id":"c1","name":"shell","arguments":"{\"command\":[\"pwd\"]}"}}"#,
                r#"{"type":"response_item","timestamp":"2026-08-10T03:20:18.000Z","payload":{"type":"function_call_output","call_id":"c1","output":"done"}}"#,
                &extra_limit,
                "not json",
            ],
        );
        symlink(&relocated_root, &relocated_link).unwrap();

        let roots = vec![
            default_root.clone(),
            default_root.clone(),
            relocated_link.clone(),
        ];
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();
        let context_totals = |conn: &Connection| -> (i64, i64) {
            conn.query_row(
                "SELECT COUNT(*), COALESCE(SUM(est_tokens), 0) FROM ctx_tools WHERE source='codex'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
        };
        let first = scan_codex(&mut conn, &roots);
        let summary = crate::queries::summary(&conn, &Filters::default()).unwrap();
        let tools = ctx_tools(&conn, &Filters::default()).unwrap();
        let context = context_totals(&conn);
        let (readings, used_pct): (i64, f64) = conn
            .query_row(
                "SELECT COUNT(*), MAX(used_pct) FROM limit_readings WHERE source='codex'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let source_file: String = conn
            .query_row(
                "SELECT source_file FROM events WHERE session_id='rollout-default'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!((first.events_inserted, first.lines_skipped), (2, 1));
        assert!(first.error.is_none());
        assert_eq!((summary.total_tokens, summary.requests), (300, 2));
        assert_eq!((tools.len(), tools[0].calls), (1, 1));
        assert_eq!((readings, used_pct), (1, 100.0));
        assert_eq!(source_file, default_rollout.to_string_lossy());

        let second = scan_codex(&mut conn, &roots);
        let second_summary = crate::queries::summary(&conn, &Filters::default()).unwrap();
        assert_eq!(second.events_inserted, 0);
        assert_eq!(
            (second_summary.total_tokens, second_summary.requests),
            (summary.total_tokens, summary.requests)
        );
        assert_eq!(context_totals(&conn), context);

        std::fs::remove_file(&relocated_link).unwrap();
        let after_disappearance = scan_codex(&mut conn, &roots);
        let durable = crate::queries::summary(&conn, &Filters::default()).unwrap();
        assert!(after_disappearance.error.is_none());
        assert_eq!((durable.total_tokens, durable.requests), (300, 2));
        assert_eq!(context_totals(&conn), context);
    }

    // ---- Limit Readings (#104 ingest rules) ----

    // The real nine-field block, verbatim from a local rollout (2026-08-10,
    // cli_version 0.147.0-alpha.6.5): one weekly window in `primary`, a null
    // `secondary`, and the credit state the whole corpus carries.
    const REAL_BLOCK: &str = r#"{"type":"event_msg","timestamp":"2026-08-10T03:16:19.385Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":50,"total_tokens":150}},"rate_limits":{"limit_id":"codex","limit_name":null,"primary":{"used_percent":100.0,"window_minutes":10080,"resets_at":1786879486},"secondary":null,"credits":{"has_credits":false,"unlimited":false,"balance":"0"},"individual_limit":null,"spend_control_reached":null,"plan_type":"plus","rate_limit_reached_type":null}}}"#;

    fn limits_line(rate_limits: &str, ts: &str) -> String {
        format!(
            r#"{{"type":"event_msg","timestamp":"{ts}","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":100,"cached_input_tokens":0,"output_tokens":50,"total_tokens":150}}}},"rate_limits":{rate_limits}}}}}"#
        )
    }

    fn readings_of(content: &str) -> Vec<LimitReading> {
        parse_file(content, "rollout", "/p/rollout.jsonl").readings
    }

    #[test]
    fn the_real_block_yields_one_reading_per_window_that_exists() {
        let readings = readings_of(&format!("{REAL_BLOCK}\n"));
        assert_eq!(readings.len(), 1, "a null `secondary` is a window that does not exist");
        assert_eq!(
            readings[0],
            LimitReading {
                source: "codex".to_string(),
                window_key: "w10080".to_string(),
                window_minutes: Some(10080),
                used_pct: 100.0,
                resets_at: 1_786_879_486,
                // The envelope timestamp, never the filename date: 123 of 212
                // local files have a name-date that differs from it (#104).
                observed_at: 1_786_331_779,
                via: "logs".to_string(),
                plan: Some("plus".to_string()),
            }
        );
    }

    #[test]
    fn a_premium_record_yields_no_readings() {
        // The fingerprint of a refused 429: an empty snapshot against a limit
        // family whose usage the server does not report (#104).
        let content = limits_line(
            r#"{"limit_id":"premium","limit_name":null,"primary":null,"secondary":null,"plan_type":"plus"}"#,
            "2026-07-03T10:16:42.271Z",
        ) + "\n";
        assert_eq!(readings_of(&content), vec![]);
    }

    #[test]
    fn windows_are_classified_by_duration_not_by_slot() {
        // Both slots populated, the 5-hour window in `secondary` — the slot
        // carries no window meaning, so the key comes from the duration alone.
        let content = limits_line(
            r#"{"limit_id":"codex","primary":{"used_percent":80.0,"window_minutes":10080,"resets_at":1786879486},"secondary":{"used_percent":61.0,"window_minutes":300,"resets_at":1783537868},"plan_type":"plus"}"#,
            "2026-08-10T03:16:19.385Z",
        ) + "\n";
        let keys: Vec<String> = readings_of(&content).into_iter().map(|r| r.window_key).collect();
        assert_eq!(keys, vec!["w10080", "w300"]);
    }

    #[test]
    fn durations_snap_to_the_canonical_set_within_five_percent() {
        let key = |minutes: i64| {
            let content = limits_line(
                &format!(
                    r#"{{"limit_id":"codex","primary":{{"used_percent":1.0,"window_minutes":{minutes},"resets_at":1786879486}},"secondary":null}}"#
                ),
                "2026-08-10T03:16:19.385Z",
            ) + "\n";
            readings_of(&content).remove(0).window_key
        };
        assert_eq!(key(300), "w300");
        assert_eq!(key(10080), "w10080");
        assert_eq!(key(10081), "w10080", "upstream rounding drift is the same window");
        assert_eq!(key(4321), "w4321", "an unrecognised duration is kept, not treated as corrupt");
    }

    #[test]
    fn a_zero_delta_snapshot_still_carries_its_reading() {
        // The token-count skips must not swallow the limits block: `info: null`
        // control lines and duplicate snapshots both carry one.
        let content = [
            r#"{"type":"event_msg","timestamp":"2026-08-10T03:16:19.385Z","payload":{"type":"token_count","info":null,"rate_limits":{"limit_id":"codex","primary":{"used_percent":42.0,"window_minutes":10080,"resets_at":1786879486},"secondary":null}}}"#,
        ]
        .join("\n") + "\n";
        let parsed = parse_file(&content, "rollout", "/p/rollout.jsonl");
        assert_eq!(parsed.events.len(), 0, "an info:null line is still no usage event");
        assert_eq!(parsed.readings.len(), 1, "but it is a Limit Reading");
        assert_eq!(parsed.readings[0].used_pct, 42.0);
    }

    #[test]
    fn a_subagent_replay_contributes_no_readings() {
        let content = [
            r#"{"type":"session_meta","timestamp":"2026-07-03T06:37:20.985Z","payload":{"id":"019f26b2","parent_thread_id":"019f2681","thread_source":"subagent","cwd":"/p"}}"#,
            REAL_BLOCK,
        ]
        .join("\n") + "\n";
        let parsed = parse_file(&content, "rollout", "/p/rollout.jsonl");
        assert_eq!(parsed.readings, vec![], "a replay must not donate a fresh observed_at");
        assert_eq!(parsed.events.len(), 1, "its usage is read as it always was");
    }

    #[test]
    fn readings_dedup_on_content_across_scans_and_repeats() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sessions");
        // Two requests at the same used_percent in the same epoch, then a third
        // that advanced the fill.
        let window = |pct: &str, ts: &str| {
            limits_line(
                &format!(
                    r#"{{"limit_id":"codex","primary":{{"used_percent":{pct},"window_minutes":10080,"resets_at":1786879486}},"secondary":null,"plan_type":"plus"}}"#
                ),
                ts,
            )
        };
        let lines = [
            window("40.0", "2026-08-10T03:16:19.385Z"),
            window("40.0", "2026-08-10T03:17:19.385Z"),
            window("41.0", "2026-08-10T03:18:19.385Z"),
        ];
        write_rollout(
            &root,
            "rollout-2026-08-10-lim.jsonl",
            &lines.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();
        scan_codex(&mut conn, std::slice::from_ref(&root));
        let rows = |conn: &Connection| -> i64 {
            conn.query_row("SELECT COUNT(*) FROM limit_readings", [], |r| r.get(0)).unwrap()
        };
        assert_eq!(rows(&conn), 2, "the repeat at an unchanged percentage costs no row");

        // Re-scanning the same file — after clearing scan state, so the parse
        // genuinely re-runs — inserts nothing new.
        conn.execute("DELETE FROM scanned_files", []).unwrap();
        scan_codex(&mut conn, std::slice::from_ref(&root));
        assert_eq!(rows(&conn), 2, "a re-parse is absorbed by the content-keyed PK");
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
