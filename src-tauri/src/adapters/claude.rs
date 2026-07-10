use super::claude_ctx::{self, Composition};
use crate::db::{get_file_state, insert_events_keep_max_output, set_file_state};
use crate::types::{CtxTokens, FileState, SourceScanResult, UsageEvent};
use rusqlite::Connection;
use std::collections::HashMap;
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

    // Context attribution (spec 2026-07-10): feed every line through the
    // running composition; attribute each API call at first sight of its
    // dedup_key (content the call produces is its output, not its input).
    let is_agent_file = path_str.contains("/subagents/");
    let mut events = Vec::new();
    let mut comps: HashMap<String, Composition> = HashMap::new();
    let mut tool_names: HashMap<String, String> = HashMap::new();
    let mut resources: Vec<(&'static str, String, i64)> = Vec::new();
    let mut attr_by_key: HashMap<String, CtxTokens> = HashMap::new();

    for line in buf[..consumed].split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_slice(line) {
            Ok(v) => v,
            Err(_) => {
                result.lines_skipped += 1;
                continue;
            }
        };
        // Session key: per-line sessionId, else the file stem (one session per file).
        let sid = v
            .get("sessionId")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
            });
        if !comps.contains_key(&sid) {
            // The persisted composition exists only to survive byte-offset resumes.
            // A full parse from byte 0 rebuilds it from scratch — loading here would
            // double-count content and make a stale taint permanent.
            let c = if start > 0 {
                // Mid-file resume with no persisted state: composition is unknowable —
                // taint the session so attribution stays NULL instead of guessing.
                match claude_ctx::load_composition(conn, &sid)? {
                    Some(c) => c,
                    None => Composition { tainted: true, ..Default::default() },
                }
            } else {
                Composition::default()
            };
            comps.insert(sid.clone(), c);
        }
        let comp = comps.get_mut(&sid).expect("inserted above");
        let line_ts = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .and_then(crate::time::iso_to_epoch)
            .unwrap_or(0);

        match v.get("type").and_then(|t| t.as_str()) {
            Some("user") => claude_ctx::apply_user_line(comp, &v, &tool_names),
            Some("system") => {
                if v.get("subtype").and_then(|s| s.as_str()) == Some("compact_boundary") {
                    comp.reset_compact();
                }
            }
            Some("assistant") => {
                if let Some(mut ev) = parse_line_event(&v, &path_str, &encoded_dir) {
                    let billed = ev.input_tokens
                        + ev.cache_read_tokens
                        + ev.cache_write_5m_tokens
                        + ev.cache_write_1h_tokens;
                    comp.init_system(billed);
                    let mut ctx = *attr_by_key
                        .entry(ev.dedup_key.clone())
                        .or_insert_with(|| comp.attribute(billed));
                    let sidechain = is_agent_file
                        || v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true);
                    if sidechain {
                        ctx.agents = Some(billed);
                    }
                    ev.ctx = ctx;
                    events.push(ev);
                }
                // Attribution first, THEN book this line's own content: what a
                // call produces is its output, not its input.
                let mut sink: Vec<(&'static str, String)> = Vec::new();
                claude_ctx::apply_assistant_content(comp, &v, &mut tool_names, &mut sink);
                resources.extend(sink.into_iter().map(|(k, n)| (k, n, line_ts)));
            }
            _ => {}
        }
    }

    let inserted = insert_events_keep_max_output(conn, &events)?;
    result.events_inserted += inserted;
    for (sid, comp) in &comps {
        claude_ctx::save_composition(conn, sid, comp)?;
    }
    claude_ctx::record_resources(conn, "claude", &resources)?;

    let new_offset = start + consumed as i64;
    set_file_state(conn, &path_str, FileState { size, mtime, byte_offset: new_offset })?;
    Ok(())
}

fn parse_line_event(v: &serde_json::Value, source_file: &str, encoded_dir: &str) -> Option<UsageEvent> {
    if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
        return None;
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
        return None;
    }

    let id = match msg["id"].as_str() {
        Some(s) if !s.is_empty() => s,
        _ => return None,
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
        None => return None,
    };

    let session_id = v
        .get("sessionId")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());

    Some(UsageEvent {
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
        ctx: CtxTokens::default(),
    })
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

    #[test]
    fn attributes_context_categories_across_a_session() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(&dir.path().join("t.db")).unwrap();
        let root = dir.path().join("projects");
        let proj = root.join("x");
        std::fs::create_dir_all(&proj).unwrap();

        // user text (40 bytes → 10 est) → call 1 (billed 1000: input 100 + cw 900)
        // → assistant thinking (80 bytes → 20 est) + tool_use → tool_result
        // → call 2 (billed 2000: input 500 + cache_read 1500)
        let user1 = r#"{"type":"user","sessionId":"s1","timestamp":"2026-07-01T10:00:00.000Z","message":{"role":"user","content":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}"#;
        let call1 = r#"{"type":"assistant","sessionId":"s1","requestId":"r1","timestamp":"2026-07-01T10:00:01.000Z","cwd":"/p/x","message":{"id":"m1","model":"claude-opus-4-8","usage":{"input_tokens":100,"output_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":900}}}"#;
        let think = r#"{"type":"assistant","sessionId":"s1","requestId":"r1","timestamp":"2026-07-01T10:00:02.000Z","cwd":"/p/x","message":{"id":"m1","model":"claude-opus-4-8","content":[{"type":"thinking","thinking":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}],"usage":{"input_tokens":100,"output_tokens":30,"cache_read_input_tokens":0,"cache_creation_input_tokens":900}}}"#;
        let tooluse = r#"{"type":"assistant","sessionId":"s1","requestId":"r1","timestamp":"2026-07-01T10:00:03.000Z","cwd":"/p/x","message":{"id":"m1","model":"claude-opus-4-8","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls -la"}}],"usage":{"input_tokens":100,"output_tokens":40,"cache_read_input_tokens":0,"cache_creation_input_tokens":900}}}"#;
        let toolres = r#"{"type":"user","sessionId":"s1","timestamp":"2026-07-01T10:00:04.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"cccccccccccccccccccccccccccccccccccccccc"}]}}"#;
        let call2 = r#"{"type":"assistant","sessionId":"s1","requestId":"r2","timestamp":"2026-07-01T10:00:05.000Z","cwd":"/p/x","message":{"id":"m2","model":"claude-opus-4-8","usage":{"input_tokens":500,"output_tokens":10,"cache_read_input_tokens":1500,"cache_creation_input_tokens":0}}}"#;
        let lines = [user1, call1, think, tooluse, toolres, call2].join("\n") + "\n";
        std::fs::write(proj.join("s1.jsonl"), lines).unwrap();

        let res = scan_claude(&mut conn, &root);
        assert_eq!(res.error, None);
        assert_eq!(res.events_inserted, 2);

        // Call 1: composition = msg 10 (user text only) → sys initialized to 990.
        // Partition: total=1000, sys=990, reas=0 → system=990, reasoning=0, messages=10.
        let (m1, s1, r1, t1): (i64, i64, i64, i64) = conn.query_row(
            "SELECT ctx_messages, ctx_system, ctx_reasoning, ctx_toolcalls FROM events WHERE dedup_key='claude:m1:r1'",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
        assert_eq!(s1, 990);
        assert_eq!(r1, 0);
        assert_eq!(m1, 10);
        assert_eq!(t1, 0);
        assert_eq!(m1 + s1 + r1, 1000, "partition exact");

        // Call 2 composition: msg 10 + tool_use input est + tool_result est(10),
        // reas 20 (in-turn thinking persists across tool_result), sys 990.
        // Just assert the invariants — exact split depends on JSON byte lengths.
        let (m2, s2, r2, t2): (i64, i64, i64, i64) = conn.query_row(
            "SELECT ctx_messages, ctx_system, ctx_reasoning, ctx_toolcalls FROM events WHERE dedup_key='claude:m2:r2'",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
        assert_eq!(m2 + s2 + r2, 2000, "partition exact");
        assert!(r2 > 0, "thinking counted within its turn");
        assert!(t2 > 0 && t2 <= m2, "toolcalls subset of messages");
        assert!(s2 > 0 && s2 < 2000);
    }

    #[test]
    fn sidechain_and_subagent_files_attribute_agents() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(&dir.path().join("t.db")).unwrap();
        let root = dir.path().join("projects");
        let agent_dir = root.join("x/sess-a/subagents");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let user = r#"{"type":"user","sessionId":"ag1","timestamp":"2026-07-01T09:00:00.000Z","message":{"role":"user","content":"task prompt here"}}"#;
        let call = r#"{"type":"assistant","sessionId":"ag1","requestId":"ra","timestamp":"2026-07-01T09:00:01.000Z","cwd":"/p/x","message":{"id":"ma","model":"claude-opus-4-8","usage":{"input_tokens":400,"output_tokens":5,"cache_read_input_tokens":600,"cache_creation_input_tokens":0}}}"#;
        std::fs::write(agent_dir.join("agent-1.jsonl"), format!("{user}\n{call}\n")).unwrap();

        let res = scan_claude(&mut conn, &root);
        assert_eq!(res.events_inserted, 1);
        let (agents, msgs): (i64, i64) = conn.query_row(
            "SELECT ctx_agents, ctx_messages FROM events WHERE dedup_key='claude:ma:ra'",
            [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(agents, 1000, "whole billed context attributed to agents");
        assert!(msgs > 0, "primary partition still computed for agent sessions");
    }

    #[test]
    fn resume_with_lost_state_taints_session_to_null() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(&dir.path().join("t.db")).unwrap();
        let root = dir.path().join("projects");
        let proj = root.join("x");
        std::fs::create_dir_all(&proj).unwrap();
        let logp = proj.join("s2.jsonl");
        let user = r#"{"type":"user","sessionId":"s2","timestamp":"2026-07-01T10:00:00.000Z","message":{"role":"user","content":"hello there friend"}}"#;
        let call1 = r#"{"type":"assistant","sessionId":"s2","requestId":"r1","timestamp":"2026-07-01T10:00:01.000Z","cwd":"/p/x","message":{"id":"m1","model":"claude-opus-4-8","usage":{"input_tokens":100,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        std::fs::write(&logp, format!("{user}\n{call1}\n")).unwrap();
        scan_claude(&mut conn, &root);

        // Simulate lost state (e.g. cleared out-of-band) between scans.
        conn.execute("DELETE FROM session_ctx", []).unwrap();

        let call2 = r#"{"type":"assistant","sessionId":"s2","requestId":"r2","timestamp":"2026-07-01T10:05:00.000Z","cwd":"/p/x","message":{"id":"m2","model":"claude-opus-4-8","usage":{"input_tokens":200,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(&logp).unwrap();
            writeln!(f, "{call2}").unwrap();
        }
        scan_claude(&mut conn, &root);
        let cm: Option<i64> = conn.query_row(
            "SELECT ctx_messages FROM events WHERE dedup_key='claude:m2:r2'",
            [], |r| r.get(0)).unwrap();
        assert_eq!(cm, None, "resumed without state: NULL, never a guess");
    }

    #[test]
    fn full_reparse_heals_tainted_session() {
        // A session tainted by a lost-state resume must recover when its file is
        // re-parsed from byte 0 (the v3 "clear scanned_files" backfill gesture):
        // a full parse rebuilds the composition from scratch and must ignore the
        // persisted tainted row.
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(&dir.path().join("t.db")).unwrap();
        let root = dir.path().join("projects");
        let proj = root.join("x");
        std::fs::create_dir_all(&proj).unwrap();
        let logp = proj.join("s9.jsonl");
        let user = r#"{"type":"user","sessionId":"s9","timestamp":"2026-07-01T10:00:00.000Z","message":{"role":"user","content":"hello there friend"}}"#;
        let call1 = r#"{"type":"assistant","sessionId":"s9","requestId":"r1","timestamp":"2026-07-01T10:00:01.000Z","cwd":"/p/x","message":{"id":"m1","model":"claude-opus-4-8","usage":{"input_tokens":100,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        std::fs::write(&logp, format!("{user}\n{call1}\n")).unwrap();
        scan_claude(&mut conn, &root);

        // Lose the composition between scans, then append: the resume taints s9.
        conn.execute("DELETE FROM session_ctx", []).unwrap();
        let call2 = r#"{"type":"assistant","sessionId":"s9","requestId":"r2","timestamp":"2026-07-01T10:05:00.000Z","cwd":"/p/x","message":{"id":"m2","model":"claude-opus-4-8","usage":{"input_tokens":200,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(&logp).unwrap();
            writeln!(f, "{call2}").unwrap();
        }
        scan_claude(&mut conn, &root);
        let cm: Option<i64> = conn
            .query_row("SELECT ctx_messages FROM events WHERE dedup_key='claude:m2:r2'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cm, None, "precondition: session tainted after lost-state resume");

        // Repair gesture: force a full re-parse (what the v3 migration backfill does).
        conn.execute("DELETE FROM scanned_files", []).unwrap();
        scan_claude(&mut conn, &root);

        // The tie-backfill fills the previously-NULL ctx columns from the healed scan.
        let (cm2, cs2): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT ctx_messages, ctx_system FROM events WHERE dedup_key='claude:m2:r2'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(cm2.is_some(), "full re-parse must heal the tainted session");
        assert_eq!(cm2.unwrap() + cs2.unwrap_or(0), 200, "partition holds after heal (reasoning 0 here)");
        // And the persisted composition is no longer tainted.
        let tainted: i64 = conn
            .query_row("SELECT tainted FROM session_ctx WHERE session_id='s9'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tainted, 0);
    }

    #[test]
    fn compact_boundary_resets_content_counters() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(&dir.path().join("t.db")).unwrap();
        let root = dir.path().join("projects");
        let proj = root.join("x");
        std::fs::create_dir_all(&proj).unwrap();
        let user1 = r#"{"type":"user","sessionId":"s3","timestamp":"2026-07-01T10:00:00.000Z","message":{"role":"user","content":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}"#;
        let call1 = r#"{"type":"assistant","sessionId":"s3","requestId":"r1","timestamp":"2026-07-01T10:00:01.000Z","cwd":"/p/x","message":{"id":"m1","model":"claude-opus-4-8","usage":{"input_tokens":100,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":900}}}"#;
        let compact = r#"{"type":"system","subtype":"compact_boundary","sessionId":"s3","timestamp":"2026-07-01T11:00:00.000Z"}"#;
        let user2 = r#"{"type":"user","sessionId":"s3","timestamp":"2026-07-01T11:00:01.000Z","message":{"role":"user","content":"bbbb"}}"#;
        let call2 = r#"{"type":"assistant","sessionId":"s3","requestId":"r2","timestamp":"2026-07-01T11:00:02.000Z","cwd":"/p/x","message":{"id":"m2","model":"claude-opus-4-8","usage":{"input_tokens":100,"output_tokens":1,"cache_read_input_tokens":900,"cache_creation_input_tokens":0}}}"#;
        std::fs::write(proj.join("s3.jsonl"), [user1, call1, compact, user2, call2].join("\n") + "\n").unwrap();

        scan_claude(&mut conn, &root);
        // After compaction: composition = msg 1 (4 bytes user2), sys 990 → of 1000
        // billed, system ≈ 990/991·1000, messages the remainder.
        let (m2, s2): (i64, i64) = conn.query_row(
            "SELECT ctx_messages, ctx_system FROM events WHERE dedup_key='claude:m2:r2'",
            [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert!(s2 > 900, "system baseline survives compaction");
        assert!(m2 < 100, "pre-compaction messages no longer in the window");
    }

    #[test]
    fn records_skill_and_mcp_resources() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(&dir.path().join("t.db")).unwrap();
        let root = dir.path().join("projects");
        let proj = root.join("x");
        std::fs::create_dir_all(&proj).unwrap();
        let line = r#"{"type":"assistant","sessionId":"s4","requestId":"r1","timestamp":"2026-07-01T10:00:00.000Z","cwd":"/p/x","message":{"id":"m1","model":"claude-opus-4-8","content":[{"type":"tool_use","id":"t1","name":"Skill","input":{"skill":"graphify"}},{"type":"tool_use","id":"t2","name":"mcp__pencil__batch_get","input":{}}],"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        std::fs::write(proj.join("s4.jsonl"), format!("{line}\n")).unwrap();
        scan_claude(&mut conn, &root);
        let rows: Vec<(String, String)> = {
            let mut stmt = conn.prepare("SELECT kind, name FROM ctx_resources WHERE source='claude' ORDER BY kind").unwrap();
            let it = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
            it.collect::<rusqlite::Result<Vec<_>>>().unwrap()
        };
        assert_eq!(rows, vec![
            ("mcp_server".to_string(), "pencil".to_string()),
            ("skill".to_string(), "graphify".to_string()),
        ]);
    }
}
