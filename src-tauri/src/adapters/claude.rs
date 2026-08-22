use super::claude_ctx::{self, Composition};
use super::exec_class;
use super::{find_jsonl, rollup_worktree};
use crate::db::{get_file_state, insert_events_keep_max_output, set_file_state};
use crate::types::{CtxTokens, FileState, SourceScanResult, UsageEvent};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::Path;

pub fn scan_claude(conn: &mut Connection, projects_root: &Path) -> SourceScanResult {
    let mut result = SourceScanResult::default();
    let mut files = Vec::new();
    find_jsonl(projects_root, &mut files);
    files.sort();
    // Plain-key Records that per-iteration Records replace, collected across the
    // whole Source and cleared once at the end. Not per file: a fork copies a
    // turn's lines into a second transcript, and whether that copy is scanned
    // before or after the original must not decide whether the stale Record
    // survives. One statement over a handful of keys.
    //
    // Within one file, line order cannot matter: parse_file retains out the
    // plain-key Records for every superseded turn in the file, whether the
    // plain-form lines come before the array line or after it. Across files in
    // one scan, the clear below runs after every insert. So the only exposure
    // is ACROSS scans.
    //
    // ponytail: only lines parsed in THIS scan report a superseded key, so a
    // fork whose plain-form copy of a fallback turn first appears on a LATER
    // scan re-inserts the plain key with nothing left to clear it, and the turn
    // double-books. It cannot fire on today's Artifact — 2,173 turns already
    // span more than one file, none of them a multi-call turn, and a fork copies
    // its source's lines verbatim, so no cross-file copy disagrees about
    // `iterations`. The honest fix is a query for plain keys that have `#it`
    // siblings, which is a correlated GLOB per row over ~100k Claude Records.
    // Do that when a copy is observed disagreeing, not before.
    let mut superseded: Vec<String> = Vec::new();
    for path in files {
        if let Err(e) = scan_file(conn, &path, &mut result, &mut superseded) {
            result.error = Some(e.to_string());
            return result;
        }
    }
    if !superseded.is_empty() {
        // Exact keys, not patterns: a plain key GLOB-matches only itself, so the
        // `#it{i}` Records that replaced it are untouched.
        if let Err(e) = crate::db::insert_events_superseding(conn, &superseded, &[]) {
            result.error = Some(e.to_string());
        }
    }
    result
}

struct ParsedClaudeFile {
    events: Vec<UsageEvent>,
    superseded: Vec<String>,
    comps: HashMap<String, Composition>,
    resources: Vec<(&'static str, String, i64)>,
    tool_rows: Vec<(String, i64, i64, i64)>,
    skill_rows: Vec<(String, i64, i64, i64)>,
    exec_rows: Vec<(String, String, String, i64, i64, i64)>,
    consumed: usize,
    lines_skipped: u64,
}

// Pure parse core (no Connection): all attribution logic lives here. `prior`
// supplies the persisted composition on a mid-file resume (start > 0),
// consulted only on first sight of a session id; a None taints that session so
// attribution stays NULL instead of guessing.
fn parse_file(
    buf: &[u8],
    start: i64,
    path_str: &str,
    encoded_dir: &str,
    file_stem: &str,
    mut prior: impl FnMut(&str) -> Option<Composition>,
) -> ParsedClaudeFile {
    // Consume only complete newline-terminated lines; a trailing partial line
    // is left for the next scan.
    let consumed = buf
        .iter()
        .rposition(|&b| b == b'\n')
        .map(|i| i + 1)
        .unwrap_or(0);

    // Context attribution (spec 2026-07-10): feed every line through the
    // running composition. Snapshot the composition at first sight of a
    // dedup_key (content the call produces is its output, not its input),
    // but split per LINE: proxied models (chatcmpl ids) log partial usage on
    // early duplicate lines, and the dedup upsert keeps the max-output line —
    // its ctx must be computed from its own billed or the partition breaks.
    // By component, not by "/subagents/": this is a path on the machine doing
    // the scanning, and on Windows its separator is a backslash — a literal
    // match there finds nothing and every subagent file reads as a main one.
    let is_agent_file = Path::new(path_str)
        .components()
        .any(|c| c.as_os_str() == "subagents");
    let mut events = Vec::new();
    let mut superseded: Vec<String> = Vec::new();
    let mut comps: HashMap<String, Composition> = HashMap::new();
    let mut tool_names: HashMap<String, String> = HashMap::new();
    let mut resources: Vec<(&'static str, String, i64)> = Vec::new();
    let mut comp_by_key: HashMap<String, Composition> = HashMap::new();
    let mut tool_rows: Vec<(String, i64, i64, i64)> = Vec::new();
    let mut skill_rows: Vec<(String, i64, i64, i64)> = Vec::new();
    let mut exec_by_id: HashMap<String, (String, String, String)> = HashMap::new();
    let mut exec_rows: Vec<(String, String, String, i64, i64, i64)> = Vec::new();
    let mut lines_skipped: u64 = 0;

    for line in buf[..consumed].split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_slice(line) {
            Ok(v) => v,
            Err(_) => {
                lines_skipped += 1;
                continue;
            }
        };
        // Session key: per-line sessionId, else the file stem (one session per file).
        let sid = v
            .get("sessionId")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| file_stem.to_string());
        if !comps.contains_key(&sid) {
            // The persisted composition exists only to survive byte-offset resumes.
            // A full parse from byte 0 rebuilds it from scratch — loading here would
            // double-count content and make a stale taint permanent.
            let c = if start > 0 {
                // Mid-file resume with no persisted state: composition is unknowable —
                // taint the session so attribution stays NULL instead of guessing.
                match prior(&sid) {
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
            Some("user") => {
                let mut sizes: Vec<(String, i64, i64)> = Vec::new();
                let mut skills: Vec<(String, i64, i64)> = Vec::new();
                claude_ctx::apply_user_line(comp, &v, &tool_names, &mut sizes, &mut skills);
                tool_rows.extend(sizes.into_iter().map(|(n, e, c)| (n, e, c, line_ts)));
                skill_rows.extend(skills.into_iter().map(|(n, e, u)| (n, e, u, line_ts)));
                collect_exec(&v, &mut exec_by_id, &mut exec_rows, line_ts);
            }
            Some("system") => {
                if v.get("subtype").and_then(|s| s.as_str()) == Some("compact_boundary") {
                    comp.reset_compact();
                }
            }
            Some("assistant") => {
                // One line can book several Records: a model fallback logs each
                // API call in usage.iterations (TOKL-26). ctx is attributed per
                // Record against its OWN billed context, because every one of
                // those calls sent a window and was billed for it.
                let (line_events, replaced) = parse_line_events(&v, path_str, encoded_dir);
                if let Some(key) = replaced {
                    superseded.push(key);
                }
                for mut ev in line_events {
                    let billed = ev.input_tokens
                        + ev.cache_read_tokens
                        + ev.cache_write_5m_tokens
                        + ev.cache_write_1h_tokens;
                    // Idempotent after the first call, so the session's system
                    // estimate comes from its first API call — which, on a
                    // multi-iteration line, is iteration 0 rather than the
                    // top-level rollup of a later one.
                    comp.init_system(billed);
                    let snap = *comp_by_key.entry(ev.dedup_key.clone()).or_insert(*comp);
                    let mut ctx = snap.attribute(billed);
                    let sidechain = is_agent_file
                        || v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true);
                    if sidechain {
                        ctx.agents = Some(billed);
                    }
                    // Reasoning share: genuine Anthropic transcripts store thinking
                    // signature-only (empty text) so the share is 0 — unobservable, NULL.
                    // Proxied third-party models (e.g. GLM) DO log thinking text; a
                    // nonzero share is a real observation and must stay, or the primary
                    // partition loses exactly that amount.
                    if ctx.reasoning == Some(0) {
                        ctx.reasoning = None;
                    }
                    ev.ctx = ctx;
                    events.push(ev);
                }
                // Attribution first, THEN book this line's own content: what a
                // call produces is its output, not its input.
                let mut sink: Vec<(&'static str, String)> = Vec::new();
                let mut sizes: Vec<(String, i64, i64)> = Vec::new();
                claude_ctx::apply_assistant_content(comp, &v, &mut tool_names, &mut sink, &mut sizes);
                resources.extend(sink.into_iter().map(|(k, n)| (k, n, line_ts)));
                tool_rows.extend(sizes.into_iter().map(|(n, e, c)| (n, e, c, line_ts)));
                collect_exec(&v, &mut exec_by_id, &mut exec_rows, line_ts);
            }
            _ => {}
        }
    }

    // Lines of one turn can disagree about `iterations`: three lines of a real
    // subagent transcript carry no array at all and the fourth carries two. The
    // per-iteration Records ARE that turn, so the plain-key Record its earlier
    // lines produced must not be booked beside them, or the turn counts twice.
    if !superseded.is_empty() {
        events.retain(|e| !superseded.contains(&e.dedup_key));
    }

    ParsedClaudeFile { events, superseded, comps, resources, tool_rows, skill_rows, exec_rows, consumed, lines_skipped }
}

fn scan_file(
    conn: &mut Connection,
    path: &Path,
    result: &mut SourceScanResult,
    superseded: &mut Vec<String>,
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
    // reparse from the start (idempotent via dedup keys). A file whose size
    // AND mtime both match the stored state is skipped outright — with ~900
    // transcripts on disk this is the difference between a scan that touches
    // every file every 30s and one that only reads what actually changed.
    let prev = get_file_state(conn, &path_str)?;
    if let Some(ref fs) = prev {
        if fs.size == size && fs.mtime == mtime {
            return Ok(());
        }
    }
    let start = match prev {
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

    // ctx_tools idempotency: a parse from byte 0 rebuilds this file's tool
    // weights from scratch (same rule as the fresh composition).
    if start == 0 {
        crate::db::clear_ctx_tools_for_file(conn, &path_str)?;
        crate::db::clear_ctx_skills_for_file(conn, &path_str)?;
        crate::db::clear_ctx_exec_for_file(conn, &path_str)?;
    }

    let file_stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    // prior lookup for mid-file resumes: a DB read error maps to None (taint),
    // the same NULL-not-a-guess outcome as a missing row.
    let parsed = parse_file(&buf, start, &path_str, &encoded_dir, &file_stem, |sid| {
        crate::db::load_composition(conn, sid).ok().flatten()
    });

    result.lines_skipped += parsed.lines_skipped;
    superseded.extend(parsed.superseded.iter().cloned());
    let inserted = insert_events_keep_max_output(conn, &parsed.events)?;
    result.events_inserted += inserted;
    for (sid, comp) in &parsed.comps {
        crate::db::save_composition(conn, sid, comp)?;
    }
    crate::db::record_resources(conn, "claude", &parsed.resources)?;
    crate::db::add_ctx_tool_rows(conn, "claude", &path_str, &parsed.tool_rows)?;
    crate::db::add_ctx_skill_rows(conn, "claude", &path_str, &parsed.skill_rows)?;
    crate::db::add_ctx_exec_rows(conn, "claude", &path_str, &parsed.exec_rows)?;

    let new_offset = start + parsed.consumed as i64;
    set_file_state(conn, &path_str, FileState { size, mtime, byte_offset: new_offset })?;
    Ok(())
}

/// The Usage Records one assistant line books, and the dedup_key they replace.
///
/// Usually one Record, keyed exactly as it always was. When `usage.iterations`
/// reports two or more API calls it is one Record PER CALL, each under its own
/// Model and `#it{i}`-suffixed key — see `claude_shaped_records` for what that
/// field is and why one row cannot hold two Models.
///
/// The plain key those messages used to book is returned as superseded. A
/// keep-max upsert cannot correct it: the surviving Record's output_tokens are
/// unchanged, and a tie keeps the stored row.
fn parse_line_events(
    v: &serde_json::Value,
    source_file: &str,
    encoded_dir: &str,
) -> (Vec<UsageEvent>, Option<String>) {
    let none = (Vec::new(), None);
    if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
        return none;
    }
    let msg = &v["message"];

    let id = match msg["id"].as_str() {
        Some(s) if !s.is_empty() => s,
        _ => return none,
    };
    let base_key = match v.get("requestId").and_then(|r| r.as_str()) {
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
        None => return none,
    };

    let session_id = v
        .get("sessionId")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());

    let event = |dedup_key: String, model: String, u: &super::ClaudeShapedUsage| UsageEvent {
        dedup_key,
        source: "claude".to_string(),
        timestamp,
        model: Some(model),
        project: project.clone(),
        // One call per Record, so Requests still sums api_calls (CONTEXT.md is
        // explicit that it is never a Ledger row count) — what changed is that
        // a fallback message now contributes a Record per call instead of one.
        api_calls: 1,
        input_tokens: u.input,
        output_tokens: u.output,
        cache_read_tokens: u.cache_read,
        cache_write_5m_tokens: u.cache_write_5m,
        cache_write_1h_tokens: u.cache_write_1h,
        source_file: source_file.to_string(),
        session_id: session_id.clone(),
        reasoning_tokens: None,
        ctx: CtxTokens::default(),
    };

    match super::claude_shaped_records(msg) {
        // <synthetic> error placeholders have all-zero usage: skip, don't count.
        super::ClaudeRecords::NoRecord => none,
        super::ClaudeRecords::OneRecord(usage) => (vec![event(base_key, model, &usage)], None),
        super::ClaudeRecords::RecordPerCall(calls) => (
            calls
                .iter()
                .map(|c| {
                    let m = c.model.clone().unwrap_or_else(|| model.clone());
                    event(format!("{base_key}#it{}", c.index), m, &c.usage)
                })
                .collect(),
            Some(base_key),
        ),
    }
}

// Bash command-level facets (spec 2026-07-10-bash-exec-drilldown): classify
// each Bash tool_use once, remember the classification by tool_use id so the
// paired result's bytes book to the same command. Reads the line only —
// independent of the attribution engine.
fn collect_exec(
    v: &serde_json::Value,
    exec_by_id: &mut HashMap<String, (String, String, String)>,
    exec_rows: &mut Vec<(String, String, String, i64, i64, i64)>,
    line_ts: i64,
) {
    let blocks = match v["message"]["content"].as_array() {
        Some(b) => b,
        None => return,
    };
    for b in blocks {
        match b.get("type").and_then(|t| t.as_str()) {
            Some("tool_use") if b.get("name").and_then(|n| n.as_str()) == Some("Bash") => {
                if let Some(cmd) = b.pointer("/input/command").and_then(|c| c.as_str()) {
                    let kind = exec_class::exec_kind(cmd).to_string();
                    let exe = exec_class::exec_exe(cmd);
                    let sig = exec_class::exec_cmd(cmd);
                    if let Some(id) = b.get("id").and_then(|i| i.as_str()) {
                        exec_by_id
                            .insert(id.to_string(), (kind.clone(), exe.clone(), sig.clone()));
                    }
                    let est = claude_ctx::est(claude_ctx::content_bytes(&b["input"]));
                    exec_rows.push((kind, exe, sig, est, 1, line_ts));
                }
            }
            Some("tool_result") => {
                let hit = b
                    .get("tool_use_id")
                    .and_then(|i| i.as_str())
                    .and_then(|id| exec_by_id.get(id))
                    .cloned();
                if let Some((kind, exe, sig)) = hit {
                    let est = claude_ctx::est(claude_ctx::content_bytes(&b["content"]));
                    exec_rows.push((kind, exe, sig, est, 0, line_ts));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_db;
    use std::io::Write;
    use std::path::PathBuf;

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
        // → assistant thinking (signature-only, empty text — like real logs)
        // + tool_use → tool_result → call 2 (billed 2000: input 500 + cache_read 1500)
        let user1 = r#"{"type":"user","sessionId":"s1","timestamp":"2026-07-01T10:00:00.000Z","message":{"role":"user","content":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}"#;
        let call1 = r#"{"type":"assistant","sessionId":"s1","requestId":"r1","timestamp":"2026-07-01T10:00:01.000Z","cwd":"/p/x","message":{"id":"m1","model":"claude-opus-4-8","usage":{"input_tokens":100,"output_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":900}}}"#;
        let think = r#"{"type":"assistant","sessionId":"s1","requestId":"r1","timestamp":"2026-07-01T10:00:02.000Z","cwd":"/p/x","message":{"id":"m1","model":"claude-opus-4-8","content":[{"type":"thinking","thinking":"","signature":"sig1"}],"usage":{"input_tokens":100,"output_tokens":30,"cache_read_input_tokens":0,"cache_creation_input_tokens":900}}}"#;
        let tooluse = r#"{"type":"assistant","sessionId":"s1","requestId":"r1","timestamp":"2026-07-01T10:00:03.000Z","cwd":"/p/x","message":{"id":"m1","model":"claude-opus-4-8","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls -la"}}],"usage":{"input_tokens":100,"output_tokens":40,"cache_read_input_tokens":0,"cache_creation_input_tokens":900}}}"#;
        let toolres = r#"{"type":"user","sessionId":"s1","timestamp":"2026-07-01T10:00:04.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"cccccccccccccccccccccccccccccccccccccccc"}]}}"#;
        let call2 = r#"{"type":"assistant","sessionId":"s1","requestId":"r2","timestamp":"2026-07-01T10:00:05.000Z","cwd":"/p/x","message":{"id":"m2","model":"claude-opus-4-8","usage":{"input_tokens":500,"output_tokens":10,"cache_read_input_tokens":1500,"cache_creation_input_tokens":0}}}"#;
        let lines = [user1, call1, think, tooluse, toolres, call2].join("\n") + "\n";
        std::fs::write(proj.join("s1.jsonl"), lines).unwrap();

        let res = scan_claude(&mut conn, &root);
        assert_eq!(res.error, None);
        assert_eq!(res.events_inserted, 2);

        // Call 1: composition = msg 10 (user text only) → sys initialized to 990.
        // Partition: total=1000, sys=990 → system=990, messages=10. Reasoning is
        // NULL: real transcripts carry signature-only thinking blocks (text always
        // empty), so reasoning-in-context is unobservable for Claude.
        let (m1, s1, r1, t1): (i64, i64, Option<i64>, i64) = conn.query_row(
            "SELECT ctx_messages, ctx_system, ctx_reasoning, ctx_toolcalls FROM events WHERE dedup_key='claude:m1:r1'",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
        assert_eq!(s1, 990);
        assert_eq!(r1, None, "reasoning unobservable in Claude logs → NULL");
        assert_eq!(m1, 10);
        assert_eq!(t1, 0);
        assert_eq!(m1 + s1, 1000, "partition exact (reasoning excluded: NULL)");

        // Call 2 composition: msg 10 + tool_use input est + tool_result est(10),
        // sys 990; the thinking block contributes nothing (empty text, like real
        // logs). Just assert the invariants — exact split depends on JSON byte
        // lengths.
        let (m2, s2, r2, t2): (i64, i64, Option<i64>, i64) = conn.query_row(
            "SELECT ctx_messages, ctx_system, ctx_reasoning, ctx_toolcalls FROM events WHERE dedup_key='claude:m2:r2'",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
        assert_eq!(r2, None, "reasoning unobservable in Claude logs → NULL");
        assert_eq!(m2 + s2, 2000, "partition exact (reasoning excluded: NULL)");
        assert!(t2 > 0 && t2 <= m2, "toolcalls subset of messages");
        assert!(s2 > 0 && s2 < 2000);
    }

    #[test]
    fn duplicate_lines_with_growing_usage_attribute_from_own_billed() {
        // Proxied models (chatcmpl ids) log PARTIAL usage on early duplicate
        // lines of the same message; the dedup upsert keeps the max-output
        // line. Its ctx must be split from its own billed, not the first
        // line's — or the primary partition falls short by the difference.
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(&dir.path().join("t.db")).unwrap();
        let root = dir.path().join("projects");
        let proj = root.join("x");
        std::fs::create_dir_all(&proj).unwrap();

        let user1 = r#"{"type":"user","sessionId":"sd","timestamp":"2026-07-01T10:00:00.000Z","message":{"role":"user","content":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}"#;
        // First duplicate: partial usage (billed 100). Second: final usage
        // (billed 2000, higher output → wins the upsert).
        let l1 = r#"{"type":"assistant","sessionId":"sd","timestamp":"2026-07-01T10:00:01.000Z","cwd":"/p/x","message":{"id":"chatcmpl-dup","model":"z-ai/glm-5.2","content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":100,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        let l2 = r#"{"type":"assistant","sessionId":"sd","timestamp":"2026-07-01T10:00:02.000Z","cwd":"/p/x","message":{"id":"chatcmpl-dup","model":"z-ai/glm-5.2","content":[{"type":"text","text":"hi there"}],"usage":{"input_tokens":500,"output_tokens":25,"cache_read_input_tokens":1500,"cache_creation_input_tokens":0}}}"#;
        let lines = [user1, l1, l2].join("\n") + "\n";
        std::fs::write(proj.join("sd.jsonl"), lines).unwrap();

        let res = scan_claude(&mut conn, &root);
        assert_eq!(res.error, None);
        assert_eq!(res.events_inserted, 1, "duplicates collapse to one event");

        let (out, m, s, r): (i64, i64, i64, Option<i64>) = conn.query_row(
            "SELECT output_tokens, ctx_messages, ctx_system, ctx_reasoning FROM events WHERE dedup_key='claude:chatcmpl-dup'",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).unwrap();
        assert_eq!(out, 25, "max-output line wins");
        assert_eq!(
            m + s + r.unwrap_or(0),
            2000,
            "kept row's partition matches its OWN billed, not the first line's"
        );
    }

    #[test]
    fn nonempty_thinking_text_yields_reasoning_share() {
        // Third-party models proxied through Claude Code (e.g. z-ai/glm-5.2) DO
        // log thinking text, unlike genuine Anthropic transcripts. The engine
        // then computes a nonzero reasoning share; it is a real observation and
        // must survive to the stored row, or the primary partition
        // (messages + system + reasoning) falls short of billed by that amount.
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(&dir.path().join("t.db")).unwrap();
        let root = dir.path().join("projects");
        let proj = root.join("x");
        std::fs::create_dir_all(&proj).unwrap();

        // user text (40 bytes → 10 est) → call m1 carrying ~400 chars of real
        // thinking text (est 100) → call m2 (billed 2000). m2's attribution is
        // computed from the composition after m1's thinking is booked, so its
        // reasoning share is nonzero.
        let think_text = "Reasoning through the request carefully to reach a correct answer. ".repeat(6);
        let user1 = r#"{"type":"user","sessionId":"sg","timestamp":"2026-07-01T10:00:00.000Z","message":{"role":"user","content":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}"#;
        let m1 = r#"{"type":"assistant","sessionId":"sg","requestId":"r1","timestamp":"2026-07-01T10:00:01.000Z","cwd":"/p/x","message":{"id":"m1","model":"z-ai/glm-5.2","content":[{"type":"thinking","thinking":"THINK","signature":"s"}],"usage":{"input_tokens":100,"output_tokens":30,"cache_read_input_tokens":0,"cache_creation_input_tokens":900}}}"#.replace("THINK", &think_text);
        let m2 = r#"{"type":"assistant","sessionId":"sg","requestId":"r2","timestamp":"2026-07-01T10:00:05.000Z","cwd":"/p/x","message":{"id":"m2","model":"z-ai/glm-5.2","usage":{"input_tokens":500,"output_tokens":10,"cache_read_input_tokens":1500,"cache_creation_input_tokens":0}}}"#;
        let lines = format!("{user1}\n{m1}\n{m2}\n");
        std::fs::write(proj.join("sg.jsonl"), lines).unwrap();

        let res = scan_claude(&mut conn, &root);
        assert_eq!(res.error, None);
        assert_eq!(res.events_inserted, 2);

        // billed m2 = input 500 + cache_read 1500 = 2000.
        let (m, s, r): (i64, i64, Option<i64>) = conn.query_row(
            "SELECT ctx_messages, ctx_system, ctx_reasoning FROM events WHERE dedup_key='claude:m2:r2'",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
        let r = r.expect("proxied thinking text is a real observation → Some, not NULL");
        assert!(r > 0, "nonzero reasoning share must be kept, not discarded");
        assert_eq!(m + s + r, 2000, "primary partition exact, incl. reasoning share");
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

    #[test]
    fn scan_populates_ctx_tools_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(&dir.path().join("t.db")).unwrap();
        let root = dir.path().join("projects");
        let proj = root.join("x");
        std::fs::create_dir_all(&proj).unwrap();
        let logp = proj.join("s7.jsonl");
        let tooluse = r#"{"type":"assistant","sessionId":"s7","requestId":"r1","timestamp":"2026-07-01T10:00:00.000Z","cwd":"/p/x","message":{"id":"m1","model":"claude-opus-4-8","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls -la"}}],"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        let toolres = r#"{"type":"user","sessionId":"s7","timestamp":"2026-07-01T10:00:01.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"cccccccccccccccccccccccccccccccccccccccc"}]}}"#;
        std::fs::write(&logp, format!("{tooluse}\n{toolres}\n")).unwrap();

        scan_claude(&mut conn, &root);
        let (est1, calls1): (i64, i64) = conn.query_row(
            "SELECT est_tokens, calls FROM ctx_tools WHERE source='claude' AND name='Bash'",
            [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert!(est1 > 0);
        assert_eq!(calls1, 1, "tool_use counts one call; its result adds size only");

        // Unchanged re-scan (resume at EOF): no new bytes, no double counting.
        scan_claude(&mut conn, &root);
        let (est2, calls2): (i64, i64) = conn.query_row(
            "SELECT est_tokens, calls FROM ctx_tools WHERE source='claude' AND name='Bash'",
            [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((est2, calls2), (est1, calls1));

        // Forced full re-parse (heal gesture): rows replaced, not doubled.
        conn.execute("DELETE FROM scanned_files", []).unwrap();
        scan_claude(&mut conn, &root);
        let (est3, calls3): (i64, i64) = conn.query_row(
            "SELECT est_tokens, calls FROM ctx_tools WHERE source='claude' AND name='Bash'",
            [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((est3, calls3), (est1, calls1), "full re-parse replaces the file's rows");
    }

    // Why supersession is hoisted out of scan_file to the end of the Source: a
    // fork copies a turn's lines into a second transcript, and the plain-form
    // copy can sort AFTER the one carrying `iterations`. Deleting per file would
    // then delete the stale Record and let the later file re-insert it. The
    // Record must be gone whichever order the files are read in.
    #[test]
    fn a_later_file_cannot_resurrect_a_superseded_record() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(&dir.path().join("t.db")).unwrap();
        let root = dir.path().join("projects");
        let proj = root.join("x");
        std::fs::create_dir_all(&proj).unwrap();

        // Same turn (id + requestId) in two files. "a" carries the iterations,
        // "b" is the plain-form fork copy and is scanned second.
        let with_iters = format!(
            r#"{{"type":"assistant","sessionId":"a","requestId":"r","timestamp":"2026-07-01T10:00:00.000Z","cwd":"/p/x","message":{{"id":"m","model":"claude-opus-4-8","usage":{{{TOP_HYBRID},"iterations":[{IT_FABLE},{IT_OPUS}]}}}}}}"#
        );
        let plain = format!(
            r#"{{"type":"assistant","sessionId":"b","requestId":"r","timestamp":"2026-07-01T10:00:00.000Z","cwd":"/p/x","message":{{"id":"m","model":"claude-opus-4-8","usage":{{{TOP_HYBRID}}}}}}}"#
        );
        std::fs::write(proj.join("a.jsonl"), format!("{with_iters}\n")).unwrap();
        std::fs::write(proj.join("b.jsonl"), format!("{plain}\n")).unwrap();

        scan_claude(&mut conn, &root);

        let plain_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM events WHERE dedup_key = 'claude:m:r'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(plain_rows, 0, "the later file resurrected the superseded Record");
        let iteration_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE dedup_key GLOB 'claude:m:r#it[0-9]*'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(iteration_rows, 2, "both calls stay booked");
    }

    #[test]
    fn scan_populates_ctx_exec_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(&dir.path().join("t.db")).unwrap();
        let root = dir.path().join("projects");
        let proj = root.join("x");
        std::fs::create_dir_all(&proj).unwrap();
        let logp = proj.join("s8.jsonl");
        let tooluse = r#"{"type":"assistant","sessionId":"s8","requestId":"r1","timestamp":"2026-07-01T10:00:00.000Z","cwd":"/p/x","message":{"id":"m1","model":"claude-opus-4-8","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"git add ."}}],"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        let toolres = r#"{"type":"user","sessionId":"s8","timestamp":"2026-07-01T10:00:01.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"okokokokokokokokokokokokokokokokokokokok"}]}}"#;
        std::fs::write(&logp, format!("{tooluse}\n{toolres}\n")).unwrap();

        scan_claude(&mut conn, &root);
        let (kind, exe, cmd, est1, calls1): (String, String, String, i64, i64) = conn
            .query_row(
                "SELECT kind, exe, cmd, est_tokens, calls FROM ctx_exec WHERE source='claude'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!((kind.as_str(), exe.as_str(), cmd.as_str()), ("git_local", "git", "git add"));
        assert!(est1 > 0, "command + result bytes booked");
        assert_eq!(calls1, 1, "tool_use counts the call; its result adds size only");

        // Unchanged re-scan: resume at EOF, nothing added.
        scan_claude(&mut conn, &root);
        let (est2, calls2): (i64, i64) = conn
            .query_row("SELECT est_tokens, calls FROM ctx_exec", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((est2, calls2), (est1, calls1));

        // Forced full re-parse: rows replaced, not doubled.
        conn.execute("DELETE FROM scanned_files", []).unwrap();
        scan_claude(&mut conn, &root);
        let (est3, calls3): (i64, i64) = conn
            .query_row("SELECT est_tokens, calls FROM ctx_exec", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((est3, calls3), (est1, calls1));
    }

    // ---- pure parse_file core (no DB) ----

    #[test]
    fn parse_file_resume_without_prior_taints_ctx_to_null() {
        // start > 0 and the prior lookup returns None → the session is tainted,
        // so every event's ctx stays all-NULL (never a guess).
        let call = r#"{"type":"assistant","sessionId":"s","requestId":"r","timestamp":"2026-07-01T10:00:00.000Z","cwd":"/p/x","message":{"id":"m","model":"claude-opus-4-8","usage":{"input_tokens":100,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        let buf = format!("{call}\n");
        let parsed = parse_file(buf.as_bytes(), 10, "/p/x/s.jsonl", "enc", "s", |_| None);
        assert_eq!(parsed.events.len(), 1);
        assert_eq!(parsed.events[0].ctx.messages, None, "tainted resume → NULL");
        assert_eq!(parsed.events[0].ctx.system, None);
    }

    #[test]
    fn parse_file_resume_with_prior_attributes_from_it() {
        // start > 0 and the prior lookup returns a known composition → the
        // event is attributed against it (initialized, so init_system no-ops).
        let call = r#"{"type":"assistant","sessionId":"s","requestId":"r","timestamp":"2026-07-01T10:00:00.000Z","cwd":"/p/x","message":{"id":"m","model":"claude-opus-4-8","usage":{"input_tokens":100,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":900}}}"#;
        let buf = format!("{call}\n");
        let known = Composition { msg: 10, sys: 990, initialized: true, ..Default::default() };
        let parsed = parse_file(buf.as_bytes(), 10, "/p/x/s.jsonl", "enc", "s", |_| Some(known));
        assert_eq!(parsed.events.len(), 1);
        let ctx = parsed.events[0].ctx;
        // billed 1000, total 1000 → system 990, messages 10.
        assert_eq!(ctx.system, Some(990));
        assert_eq!(ctx.messages, Some(10));
        assert_eq!(ctx.messages.unwrap() + ctx.system.unwrap(), 1000);
    }

    #[test]
    fn parse_file_snapshots_composition_at_first_sight_of_dedup_key() {
        // Two assistant lines share (id, requestId) → one dedup_key. The second
        // line's ctx uses the FIRST-sight composition (before l1's 400-byte text
        // was booked) but partitions its OWN billed.
        let user = r#"{"type":"user","sessionId":"s","timestamp":"2026-07-01T10:00:00.000Z","message":{"role":"user","content":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}"#;
        let big = "t".repeat(400);
        let l1 = format!(r#"{{"type":"assistant","sessionId":"s","requestId":"r","timestamp":"2026-07-01T10:00:01.000Z","cwd":"/p/x","message":{{"id":"m","model":"claude-opus-4-8","content":[{{"type":"text","text":"{big}"}}],"usage":{{"input_tokens":100,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":900}}}}}}"#);
        let l2 = r#"{"type":"assistant","sessionId":"s","requestId":"r","timestamp":"2026-07-01T10:00:02.000Z","cwd":"/p/x","message":{"id":"m","model":"claude-opus-4-8","usage":{"input_tokens":500,"output_tokens":9,"cache_read_input_tokens":1500,"cache_creation_input_tokens":0}}}"#;
        let buf = format!("{user}\n{l1}\n{l2}\n");
        let parsed = parse_file(buf.as_bytes(), 0, "/p/x/s.jsonl", "enc", "s", |_| None);
        assert_eq!(parsed.events.len(), 2);
        // l2 is the higher-output line; first-sight snapshot ratio (10 msg / 990
        // sys) × l2 billed 2000 → system 1980, not the live-comp 1800.
        let e2 = parsed.events.iter().find(|e| e.output_tokens == 9).unwrap();
        assert_eq!(e2.ctx.system, Some(1980), "first-sight snapshot ratio, not live comp");
        assert_eq!(e2.ctx.messages, Some(20));
        assert_eq!(e2.ctx.messages.unwrap() + e2.ctx.system.unwrap(), 2000, "l2 partitions its OWN billed");
    }

    #[test]
    fn parse_file_consumed_stops_at_last_newline() {
        // A trailing partial (unterminated) line is left for the next scan:
        // consumed ends at the last newline and the partial line is not parsed.
        let l1 = r#"{"type":"assistant","sessionId":"s","requestId":"r","timestamp":"2026-07-01T10:00:00.000Z","cwd":"/p/x","message":{"id":"m1","model":"claude-opus-4-8","usage":{"input_tokens":10,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        let partial = r#"{"type":"assistant","sessionId":"s","requestId":"r2","timestamp":"2026-07-01"#;
        let buf = format!("{l1}\n{partial}");
        let parsed = parse_file(buf.as_bytes(), 0, "/p/x/s.jsonl", "enc", "s", |_| None);
        assert_eq!(parsed.consumed, l1.len() + 1, "consumed ends at the last newline");
        assert_eq!(parsed.events.len(), 1, "trailing partial line left unparsed");
    }

    // ---- TOKL-26: usage.iterations ----------------------------------------
    //
    // What the field is and why one Record per message was wrong: see
    // `adapters::claude_shaped_records`.
    //
    // Fixture figures are a real production message (msg_011Cd9er…): fable-5
    // falling back to opus-4-8, with the top-level `cache_creation_input_tokens`
    // at 0 while `cache_creation.ephemeral_1h_input_tokens` is the FIRST
    // iteration's 24 — the hybrid that made today's opus Record carry fable's
    // cache-write.
    const TOP_HYBRID: &str = r#""input_tokens":2,"output_tokens":1728,"cache_read_input_tokens":180874,"cache_creation_input_tokens":0,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":24}"#;
    const IT_FABLE: &str = r#"{"input_tokens":2,"output_tokens":46,"cache_read_input_tokens":239869,"cache_creation_input_tokens":24,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":24},"type":"message","model":"claude-fable-5"}"#;
    const IT_OPUS: &str = r#"{"input_tokens":2,"output_tokens":1728,"cache_read_input_tokens":180874,"cache_creation_input_tokens":0,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":0},"type":"fallback_message","model":"claude-opus-4-8"}"#;

    fn assistant_line(usage_body: &str) -> String {
        format!(
            r#"{{"type":"assistant","sessionId":"s","requestId":"r","timestamp":"2026-07-01T10:00:00.000Z","cwd":"/p/x","message":{{"id":"m","model":"claude-opus-4-8","usage":{{{usage_body}}}}}}}"#
        )
    }

    // The guard that protects 2,600 real Claude messages (and 385 Qoder ones):
    // an EMPTY iterations array alongside a non-zero token count. Deriving the
    // call count from the array length books zero Requests for every one of
    // them — worse than the bug being fixed.
    //
    // Two independent things now send an empty array to the top level: the
    // `len() > 1` filter, and the fallback for an array that yields no call. So
    // this test fires when BOTH are gone (the array trusted and the fallback
    // deleted — an empty PerCall books nothing and supersedes the plain key).
    // Trusting the length ALONE is caught by
    // `single_iteration_keeps_todays_record_untouched`, which is the bigger
    // guard: it stands in front of ~104,000 single-entry messages whose keys
    // would otherwise be rewritten and whose Records superseded.
    #[test]
    fn empty_iterations_array_falls_back_to_the_top_level() {
        let line = assistant_line(&format!(r#"{TOP_HYBRID},"iterations":[]"#));
        let parsed = parse_file(format!("{line}\n").as_bytes(), 0, "/p/x/s.jsonl", "enc", "s", |_| None);
        assert_eq!(parsed.events.len(), 1, "an empty array is one Record, not zero");
        let e = &parsed.events[0];
        assert_eq!(e.dedup_key, "claude:m:r", "the key must not gain an iteration suffix");
        assert_eq!(e.api_calls, 1);
        assert_eq!(e.model.as_deref(), Some("claude-opus-4-8"));
        assert_eq!(e.output_tokens, 1728, "top-level figures survive");
        assert_eq!(e.cache_read_tokens, 180874);
        assert_eq!(e.cache_write_1h_tokens, 24);
        assert!(parsed.superseded.is_empty(), "nothing to supersede");
    }

    // 245 older messages have no `iterations` key at all. Same fallback.
    #[test]
    fn absent_iterations_key_falls_back_to_the_top_level() {
        let line = assistant_line(TOP_HYBRID);
        let parsed = parse_file(format!("{line}\n").as_bytes(), 0, "/p/x/s.jsonl", "enc", "s", |_| None);
        assert_eq!(parsed.events.len(), 1);
        assert_eq!(parsed.events[0].dedup_key, "claude:m:r");
        assert_eq!(parsed.events[0].api_calls, 1);
        assert_eq!(parsed.events[0].output_tokens, 1728);
        assert!(parsed.superseded.is_empty());
    }

    // The overwhelming majority (~104,000 messages): exactly one iteration. Its
    // figures are the top-level figures, so the single case is NOT re-derived
    // from the array — the key keeps its historical shape and nothing is
    // superseded, or every one of those Records would be rewritten.
    //
    // This is the test that catches a parser trusting `iterations.len()`:
    // mutation-check by dropping the `len() > 1` filter and it fails on the
    // `#it0` key.
    #[test]
    fn single_iteration_keeps_todays_record_untouched() {
        let line = assistant_line(&format!(r#"{TOP_HYBRID},"iterations":[{IT_OPUS}]"#));
        let parsed = parse_file(format!("{line}\n").as_bytes(), 0, "/p/x/s.jsonl", "enc", "s", |_| None);
        assert_eq!(parsed.events.len(), 1);
        let e = &parsed.events[0];
        assert_eq!(e.dedup_key, "claude:m:r", "no suffix for a single call");
        assert_eq!(e.api_calls, 1);
        assert_eq!(e.cache_write_1h_tokens, 24, "top-level split, not the iteration's 0");
        assert!(parsed.superseded.is_empty());
    }

    // The fix. Two iterations → two Records, each under its OWN Model with its
    // OWN figures. Mutation-check: drop the per-iteration branch (or sum the
    // iterations into one Record) and this fails.
    #[test]
    fn a_model_fallback_books_one_record_per_iteration() {
        let line = assistant_line(&format!(r#"{TOP_HYBRID},"iterations":[{IT_FABLE},{IT_OPUS}]"#));
        let parsed = parse_file(format!("{line}\n").as_bytes(), 0, "/p/x/s.jsonl", "enc", "s", |_| None);
        assert_eq!(parsed.events.len(), 2, "two API calls, two Records");

        let first = &parsed.events[0];
        assert_eq!(first.dedup_key, "claude:m:r#it0");
        assert_eq!(first.model.as_deref(), Some("claude-fable-5"), "the attempt keeps its own Model");
        assert_eq!(first.api_calls, 1, "each Record is one call");
        assert_eq!(first.input_tokens, 2);
        assert_eq!(first.output_tokens, 46);
        assert_eq!(first.cache_read_tokens, 239869, "the figure the top level never reported");
        assert_eq!(first.cache_write_1h_tokens, 24);

        let second = &parsed.events[1];
        assert_eq!(second.dedup_key, "claude:m:r#it1");
        assert_eq!(second.model.as_deref(), Some("claude-opus-4-8"));
        assert_eq!(second.api_calls, 1);
        assert_eq!(second.output_tokens, 1728);
        assert_eq!(second.cache_read_tokens, 180874);
        // The hybrid correction: the top level filed the FIRST iteration's 24
        // here, under the fallback's Model. The fallback wrote no cache.
        assert_eq!(second.cache_write_1h_tokens, 0, "each Record takes its OWN TTL split");

        // Requests is a count of API calls, and the two Records carry all of
        // both calls' tokens — nothing summed into a single mixed-Model row.
        assert_eq!(parsed.events.iter().map(|e| e.api_calls).sum::<i64>(), 2);
        assert_eq!(first.cache_write_1h_tokens + second.cache_write_1h_tokens, 24);
    }

    // The plain key is what today's Ledger holds for these messages, with the
    // fallback's Model and the hybrid cache-write. Per-iteration Records replace
    // it, so it must be superseded rather than left to the keep-max upsert (see
    // `parse_line_events` for why a tie keeps the stored row).
    // Mutation-check: stop reporting the superseded key and this fails.
    #[test]
    fn iteration_records_supersede_the_plain_key() {
        let line = assistant_line(&format!(r#"{TOP_HYBRID},"iterations":[{IT_FABLE},{IT_OPUS}]"#));
        let parsed = parse_file(format!("{line}\n").as_bytes(), 0, "/p/x/s.jsonl", "enc", "s", |_| None);
        assert_eq!(parsed.superseded, vec!["claude:m:r".to_string()]);
        assert!(
            !parsed.events.iter().any(|e| e.dedup_key == "claude:m:r"),
            "the plain key must not also be booked, or the turn counts twice"
        );
    }

    // Three-or-more iterations are unobserved locally, which is not the same as
    // impossible: the parser handles N rather than special-casing 2.
    #[test]
    fn n_iterations_book_n_records() {
        let third = IT_OPUS.replace("1728", "77").replace("claude-opus-4-8", "claude-sonnet-5");
        let line = assistant_line(&format!(r#"{TOP_HYBRID},"iterations":[{IT_FABLE},{IT_OPUS},{third}]"#));
        let parsed = parse_file(format!("{line}\n").as_bytes(), 0, "/p/x/s.jsonl", "enc", "s", |_| None);
        assert_eq!(parsed.events.len(), 3);
        let keys: Vec<&str> = parsed.events.iter().map(|e| e.dedup_key.as_str()).collect();
        assert_eq!(keys, vec!["claude:m:r#it0", "claude:m:r#it1", "claude:m:r#it2"]);
        assert_eq!(parsed.events[2].model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(parsed.events.iter().map(|e| e.api_calls).sum::<i64>(), 3);
    }

    // An all-zero iteration is not a Usage Record, the same rule the top-level
    // figures follow (a <synthetic> placeholder books nothing). The surviving
    // iteration still books, and the plain key is still superseded.
    //
    // Not production's mix, and not pretending to be: this shape occurs 0 times
    // in ~105,000 real lines. It is here to pin the rule, because the all-zero
    // path is where a silent drop actually hid (see the sibling test for a
    // wholly-zero array), not because a reader should expect to meet it.
    #[test]
    fn an_all_zero_iteration_books_no_record() {
        let zero = r#"{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"type":"message","model":"claude-fable-5"}"#;
        let line = assistant_line(&format!(r#"{TOP_HYBRID},"iterations":[{zero},{IT_OPUS}]"#));
        let parsed = parse_file(format!("{line}\n").as_bytes(), 0, "/p/x/s.jsonl", "enc", "s", |_| None);
        assert_eq!(parsed.events.len(), 1, "the zero iteration is not a Record");
        assert_eq!(parsed.events[0].dedup_key, "claude:m:r#it1", "the index stays the iteration's own");
        assert_eq!(parsed.superseded, vec!["claude:m:r".to_string()]);
    }

    // A multi-entry array whose every entry is all-zero reports no call, while
    // the top level still reports real usage. Booking nothing there would lose a
    // Record the old parser kept — a silent drop, which is worse than the floor
    // this ticket fixed. Absent is not zero. Mutation-check: make the
    // per-iteration branch return unconditionally and this fails.
    #[test]
    fn an_all_zero_iterations_array_falls_back_to_the_top_level() {
        let zero = r#"{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"type":"message","model":"claude-fable-5"}"#;
        let line = assistant_line(&format!(r#"{TOP_HYBRID},"iterations":[{zero},{zero}]"#));
        let parsed = parse_file(format!("{line}\n").as_bytes(), 0, "/p/x/s.jsonl", "enc", "s", |_| None);
        assert_eq!(parsed.events.len(), 1, "the billed message is still one Record");
        let e = &parsed.events[0];
        assert_eq!(e.dedup_key, "claude:m:r", "no suffix: there is no per-call figure to key");
        assert_eq!(e.model.as_deref(), Some("claude-opus-4-8"));
        assert_eq!(e.output_tokens, 1728, "top-level figures survive");
        assert_eq!(e.cache_write_1h_tokens, 24);
        assert!(parsed.superseded.is_empty(), "nothing was replaced, so nothing is superseded");
    }

    // Duplicate content-block lines repeat the whole iterations array. Each
    // iteration slot dedups on its own key, so a repeated line adds no Records.
    #[test]
    fn repeated_lines_dedup_per_iteration_slot() {
        let line = assistant_line(&format!(r#"{TOP_HYBRID},"iterations":[{IT_FABLE},{IT_OPUS}]"#));
        let buf = format!("{line}\n{line}\n");
        let parsed = parse_file(buf.as_bytes(), 0, "/p/x/s.jsonl", "enc", "s", |_| None);
        assert_eq!(parsed.events.len(), 4, "the parser emits per line; the upsert dedups");
        let mut keys: Vec<&str> = parsed.events.iter().map(|e| e.dedup_key.as_str()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys, vec!["claude:m:r#it0", "claude:m:r#it1"], "two distinct Records");
    }

    // Production really does mix shapes inside one turn: in a subagent
    // transcript, three lines of msg_011CdhQD… carry NO `iterations` key and the
    // fourth carries two (fable-5 falling back to opus-5). The early lines book
    // the plain key, so without this the turn is counted twice — once as the old
    // single Record, once as its per-iteration Records. Mutation-check: drop the
    // retain() in parse_file and this fails.
    #[test]
    fn a_turn_whose_lines_disagree_about_iterations_books_only_its_calls() {
        let early = assistant_line(TOP_HYBRID);
        let late = assistant_line(&format!(r#"{TOP_HYBRID},"iterations":[{IT_FABLE},{IT_OPUS}]"#));
        let buf = format!("{early}\n{early}\n{late}\n");
        let parsed = parse_file(buf.as_bytes(), 0, "/p/x/s.jsonl", "enc", "s", |_| None);

        assert!(
            !parsed.events.iter().any(|e| e.dedup_key == "claude:m:r"),
            "the plain key must not be booked beside the per-iteration Records",
        );
        let mut keys: Vec<&str> = parsed.events.iter().map(|e| e.dedup_key.as_str()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys, vec!["claude:m:r#it0", "claude:m:r#it1"]);
        // Still reported as superseded: an earlier scan may already have stored
        // the plain-key Record, and only this can clear it.
        assert_eq!(parsed.superseded, vec!["claude:m:r".to_string()]);
    }

    // ctx is a partition of each Record's OWN billed context: both calls sent a
    // context window and both were billed for it, so each Record partitions its
    // own total rather than sharing one message-level split.
    #[test]
    fn each_iteration_partitions_its_own_billed_context() {
        let user = r#"{"type":"user","sessionId":"s","timestamp":"2026-07-01T09:59:00.000Z","message":{"role":"user","content":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}"#;
        let line = assistant_line(&format!(r#"{TOP_HYBRID},"iterations":[{IT_FABLE},{IT_OPUS}]"#));
        let parsed = parse_file(format!("{user}\n{line}\n").as_bytes(), 0, "/p/x/s.jsonl", "enc", "s", |_| None);
        assert_eq!(parsed.events.len(), 2);
        for e in &parsed.events {
            let billed = e.input_tokens + e.cache_read_tokens + e.cache_write_5m_tokens + e.cache_write_1h_tokens;
            let ctx = e.ctx;
            assert_eq!(
                ctx.messages.unwrap() + ctx.system.unwrap() + ctx.reasoning.unwrap_or(0),
                billed,
                "each Record's ctx partitions its own billed exactly"
            );
        }
    }

}
