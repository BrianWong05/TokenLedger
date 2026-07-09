use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::Value;

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

    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let file_stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut events: Vec<UsageEvent> = Vec::new();
    let mut skipped: u64 = 0;
    let mut model = String::from("unknown");
    let mut cwd: Option<String> = None;
    // Previous cumulative snapshot (raw, unclamped).
    let mut prev_input: i64 = 0;
    let mut prev_cached: i64 = 0;
    let mut prev_output: i64 = 0;

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
                if input == 0 && cache_read == 0 && output == 0 {
                    continue;
                }

                let ts = v
                    .get("timestamp")
                    .and_then(|t| t.as_str())
                    .and_then(iso_to_epoch)
                    .unwrap_or(0);

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
                    source_file: path_str.clone(),
                    session_id: None,
                    reasoning_tokens: None,
                });
            }
            _ => {}
        }
    }

    let inserted = db::insert_events(conn, &events).map_err(|e| e.to_string())?;
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
    Ok((inserted, skipped))
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
}
