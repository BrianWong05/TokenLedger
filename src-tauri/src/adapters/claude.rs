use crate::db::{get_file_state, insert_events_keep_max_output, set_file_state};
use crate::types::{FileState, SourceScanResult, UsageEvent};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

pub fn scan_claude(conn: &mut Connection, projects_root: &Path) -> SourceScanResult {
    let mut result = SourceScanResult::default();
    let mut files = Vec::new();
    find_jsonl(projects_root, &mut files);
    files.sort();
    for path in files {
        if let Err(e) = scan_file(conn, &path, &mut result) {
            result.error = Some(e.to_string());
            return result;
        }
    }
    result
}

fn find_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return, // missing directory => zero events, not an error
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            find_jsonl(&p, out);
        } else if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            out.push(p);
        }
    }
}

fn scan_file(
    conn: &mut Connection,
    path: &Path,
    result: &mut SourceScanResult,
) -> rusqlite::Result<()> {
    use std::io::{Read, Seek, SeekFrom};

    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return Ok(()),
    };
    let size = meta.len() as i64;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let path_str = path.to_string_lossy().to_string();

    // ~/.claude/projects/<encoded-dir>/<session>.jsonl: the encoded dir is the
    // file's parent basename. Used verbatim (never decoded — provably lossy)
    // as the project fallback when a line has no `cwd`.
    let encoded_dir = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    // Resume from stored offset only when the file has not shrunk; otherwise
    // reparse from the start (idempotent via dedup keys).
    let start = match get_file_state(conn, &path_str)? {
        Some(fs) if size >= fs.size => fs.byte_offset,
        _ => 0,
    };

    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Ok(()),
    };
    if file.seek(SeekFrom::Start(start as u64)).is_err() {
        return Ok(());
    }
    let mut buf = Vec::new();
    if file.read_to_end(&mut buf).is_err() {
        return Ok(());
    }

    // Consume only complete newline-terminated lines; a trailing partial line
    // is left for the next scan.
    let consumed = buf
        .iter()
        .rposition(|&b| b == b'\n')
        .map(|i| i + 1)
        .unwrap_or(0);

    let mut events = Vec::new();
    for line in buf[..consumed].split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        match parse_line(line, &path_str, &encoded_dir) {
            Ok(Some(ev)) => events.push(ev),
            Ok(None) => {}                          // non-assistant or synthetic: ignore
            Err(()) => result.lines_skipped += 1,   // malformed line
        }
    }

    let inserted = insert_events_keep_max_output(conn, &events)?;
    result.events_inserted += inserted;

    let new_offset = start + consumed as i64;
    set_file_state(conn, &path_str, FileState { size, mtime, byte_offset: new_offset })?;
    Ok(())
}

fn parse_line(line: &[u8], source_file: &str, encoded_dir: &str) -> Result<Option<UsageEvent>, ()> {
    let v: serde_json::Value = serde_json::from_slice(line).map_err(|_| ())?;
    if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
        return Ok(None);
    }
    let msg = &v["message"];
    let usage = &msg["usage"];

    let input = usage["input_tokens"].as_i64().unwrap_or(0);
    let output = usage["output_tokens"].as_i64().unwrap_or(0);
    let cache_read = usage["cache_read_input_tokens"].as_i64().unwrap_or(0);
    let cc_total = usage["cache_creation_input_tokens"].as_i64().unwrap_or(0);
    let cc = &usage["cache_creation"];
    let (cw5m, cw1h) = if cc.is_object() {
        (
            cc["ephemeral_5m_input_tokens"].as_i64().unwrap_or(0),
            cc["ephemeral_1h_input_tokens"].as_i64().unwrap_or(0),
        )
    } else {
        // sub-object absent: whole creation total is 5m-TTL
        (cc_total, 0)
    };

    // <synthetic> error placeholders have all-zero usage: skip, don't count.
    if input == 0 && output == 0 && cache_read == 0 && cw5m == 0 && cw1h == 0 {
        return Ok(None);
    }

    let id = match msg["id"].as_str() {
        Some(s) if !s.is_empty() => s,
        _ => return Ok(None),
    };
    let dedup_key = match v.get("requestId").and_then(|r| r.as_str()) {
        Some(r) => format!("claude:{id}:{r}"),
        None => format!("claude:{id}"),
    };
    let model = msg["model"].as_str().unwrap_or("unknown").to_string();
    let project = Some(match v.get("cwd").and_then(|c| c.as_str()) {
        Some(cwd) => rollup_worktree(cwd),
        None => encoded_dir.to_string(), // fallback: raw dash-encoded dir name, not decoded
    });
    let timestamp = match v.get("timestamp").and_then(|t| t.as_str()).and_then(crate::time::iso_to_epoch) {
        Some(ts) => ts,
        None => return Ok(None),
    };

    let session_id = v
        .get("sessionId")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());

    Ok(Some(UsageEvent {
        dedup_key,
        source: "claude".to_string(),
        timestamp,
        model,
        project,
        api_calls: 1,
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_write_5m_tokens: cw5m,
        cache_write_1h_tokens: cw1h,
        source_file: source_file.to_string(),
        session_id,
        reasoning_tokens: None,
        ctx: Default::default(),
    }))
}

fn rollup_worktree(cwd: &str) -> String {
    match cwd.find("/.claude/worktrees/") {
        Some(i) => cwd[..i].to_string(),
        None => cwd.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_db;
    use std::io::Write;

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude/projects")
    }

    #[test]
    fn parses_dedups_splits_and_skips() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(&dir.path().join("t.db")).unwrap();
        let res = scan_claude(&mut conn, &fixtures());

        assert_eq!(res.error, None);
        assert_eq!(res.events_inserted, 5);
        assert_eq!(res.lines_skipped, 1);

        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 5);

        // duplicate message across two files deduped to one row
        let dup: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE dedup_key = 'claude:msg_dup:req_9'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dup, 1);

        // explicit 5m/1h split preserved
        let (m5, h1): (i64, i64) = conn
            .query_row(
                "SELECT cache_write_5m_tokens, cache_write_1h_tokens FROM events WHERE dedup_key = 'claude:msg_aaa:req_1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((m5, h1), (4, 6));

        // absent cache_creation sub-object => whole creation total in 5m
        let (m5b, h1b): (i64, i64) = conn
            .query_row(
                "SELECT cache_write_5m_tokens, cache_write_1h_tokens FROM events WHERE dedup_key = 'claude:msg_bbb:req_2'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((m5b, h1b), (80, 0));

        // <synthetic> all-zero line skipped
        let syn: i64 = conn
            .query_row("SELECT COUNT(*) FROM events WHERE model = '<synthetic>'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(syn, 0);

        // worktree cwd rolled up to parent repo
        let proj: String = conn
            .query_row(
                "SELECT project FROM events WHERE dedup_key = 'claude:msg_ddd:req_3'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(proj, "/Users/dev/projects/beta");

        // missing requestId => fallback dedup key
        let fb: i64 = conn
            .query_row("SELECT COUNT(*) FROM events WHERE dedup_key = 'claude:msg_ccc'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fb, 1);

        // timestamp parsed from the ISO `timestamp` field (2026-06-01T10:00:00Z)
        let ts: i64 = conn
            .query_row(
                "SELECT timestamp FROM events WHERE dedup_key = 'claude:msg_aaa:req_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ts, 1780308000);
    }

    #[test]
    fn resumes_after_append_and_ignores_trailing_partial() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(&dir.path().join("t.db")).unwrap();
        let proj = dir.path().join("projects/x");
        std::fs::create_dir_all(&proj).unwrap();
        let logp = proj.join("s.jsonl");
        let root = dir.path().join("projects");

        let line1 = r#"{"type":"assistant","requestId":"req_a","timestamp":"2026-06-01T10:00:00.000Z","cwd":"/Users/dev/projects/x","message":{"id":"msg_r1","model":"claude-opus-4-8","usage":{"input_tokens":10,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        let line2 = r#"{"type":"assistant","requestId":"req_b","timestamp":"2026-06-01T11:00:00.000Z","cwd":"/Users/dev/projects/x","message":{"id":"msg_r2","model":"claude-opus-4-8","usage":{"input_tokens":20,"output_tokens":2,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;

        // first write: line1 complete
        std::fs::write(&logp, format!("{line1}\n")).unwrap();
        let r1 = scan_claude(&mut conn, &root);
        assert_eq!(r1.events_inserted, 1);

        // append line2 WITHOUT a trailing newline -> partial, must be ignored
        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&logp).unwrap();
            write!(f, "{line2}").unwrap();
        }
        let r2 = scan_claude(&mut conn, &root);
        assert_eq!(r2.events_inserted, 0);

        // complete line2 with the newline -> now consumed on resume
        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&logp).unwrap();
            writeln!(f).unwrap();
        }
        let r3 = scan_claude(&mut conn, &root);
        assert_eq!(r3.events_inserted, 1);

        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 2);
    }

    #[test]
    fn falls_back_to_encoded_dir_name_when_cwd_absent() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(&dir.path().join("t.db")).unwrap();
        let root = dir.path().join("projects");
        let proj = root.join("-Users-dev-projects-gamma");
        std::fs::create_dir_all(&proj).unwrap();

        // usage-bearing assistant line with NO `cwd` field
        let line = r#"{"type":"assistant","requestId":"req_g","timestamp":"2026-06-03T09:00:00.000Z","message":{"id":"msg_ggg","model":"claude-opus-4-8","usage":{"input_tokens":11,"output_tokens":2,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        std::fs::write(proj.join("s.jsonl"), format!("{line}\n")).unwrap();

        let res = scan_claude(&mut conn, &root);
        assert_eq!(res.events_inserted, 1);

        // cwd absent => raw dash-encoded project-dir basename, verbatim (not decoded, not None)
        let project: String = conn
            .query_row(
                "SELECT project FROM events WHERE dedup_key = 'claude:msg_ggg:req_g'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(project, "-Users-dev-projects-gamma");
    }

    #[test]
    fn keeps_max_output_tokens_across_content_block_lines() {
        // One turn is logged as several assistant lines sharing message.id+requestId,
        // with a growing output_tokens snapshot; the final (largest) is the true count.
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(&dir.path().join("t.db")).unwrap();
        let root = dir.path().join("projects");
        let proj = root.join("x");
        std::fs::create_dir_all(&proj).unwrap();
        let logp = proj.join("s.jsonl");

        // identical id/requestId/input/cache; only output_tokens grows: 2 -> 4626
        let text_block = r#"{"type":"assistant","requestId":"req_z","timestamp":"2026-06-04T09:00:00.000Z","cwd":"/Users/dev/projects/x","message":{"id":"msg_zzz","model":"claude-opus-4-8","usage":{"input_tokens":30,"output_tokens":2,"cache_read_input_tokens":5,"cache_creation_input_tokens":0}}}"#;
        let tool_block = r#"{"type":"assistant","requestId":"req_z","timestamp":"2026-06-04T09:00:00.000Z","cwd":"/Users/dev/projects/x","message":{"id":"msg_zzz","model":"claude-opus-4-8","usage":{"input_tokens":30,"output_tokens":4626,"cache_read_input_tokens":5,"cache_creation_input_tokens":0}}}"#;
        std::fs::write(&logp, format!("{text_block}\n{tool_block}\n")).unwrap();

        // two content-block lines for one turn count as ONE distinct new event
        let r1 = scan_claude(&mut conn, &root);
        assert_eq!(r1.events_inserted, 1);

        // exactly one row for the key, carrying the MAX output (4626, not 2, not 4628)
        let (rows, out): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), MAX(output_tokens) FROM events WHERE dedup_key = 'claude:msg_zzz:req_z'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(rows, 1);
        assert_eq!(out, 4626);

        // idempotent: a second scan of the same file inserts nothing and stays at 4626
        let r2 = scan_claude(&mut conn, &root);
        assert_eq!(r2.events_inserted, 0);
        let out2: i64 = conn
            .query_row(
                "SELECT output_tokens FROM events WHERE dedup_key = 'claude:msg_zzz:req_z'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(out2, 4626);
    }

    #[test]
    fn captures_session_id_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(&dir.path().join("t.db")).unwrap();
        let root = dir.path().join("projects");
        let proj = root.join("x");
        std::fs::create_dir_all(&proj).unwrap();

        let with_sid = r#"{"type":"assistant","sessionId":"sess-cl-1","requestId":"req_s1","timestamp":"2026-06-05T09:00:00.000Z","cwd":"/Users/dev/projects/x","message":{"id":"msg_s1","model":"claude-opus-4-8","usage":{"input_tokens":10,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        let without_sid = r#"{"type":"assistant","requestId":"req_s2","timestamp":"2026-06-05T09:01:00.000Z","cwd":"/Users/dev/projects/x","message":{"id":"msg_s2","model":"claude-opus-4-8","usage":{"input_tokens":20,"output_tokens":2,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        std::fs::write(proj.join("s.jsonl"), format!("{with_sid}\n{without_sid}\n")).unwrap();

        let res = scan_claude(&mut conn, &root);
        assert_eq!(res.events_inserted, 2);

        let (sid, rt): (Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT session_id, reasoning_tokens FROM events WHERE dedup_key='claude:msg_s1:req_s1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(sid, Some("sess-cl-1".to_string()));
        assert_eq!(rt, None, "Claude does not report reasoning separately");

        let sid2: Option<String> = conn
            .query_row(
                "SELECT session_id FROM events WHERE dedup_key='claude:msg_s2:req_s2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sid2, None);
    }
}
