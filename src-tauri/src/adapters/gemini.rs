use super::unchanged;
use crate::db::{file_has_events, replace_file_events, set_file_state};
use crate::time::iso_to_epoch;
use crate::types::{FileState, SourceScanResult, UsageEvent};
use rusqlite::Connection;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

const MALFORMED_ARTIFACT_WARNING: &str = "gemini: malformed or unsupported chat Artifact";
const MOVED_ARTIFACT_WARNING: &str =
    "gemini: a chat Artifact that had booked Requests now parses to none";

#[derive(Deserialize)]
struct SessionFile {
    #[serde(rename = "sessionId")]
    session_id: String,
    messages: Vec<Message>,
}

#[derive(Deserialize)]
struct Message {
    id: String,
    timestamp: String,
    model: Option<String>,
    tokens: Option<Tokens>,
}

/// One line of the current `.jsonl` Artifact. The five line kinds — header,
/// `$set` patch, and `gemini`/`user`/`info` records — differ only by which
/// fields are present, so everything is optional and the caller classifies:
/// a `sessionId` marks the header, an `id` + `timestamp` marks a record, and
/// a `$set` patch has neither.
#[derive(Deserialize)]
struct JsonlLine {
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    id: Option<String>,
    timestamp: Option<String>,
    model: Option<String>,
    tokens: Option<Tokens>,
}

#[derive(Deserialize)]
struct Tokens {
    input: i64,
    output: i64,
    cached: i64,
    thoughts: i64,
    #[serde(default)]
    tool: i64,
}

pub fn scan_gemini(conn: &mut Connection, tmp_root: &Path, projects_json: &Path) -> SourceScanResult {
    let mut result = SourceScanResult::default();
    let reverse = load_reverse_map(projects_json, &mut result);
    let mut identities = HashSet::new();

    if tmp_root.is_file() {
        let project = resolve_project(project_dir_of(tmp_root), &reverse);
        process_file(conn, tmp_root, &project, &mut result, &mut identities);
        return result;
    }

    let subdirs = match fs::read_dir(tmp_root) {
        Ok(rd) => rd,
        Err(_) => return result, // missing dir → zero events, no error
    };
    for sub in subdirs.flatten() {
        let sub_path = sub.path();
        if !sub_path.is_dir() {
            continue;
        }
        let dir_name = match sub_path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let project = resolve_project(&dir_name, &reverse);
        let chats = sub_path.join("chats");
        let entries = match fs::read_dir(&chats) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // Subagent Sessions sit one level in, under `chats/<uuid>/`, and
            // are named for their own id rather than `session-`.
            if path.is_dir() {
                let Ok(nested) = fs::read_dir(&path) else { continue };
                for child in nested.flatten() {
                    let child = child.path();
                    if child.is_file() && is_chat_artifact(&child) {
                        process_file(conn, &child, &project, &mut result, &mut identities);
                    }
                }
                continue;
            }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            if name.starts_with("session-") && is_chat_artifact(&path) {
                process_file(conn, &path, &project, &mut result, &mut identities);
            }
        }
    }
    result
}

fn process_file(
    conn: &mut Connection,
    path: &Path,
    project: &str,
    result: &mut SourceScanResult,
    identities: &mut HashSet<String>,
) {
    let path_str = path.to_string_lossy().to_string();
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return,
    };
    let size = meta.len() as i64;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // unchanged (same size AND mtime) → skip whole file
    if unchanged(conn, path, &FileState { size, mtime, byte_offset: 0 }) {
        return;
    }

    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => {
            result.lines_skipped += 1;
            record_gemini_warning(result, MALFORMED_ARTIFACT_WARNING);
            return;
        }
    };
    let session = match parse_session(path, &content, result) {
        Some(s) => s,
        None => {
            result.lines_skipped += 1;
            record_gemini_warning(result, MALFORMED_ARTIFACT_WARNING);
            return;
        }
    };

    let mut events = Vec::new();
    for m in &session.messages {
        let tokens = match &m.tokens {
            Some(t) => t,
            None => continue, // non-token messages contribute nothing
        };
        if tokens.input < 0
            || tokens.output < 0
            || tokens.cached < 0
            || tokens.thoughts < 0
            || tokens.tool < 0
            || tokens.cached > tokens.input
        {
            record_gemini_warning(result, MALFORMED_ARTIFACT_WARNING);
            result.lines_skipped += 1;
            continue;
        }
        let input_tokens = tokens.input.saturating_sub(tokens.cached);
        let output_tokens = tokens.output.saturating_add(tokens.thoughts);
        let total_tokens = input_tokens
            .saturating_add(output_tokens)
            .saturating_add(tokens.cached);
        if total_tokens <= 0 {
            result.lines_skipped += 1;
            continue;
        }
        let ts = match iso_to_epoch(&m.timestamp) {
            Some(t) => t,
            None => {
                result.lines_skipped += 1;
                record_gemini_warning(result, MALFORMED_ARTIFACT_WARNING);
                continue;
            }
        };
        let dedup_key = format!("gemini:{}:{}", session.session_id, m.id);
        if !identities.insert(dedup_key.clone()) {
            continue;
        }
        events.push(UsageEvent {
            dedup_key,
            source: "gemini".to_string(),
            timestamp: ts,
            model: m.model.clone().filter(|model| !model.trim().is_empty()),
            project: Some(project.to_string()),
            api_calls: 1,
            input_tokens,
            output_tokens,
            cache_read_tokens: tokens.cached,
            cache_write_5m_tokens: 0,
            cache_write_1h_tokens: 0,
            source_file: path_str.clone(),
            session_id: Some(session.session_id.clone()),
            reasoning_tokens: Some(tokens.thoughts),
            ctx: crate::types::CtxTokens {
                // Billed context = raw input (cached is a subset; no cache writes).
                messages: Some(tokens.input.max(0)),
                system: None,
                reasoning: None, // thoughts are output-side, never re-sent as input
                toolcalls: Some(tokens.tool.clamp(0, tokens.input.max(0))),
                agents: None,
                mcp: None,
                skills: None,
            },
        });
    }

    // Nothing parsed is not the same as nothing consumed: a Session whose usage
    // field has moved still reads as valid JSON, and the replace below would
    // delete Records this parser can no longer re-derive. Leaving the file
    // unstamped re-parses it next Scan.
    if events.is_empty() {
        // A file that booked Requests before and books none now is the Source
        // moving its Artifact underneath us. Keeping the Records is the right
        // call, but doing it silently is how this Source read as idle for
        // 3.7 months (TOKL-23) — say so.
        // Unreadable is not "no Records": if the Ledger cannot answer, warn
        // anyway rather than let a failed query buy the silence this whole
        // ticket is about.
        if file_has_events(conn, &path_str).unwrap_or(true) {
            record_gemini_warning(result, MOVED_ARTIFACT_WARNING);
        }
        return;
    }

    let n = events.len() as u64;
    if replace_file_events(conn, &path_str, &events).is_err() {
        result.error = Some(format!("failed to write events for {}", path_str));
        return;
    }
    result.events_inserted += n;
    let _ = set_file_state(conn, &path_str, FileState { size, mtime, byte_offset: 0 });
}

/// Both Artifact shapes reduce to the same thing: a `sessionId` and the
/// records under it. Only the framing moved on 2026-05-03 (TOKL-23) — the
/// per-record fields, the token maths and the dedup key are unchanged.
fn parse_session(
    path: &Path,
    content: &str,
    result: &mut SourceScanResult,
) -> Option<SessionFile> {
    if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return serde_json::from_str(content).ok();
    }

    // One rule governs the classification below: a line carrying counts is
    // usage, and usage never leaves quietly. Every path that declines to book
    // a line holding `tokens` says so, because a Request that vanishes without
    // a word is the failure this Source spent 3.7 months demonstrating.
    let skip = |result: &mut SourceScanResult| {
        result.lines_skipped += 1;
        record_gemini_warning(result, MALFORMED_ARTIFACT_WARNING);
    };

    let mut session_id = None;
    let mut messages = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<JsonlLine>(line) else {
            // A line this reader cannot shape may still have been a Request.
            // The file's other lines are readable, so keep the Session.
            skip(result);
            continue;
        };
        // The header names the Session and carries no counts. A line holding
        // counts is a Request whatever else it holds, so it can never take
        // this branch — were the Source to start stamping records with their
        // `sessionId`, every one of them would otherwise be read as a header
        // and dropped.
        if parsed.tokens.is_none() {
            if let Some(id) = parsed.session_id {
                session_id.get_or_insert(id);
                continue;
            }
        }
        // The dedup key cannot be formed without an id. A `$set` patch has
        // neither id nor counts and is simply not a Request; a line with counts
        // and no id is usage this reader cannot identify, which is a warning.
        let Some(id) = parsed.id else {
            if parsed.tokens.is_some() {
                skip(result);
            }
            continue;
        };
        // A missing or unreadable timestamp is left to the stamp check in the
        // caller, which already warns.
        messages.push(Message {
            id,
            timestamp: parsed.timestamp.unwrap_or_default(),
            model: parsed.model,
            tokens: parsed.tokens,
        });
    }
    Some(SessionFile { session_id: session_id?, messages })
}

/// The Project directory is the parent of `chats/`, however deep the Artifact
/// sits under it — a nested subagent Session is one level further down than a
/// top-level one, so counting parents gets it wrong.
fn project_dir_of(path: &Path) -> &str {
    path.ancestors()
        .find(|a| a.file_name().is_some_and(|n| n == "chats"))
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        .unwrap_or("")
}

/// A chat Artifact in either shape. The `session-` prefix is only meaningful
/// for a top-level Session: a nested subagent file is named for its own id.
fn is_chat_artifact(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()), Some("json" | "jsonl"))
}

fn record_gemini_warning(result: &mut SourceScanResult, warning: &str) {
    match &mut result.error {
        Some(existing) if !existing.contains(warning) => {
            existing.push_str("; ");
            existing.push_str(warning);
        }
        Some(_) => {}
        None => result.error = Some(warning.to_string()),
    }
}

/// projects.json is `{"projects": {realPath: friendlyName}}`; build friendly → real.
fn load_reverse_map(
    projects_json: &Path,
    result: &mut SourceScanResult,
) -> HashMap<String, String> {
    #[derive(Deserialize)]
    struct Projects {
        projects: HashMap<String, String>,
    }
    let mut map = HashMap::new();
    let content = match fs::read_to_string(projects_json) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return map,
        Err(_) => {
            record_gemini_warning(result, MALFORMED_ARTIFACT_WARNING);
            return map;
        }
    };
    let p = match serde_json::from_str::<Projects>(&content) {
        Ok(projects) => projects,
        Err(_) => {
            record_gemini_warning(result, MALFORMED_ARTIFACT_WARNING);
            return map;
        }
    };
    for (real, friendly) in p.projects {
        map.insert(friendly, real);
    }
    map
}

fn resolve_project(dir_name: &str, reverse: &HashMap<String, String>) -> String {
    match reverse.get(dir_name) {
        Some(real) => real.clone(),
        None => dir_name.chars().take(8).collect(), // hash dir → shortened hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // session with an info message (no tokens → skipped) and two gemini messages
    const SESSION_ALPHA: &str = r#"{
      "sessionId": "sess-alpha",
      "projectHash": "alpha",
      "startTime": "2026-03-01T10:00:00.000Z",
      "lastUpdated": "2026-03-01T11:30:00.500Z",
      "messages": [
        { "id": "m0", "timestamp": "2026-03-01T10:00:00.000Z", "type": "info",
          "content": "Gemini CLI update available!" },
        { "id": "m1", "timestamp": "2026-03-01T10:05:00.000Z", "type": "gemini",
          "model": "gemini-2.5-flash",
          "tokens": { "input": 1000, "output": 200, "cached": 300, "thoughts": 50, "tool": 0, "total": 1250 } },
        { "id": "m2", "timestamp": "2026-03-01T11:30:00.500Z", "type": "gemini",
          "model": "gemini-2.5-flash",
          "tokens": { "input": 500, "output": 100, "cached": 0, "thoughts": 0, "tool": 120, "total": 600 } }
      ]
    }"#;

    // session under a hash-named dir (not in projects.json → shortened to 8 chars)
    const SESSION_HASH: &str = r#"{
      "sessionId": "sess-beta",
      "projectHash": "abcdef1234567890",
      "startTime": "2026-03-02T09:00:00.000Z",
      "lastUpdated": "2026-03-02T09:00:00.000Z",
      "messages": [
        { "id": "m3", "timestamp": "2026-03-02T09:00:00.000Z", "type": "gemini",
          "model": "gemini-3-pro-preview",
          "tokens": { "input": 800, "output": 400, "cached": 200, "thoughts": 100, "tool": 0, "total": 1300 } }
      ]
    }"#;

    fn write(path: &std::path::Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn test_scan_gemini_extracts_and_maps() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let tmp_root = base.join("tmp");
        let projects_json = base.join("projects.json");

        std::fs::write(
            &projects_json,
            r#"{"projects":{"/Users/dev/projects/alpha":"alpha"}}"#,
        )
        .unwrap();
        write(&tmp_root.join("alpha/chats/session-1.json"), SESSION_ALPHA);
        write(&tmp_root.join("abcdef1234567890/chats/session-2.json"), SESSION_HASH);
        write(&tmp_root.join("alpha/chats/session-bad.json"), "{ not json");

        let mut conn = crate::db::open_db(&base.join("t.db")).unwrap();
        let r = scan_gemini(&mut conn, &tmp_root, &projects_json);
        assert_eq!(r.events_inserted, 3);
        assert_eq!(r.lines_skipped, 1); // the malformed file only
        assert!(r
            .error
            .as_deref()
            .is_some_and(|error| error.contains("gemini") && error.contains("malformed")));

        // m1: input excludes cached (1000-300); output includes thoughts (200+50)
        let (input, output, cread, model, project): (i64, i64, i64, String, String) = conn
            .query_row(
                "SELECT input_tokens, output_tokens, cache_read_tokens, model, project \
                 FROM events WHERE dedup_key = 'gemini:sess-alpha:m1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap();
        assert_eq!(input, 700);
        assert_eq!(output, 250);
        assert_eq!(cread, 300);
        assert_eq!(model, "gemini-2.5-flash");
        assert_eq!(project, "/Users/dev/projects/alpha"); // friendly-name reverse map

        // v2 columns: session id and thoughts-as-reasoning.
        let (sid, rt): (Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT session_id, reasoning_tokens FROM events WHERE dedup_key = 'gemini:sess-alpha:m1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(sid, Some("sess-alpha".to_string()));
        assert_eq!(rt, Some(50), "thoughts reported as reasoning subset");
        let rt2: Option<i64> = conn
            .query_row(
                "SELECT reasoning_tokens FROM events WHERE dedup_key = 'gemini:sess-alpha:m2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rt2, Some(0), "reported zero, not NULL");

        // Context attribution: messages = raw input (incl. cached) = billed;
        // toolcalls = reported tokens.tool; the rest NULL.
        let (cm, ct, cs, cr): (i64, i64, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT ctx_messages, ctx_toolcalls, ctx_system, ctx_reasoning \
                 FROM events WHERE dedup_key = 'gemini:sess-alpha:m1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(cm, 1000, "billed context = raw input incl. cached");
        assert_eq!(ct, 0);
        assert_eq!(cs, None);
        assert_eq!(cr, None, "thoughts are output-side, never re-sent as input");
        let (cm2, ct2): (i64, i64) = conn
            .query_row(
                "SELECT ctx_messages, ctx_toolcalls FROM events WHERE dedup_key = 'gemini:sess-alpha:m2'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(cm2, 500);
        assert_eq!(ct2, 120, "reported tokens.tool, not an estimate");

        // m1 timestamp = epoch of 2026-03-01T10:05:00Z
        let ts: i64 = conn
            .query_row(
                "SELECT timestamp FROM events WHERE dedup_key = 'gemini:sess-alpha:m1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ts, 1772359500);

        // m3: hash dir shortened to first 8 chars; math 800-200 / 400+100
        let (i3, o3, project3): (i64, i64, String) = conn
            .query_row(
                "SELECT input_tokens, output_tokens, project \
                 FROM events WHERE dedup_key = 'gemini:sess-beta:m3'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(i3, 600);
        assert_eq!(o3, 500);
        assert_eq!(project3, "abcdef12");

        // the info message (no tokens) produced no event
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 3);

        // idempotent: unchanged files skipped → 0 new, still 3 total
        let r2 = scan_gemini(&mut conn, &tmp_root, &projects_json);
        assert_eq!(r2.events_inserted, 0);
        let count2: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count2, 3);
    }

    #[test]
    fn test_scan_gemini_replaces_changed_file() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let tmp_root = base.join("tmp");
        let projects_json = base.join("projects.json");
        std::fs::write(&projects_json, r#"{"projects":{}}"#).unwrap();
        let session = tmp_root.join("proj/chats/session-x.json");

        write(
            &session,
            r#"{"sessionId":"sx","messages":[
              {"id":"a","timestamp":"2026-03-01T10:00:00.000Z","type":"gemini","model":"gemini-2.5-flash",
               "tokens":{"input":100,"output":10,"cached":0,"thoughts":0,"tool":0,"total":110}}
            ]}"#,
        );

        let mut conn = crate::db::open_db(&base.join("t.db")).unwrap();
        let r1 = scan_gemini(&mut conn, &tmp_root, &projects_json);
        assert_eq!(r1.events_inserted, 1);

        // rewrite with two different messages (larger size → change detected)
        write(
            &session,
            r#"{"sessionId":"sx","messages":[
              {"id":"b","timestamp":"2026-03-01T10:01:00.000Z","type":"gemini","model":"gemini-2.5-flash",
               "tokens":{"input":200,"output":20,"cached":0,"thoughts":0,"tool":0,"total":220}},
              {"id":"c","timestamp":"2026-03-01T10:02:00.000Z","type":"gemini","model":"gemini-2.5-flash",
               "tokens":{"input":300,"output":30,"cached":0,"thoughts":0,"tool":0,"total":330}}
            ]}"#,
        );

        let r2 = scan_gemini(&mut conn, &tmp_root, &projects_json);
        assert_eq!(r2.events_inserted, 2);

        // old event 'a' was deleted by replace-per-file; only b, c remain
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
        let has_a: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE dedup_key = 'gemini:sx:a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_a, 0);
    }

    #[test]
    fn missing_model_is_stored_as_unattributed_usage() {
        let dir = tempfile::tempdir().unwrap();
        let tmp_root = dir.path().join("tmp");
        let projects_json = dir.path().join("projects.json");
        std::fs::write(&projects_json, r#"{"projects":{}}"#).unwrap();
        write(
            &tmp_root.join("project/chats/session-unattributed.json"),
            r#"{"sessionId":"unattributed","messages":[
              {"id":"m","timestamp":"2026-03-01T10:00:00.000Z",
               "tokens":{"input":100,"output":20,"cached":10,"thoughts":5,"tool":0,"total":115}}
            ]}"#,
        );

        let mut conn = crate::db::open_db(&dir.path().join("ledger.db")).unwrap();
        let result = scan_gemini(&mut conn, &tmp_root, &projects_json);
        assert_eq!(result.events_inserted, 1);

        let (model, input, output, cached): (Option<String>, i64, i64, i64) = conn
            .query_row(
                "SELECT model, input_tokens, output_tokens, cache_read_tokens FROM events",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(model, None);
        assert_eq!(input, 90);
        assert_eq!(output, 25);
        assert_eq!(cached, 10);
    }

    #[test]
    fn zero_or_negative_token_observations_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let tmp_root = dir.path().join("tmp");
        let projects_json = dir.path().join("projects.json");
        std::fs::write(&projects_json, r#"{"projects":{}}"#).unwrap();
        write(
            &tmp_root.join("project/chats/session-tokens.json"),
            r#"{"sessionId":"tokens","messages":[
              {"id":"zero","timestamp":"2026-03-01T10:00:00.000Z","model":"gemini-2.5-flash",
               "tokens":{"input":0,"output":0,"cached":0,"thoughts":0,"tool":0,"total":0}},
              {"id":"negative","timestamp":"2026-03-01T10:01:00.000Z","model":"gemini-2.5-flash",
               "tokens":{"input":-1,"output":0,"cached":0,"thoughts":0,"tool":0,"total":-1}},
              {"id":"usage","timestamp":"2026-03-01T10:02:00.000Z","model":"gemini-2.5-flash",
               "tokens":{"input":100,"output":20,"cached":30,"thoughts":5,"tool":10,"total":125}}
            ]}"#,
        );

        let mut conn = crate::db::open_db(&dir.path().join("ledger.db")).unwrap();
        let result = scan_gemini(&mut conn, &tmp_root, &projects_json);
        assert_eq!(result.events_inserted, 1);
        assert_eq!(result.lines_skipped, 2);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn inconsistent_cache_counters_are_skipped_with_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let tmp_root = dir.path().join("tmp");
        let projects_json = dir.path().join("projects.json");
        std::fs::write(&projects_json, r#"{"projects":{}}"#).unwrap();
        write(
            &tmp_root.join("project/chats/session-inconsistent.json"),
            r#"{"sessionId":"inconsistent","messages":[
              {"id":"m","timestamp":"2026-03-01T10:00:00.000Z","model":"gemini-2.5-flash",
               "tokens":{"input":10,"output":20,"cached":11,"thoughts":0,"tool":0,"total":30}}
            ]}"#,
        );

        let mut conn = crate::db::open_db(&dir.path().join("ledger.db")).unwrap();
        let result = scan_gemini(&mut conn, &tmp_root, &projects_json);
        assert_eq!(result.events_inserted, 0);
        assert_eq!(result.lines_skipped, 1);
        assert!(result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("gemini") && error.contains("malformed")));
    }

    #[test]
    fn malformed_projects_artifact_reports_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let tmp_root = dir.path().join("tmp");
        let projects_json = dir.path().join("projects.json");
        std::fs::create_dir_all(&tmp_root).unwrap();
        std::fs::write(&projects_json, "{ not json").unwrap();

        let mut conn = crate::db::open_db(&dir.path().join("ledger.db")).unwrap();
        let result = scan_gemini(&mut conn, &tmp_root, &projects_json);
        assert_eq!(result.events_inserted, 0);
        assert!(result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("gemini") && error.contains("malformed")));
    }

    #[test]
    fn duplicate_message_identities_are_counted_once() {
        let dir = tempfile::tempdir().unwrap();
        let tmp_root = dir.path().join("tmp");
        let projects_json = dir.path().join("projects.json");
        std::fs::write(&projects_json, r#"{"projects":{}}"#).unwrap();
        write(
            &tmp_root.join("project/chats/session-duplicate.json"),
            r#"{"sessionId":"duplicate","messages":[
              {"id":"m","timestamp":"2026-03-01T10:00:00.000Z","model":"gemini-2.5-flash",
              "tokens":{"input":100,"output":20,"cached":0,"thoughts":0,"tool":0,"total":120}},
              {"id":"m","timestamp":"2026-03-01T10:00:01.000Z","model":"gemini-2.5-flash",
              "tokens":{"input":900,"output":90,"cached":0,"thoughts":0,"tool":0,"total":990}}
            ]}"#,
        );
        write(
            &tmp_root.join("other/chats/session-duplicate-copy.json"),
            r#"{"sessionId":"duplicate","messages":[
              {"id":"m","timestamp":"2026-03-01T10:00:00.000Z","model":"gemini-2.5-flash",
               "tokens":{"input":100,"output":20,"cached":0,"thoughts":0,"tool":0,"total":120}}
            ]}"#,
        );

        let mut conn = crate::db::open_db(&dir.path().join("ledger.db")).unwrap();
        let result = scan_gemini(&mut conn, &tmp_root, &projects_json);
        assert_eq!(result.events_inserted, 1);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("SELECT input_tokens FROM events", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            100
        );
    }

    // TOKL-28: a Session whose usage field moved parses to zero events, and the
    // replace below would delete the Records it already booked. Stale Records
    // are recoverable by a re-Scan once the parser is fixed; deleted ones are
    // not recoverable at all.
    #[test]
    fn renamed_token_field_keeps_booked_records() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let tmp_root = base.join("tmp");
        let projects_json = base.join("projects.json");
        std::fs::write(&projects_json, r#"{"projects":{}}"#).unwrap();
        let session = tmp_root.join("alpha/chats/session-1.json");
        write(&session, SESSION_ALPHA);
        let path_str = session.to_string_lossy().to_string();

        let mut conn = crate::db::open_db(&base.join("t.db")).unwrap();
        assert_eq!(scan_gemini(&mut conn, &tmp_root, &projects_json).events_inserted, 2);

        // Gemini renames `tokens` -> `usage`. Still valid JSON, still the shape
        // serde accepts: every message simply reports no tokens.
        let moved = SESSION_ALPHA.replace("\"tokens\":", "\"usage\":");
        assert_ne!(
            moved.len(),
            SESSION_ALPHA.len(),
            "size must differ, or unchanged() skips the file and this test proves nothing"
        );
        write(&session, &moved);

        let result = scan_gemini(&mut conn, &tmp_root, &projects_json);
        assert_eq!(result.events_inserted, 0);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            2,
            "the empty parse must not delete Records this parser can no longer re-derive"
        );
        // ...and it says so. Keeping the Records silently is how this Source
        // read as idle for 3.7 months.
        assert!(
            result.error.as_deref().is_some_and(|e| e.contains(MOVED_ARTIFACT_WARNING)),
            "a previously-productive Artifact that now parses to nothing must warn: {:?}",
            result.error
        );

        // Unstamped: the state still describes the pre-rename file, so the next
        // Scan re-reads it instead of treating the empty read as settled. This
        // is the whole proof that the retry is real — `unchanged()` keys on
        // size and mtime, and the stored pair no longer matches the file.
        let state = crate::db::get_file_state(&conn, &path_str).unwrap().unwrap();
        assert_eq!(state.size, SESSION_ALPHA.len() as i64);
        assert_ne!(state.size, moved.len() as i64, "empty parse must not be marked scanned");
    }

    // The current chat Artifact (2026-05-03 onward): one JSON object per line.
    // Line 0 is the session header; `$set` lines patch metadata; `user`/`info`
    // lines carry no tokens. Shape taken from real files under ~/.gemini/tmp.
    const SESSION_JSONL: &str = concat!(
        r#"{"sessionId":"sess-jsonl","projectHash":"alpha","startTime":"2026-05-03T08:00:00.000Z","lastUpdated":"2026-05-03T08:05:00.000Z","kind":"session"}"#, "\n",
        r#"{"id":"u1","timestamp":"2026-05-03T08:00:30.000Z","type":"user","content":"hi"}"#, "\n",
        r#"{"id":"g1","timestamp":"2026-05-03T08:01:34.117Z","type":"gemini","model":"gemini-3.1-pro-preview","content":"","tokens":{"input":16870,"output":40,"cached":0,"thoughts":196,"tool":0,"total":17106}}"#, "\n",
        r#"{"$set":{"lastUpdated":"2026-05-03T08:01:35.000Z"}}"#, "\n",
        r#"{"id":"g2","timestamp":"2026-05-03T08:02:00.000Z","type":"gemini","model":"gemini-3.1-pro-preview","content":"","tokens":{"input":20000,"output":50,"cached":4000,"thoughts":10,"tool":0,"total":20060}}"#, "\n",
        r#"{"id":"i1","timestamp":"2026-05-03T08:03:00.000Z","type":"info","content":"Request cancelled."}"#, "\n",
    );

    fn jsonl_fixture(base: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
        let tmp_root = base.join("tmp");
        let projects_json = base.join("projects.json");
        std::fs::write(&projects_json, r#"{"projects":{"/Users/dev/projects/alpha":"alpha"}}"#)
            .unwrap();
        (tmp_root, projects_json)
    }

    // TOKL-23: Gemini CLI moved the chat Artifact to one JSON object per line
    // and renamed the extension, so the Scan stopped opening the file at all.
    #[test]
    fn a_jsonl_session_books_its_usage_lines() {
        let dir = tempfile::tempdir().unwrap();
        let (tmp_root, projects_json) = jsonl_fixture(dir.path());
        write(&tmp_root.join("alpha/chats/session-2026-05-03T08-00-abcd.jsonl"), SESSION_JSONL);

        let mut conn = crate::db::open_db(&dir.path().join("t.db")).unwrap();
        let r = scan_gemini(&mut conn, &tmp_root, &projects_json);
        assert!(r.error.is_none(), "{:?}", r.error);
        assert_eq!(r.events_inserted, 2, "header, $set, user and info lines are not Requests");

        // sessionId comes off the header line, so the dedup key shape is unchanged.
        let (input, output, cread, model, project, sid, reasoning): (
            i64, i64, i64, String, String, Option<String>, Option<i64>,
        ) = conn
            .query_row(
                "SELECT input_tokens, output_tokens, cache_read_tokens, model, project, \
                        session_id, reasoning_tokens \
                 FROM events WHERE dedup_key = 'gemini:sess-jsonl:g1'",
                [],
                |r| {
                    Ok((
                        r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(input, 16870); // no cache read to exclude
        assert_eq!(output, 236); // 40 + 196 thoughts
        assert_eq!(cread, 0);
        assert_eq!(model, "gemini-3.1-pro-preview");
        assert_eq!(project, "/Users/dev/projects/alpha");
        assert_eq!(sid, Some("sess-jsonl".to_string()));
        assert_eq!(reasoning, Some(196));

        let (i2, o2, c2): (i64, i64, i64) = conn
            .query_row(
                "SELECT input_tokens, output_tokens, cache_read_tokens \
                 FROM events WHERE dedup_key = 'gemini:sess-jsonl:g2'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(i2, 16000); // 20000 - 4000 cached, per ADR-0001
        assert_eq!(o2, 60);
        assert_eq!(c2, 4000);
    }

    // A `.jsonl` is append-only: Gemini writes the record when the model
    // answers, then writes it again with `toolCalls` filled in. Same id, same
    // timestamp, same tokens — one Request, and the second copy must not book.
    // Every tool-using Request on this machine appears exactly twice this way.
    #[test]
    fn a_record_reappended_with_tool_calls_books_once() {
        let dir = tempfile::tempdir().unwrap();
        let (tmp_root, projects_json) = jsonl_fixture(dir.path());
        let replayed = format!(
            "{SESSION_JSONL}{}\n",
            r#"{"id":"g1","timestamp":"2026-05-03T08:01:34.117Z","type":"gemini","model":"gemini-3.1-pro-preview","content":"","toolCalls":[{"name":"read_file","result":"ok"}],"tokens":{"input":16870,"output":40,"cached":0,"thoughts":196,"tool":0,"total":17106}}"#
        );
        write(&tmp_root.join("alpha/chats/session-2026-05-03T08-00-abcd.jsonl"), &replayed);

        let mut conn = crate::db::open_db(&dir.path().join("t.db")).unwrap();
        let r = scan_gemini(&mut conn, &tmp_root, &projects_json);
        assert_eq!(r.events_inserted, 2, "the re-appended record is the same Request");

        let (rows, tokens): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), \
                 COALESCE(SUM(input_tokens + output_tokens + cache_read_tokens), 0) \
                 FROM events WHERE dedup_key = 'gemini:sess-jsonl:g1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(rows, 1);
        assert_eq!(tokens, 16870 + 236, "billing the replay would double this Request");
    }

    // The same fault in the shape Gemini actually writes today: the counts move
    // out from under `tokens`, every line still parses, and the Session books
    // nothing. Its Records must survive, and the Scan must say so.
    #[test]
    fn a_jsonl_session_whose_counts_moved_keeps_its_records_and_warns() {
        let dir = tempfile::tempdir().unwrap();
        let (tmp_root, projects_json) = jsonl_fixture(dir.path());
        let session = tmp_root.join("alpha/chats/session-2026-05-03T08-00-abcd.jsonl");
        write(&session, SESSION_JSONL);

        let mut conn = crate::db::open_db(&dir.path().join("t.db")).unwrap();
        assert_eq!(scan_gemini(&mut conn, &tmp_root, &projects_json).events_inserted, 2);

        let moved = SESSION_JSONL.replace(r#""tokens":"#, r#""usage":"#);
        assert_ne!(moved.len(), SESSION_JSONL.len(), "size must differ, or unchanged() skips it");
        write(&session, &moved);

        let r = scan_gemini(&mut conn, &tmp_root, &projects_json);
        assert_eq!(r.events_inserted, 0);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            2,
            "a renamed count must not delete the Requests already booked"
        );
        assert!(
            r.error.as_deref().is_some_and(|e| e.contains(MOVED_ARTIFACT_WARNING)),
            "expected the moved-Artifact warning, got {:?}",
            r.error
        );
        // Unstamped, so the next Scan retries rather than settling for nothing.
        let state = crate::db::get_file_state(&conn, &session.to_string_lossy())
            .unwrap()
            .unwrap();
        assert_eq!(state.size, SESSION_JSONL.len() as i64);
    }

    // Classification must never cost a Request. This Source already moved its
    // Artifact once (TOKL-23); were it to stamp each record with the Session it
    // belongs to, a reader that treats any `sessionId` as the header would drop
    // every record in the file and report an idle Source — the same silence,
    // reached a different way. Counts decide: a line carrying them is usage.
    #[test]
    fn a_usage_line_is_not_mistaken_for_the_header() {
        let dir = tempfile::tempdir().unwrap();
        let (tmp_root, projects_json) = jsonl_fixture(dir.path());
        write(
            &tmp_root.join("alpha/chats/session-2026-05-03T08-00-abcd.jsonl"),
            concat!(
                r#"{"sessionId":"sess-jsonl","projectHash":"alpha","kind":"session"}"#, "\n",
                r#"{"sessionId":"sess-jsonl","id":"g1","timestamp":"2026-05-03T08:01:34.117Z","type":"gemini","model":"gemini-3.1-pro-preview","tokens":{"input":600,"output":20,"cached":100,"thoughts":5,"tool":0,"total":625}}"#, "\n",
            ),
        );

        let mut conn = crate::db::open_db(&dir.path().join("t.db")).unwrap();
        let r = scan_gemini(&mut conn, &tmp_root, &projects_json);
        assert_eq!(r.events_inserted, 1, "a record that names its Session is still a Request");
        let (input, sid): (i64, Option<String>) = conn
            .query_row(
                "SELECT input_tokens, session_id FROM events \
                 WHERE dedup_key = 'gemini:sess-jsonl:g1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(input, 500); // 600 - 100 cached
        assert_eq!(sid, Some("sess-jsonl".to_string()));
    }

    // A Usage Record is identified by `gemini:{sessionId}:{id}`, so a line with
    // no id is not a Request whatever else it carries. This Source rewrites its
    // Artifact (TOKL-23); were it to rename `id`, every usage line would share
    // the empty identity and collapse into one phantom Record under it.
    #[test]
    fn a_usage_line_without_an_id_is_not_a_request() {
        let dir = tempfile::tempdir().unwrap();
        let (tmp_root, projects_json) = jsonl_fixture(dir.path());
        write(
            &tmp_root.join("alpha/chats/session-2026-05-03T08-00-abcd.jsonl"),
            concat!(
                r#"{"sessionId":"sess-jsonl","projectHash":"alpha","kind":"session"}"#, "\n",
                r#"{"messageId":"g1","timestamp":"2026-05-03T08:01:34.117Z","type":"gemini","model":"gemini-3.1-pro-preview","tokens":{"input":999,"output":9,"cached":0,"thoughts":0,"tool":0,"total":1008}}"#, "\n",
                r#"{"messageId":"g2","timestamp":"2026-05-03T08:02:00.000Z","type":"gemini","model":"gemini-3.1-pro-preview","tokens":{"input":777,"output":7,"cached":0,"thoughts":0,"tool":0,"total":784}}"#, "\n",
            ),
        );

        let mut conn = crate::db::open_db(&dir.path().join("t.db")).unwrap();
        let r = scan_gemini(&mut conn, &tmp_root, &projects_json);
        assert_eq!(r.events_inserted, 0);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0,
            "an unidentifiable line must not book under an empty identity"
        );
        // Declining to book it is only half the answer. Usage this reader
        // cannot identify is exactly the silent-idle failure TOKL-23 exists to
        // end, one field over, so both lines are counted and the Scan says so.
        assert_eq!(r.lines_skipped, 2, "both unidentifiable Requests are counted");
        assert!(
            r.error.as_deref().is_some_and(|e| e.contains(MALFORMED_ARTIFACT_WARNING)),
            "usage that cannot be identified must not pass in silence: {:?}",
            r.error
        );
    }

    // A usage line the Scan cannot stamp is skipped loudly, not dropped: the
    // record reaches the timestamp check rather than being filtered out for
    // want of a field the dedup key never needed.
    #[test]
    fn a_jsonl_record_with_an_unreadable_timestamp_warns() {
        let dir = tempfile::tempdir().unwrap();
        let (tmp_root, projects_json) = jsonl_fixture(dir.path());
        write(
            &tmp_root.join("alpha/chats/session-2026-05-03T08-00-abcd.jsonl"),
            concat!(
                r#"{"sessionId":"sess-jsonl","projectHash":"alpha","kind":"session"}"#, "\n",
                r#"{"id":"g1","type":"gemini","model":"gemini-3.1-pro-preview","tokens":{"input":100,"output":10,"cached":0,"thoughts":0,"tool":0,"total":110}}"#, "\n",
            ),
        );

        let mut conn = crate::db::open_db(&dir.path().join("t.db")).unwrap();
        let r = scan_gemini(&mut conn, &tmp_root, &projects_json);
        assert_eq!(r.events_inserted, 0);
        assert_eq!(r.lines_skipped, 1, "the record is skipped, not silently discarded");
        assert!(
            r.error.as_deref().is_some_and(|e| e.contains(MALFORMED_ARTIFACT_WARNING)),
            "expected a warning, got {:?}",
            r.error
        );
    }

    // Scanning one Artifact directly (`tmp_root.is_file()`) must resolve the
    // Project the same way. A nested subagent file sits one level deeper than a
    // top-level Session, so counting parents lands on `chats` instead.
    #[test]
    fn a_single_nested_file_resolves_its_project() {
        let dir = tempfile::tempdir().unwrap();
        let (tmp_root, projects_json) = jsonl_fixture(dir.path());
        let nested = tmp_root.join("alpha/chats/abcd-1111/ldsu1h.jsonl");
        write(
            &nested,
            concat!(
                r#"{"sessionId":"ldsu1h","projectHash":"alpha","kind":"subagent"}"#, "\n",
                r#"{"id":"c1","timestamp":"2026-05-03T08:01:40.000Z","type":"gemini","model":"gemini-3.1-pro-preview","tokens":{"input":900,"output":10,"cached":100,"thoughts":5,"tool":0,"total":915}}"#, "\n",
            ),
        );

        let mut conn = crate::db::open_db(&dir.path().join("t.db")).unwrap();
        let r = scan_gemini(&mut conn, &nested, &projects_json);
        assert_eq!(r.events_inserted, 1);
        let project: String = conn
            .query_row(
                "SELECT project FROM events WHERE dedup_key = 'gemini:ldsu1h:c1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            project, "/Users/dev/projects/alpha",
            "the Project is the parent of `chats`, however deep the Artifact sits"
        );
    }

    // Subagent Sessions live one directory in, under `chats/<uuid>/`, and carry
    // no `session-` prefix. Their ids do not overlap the parent's, so they are
    // additional Requests — but only if the Scan walks into the directory.
    #[test]
    fn a_nested_subagent_session_books_beside_its_parent() {
        let dir = tempfile::tempdir().unwrap();
        let (tmp_root, projects_json) = jsonl_fixture(dir.path());
        let chats = tmp_root.join("alpha/chats");
        write(&chats.join("session-2026-05-03T08-00-abcd.jsonl"), SESSION_JSONL);
        write(
            &chats.join("abcd-1111/ldsu1h.jsonl"),
            concat!(
                r#"{"sessionId":"ldsu1h","projectHash":"alpha","startTime":"2026-05-03T08:01:00.000Z","kind":"subagent"}"#, "\n",
                r#"{"id":"c1","timestamp":"2026-05-03T08:01:40.000Z","type":"gemini","model":"gemini-3.1-pro-preview","content":"","tokens":{"input":900,"output":10,"cached":100,"thoughts":5,"tool":0,"total":915}}"#, "\n",
            ),
        );
        // The older whole-file shape also appears nested, tagged `subagent`.
        write(
            &chats.join("abcd-1111/e8fuq2.json"),
            r#"{"sessionId":"e8fuq2","kind":"subagent","projectHash":"alpha","messages":[
                 {"id":"c2","timestamp":"2026-05-03T08:02:10.000Z","type":"gemini",
                  "model":"gemini-3.1-pro-preview",
                  "tokens":{"input":500,"output":8,"cached":0,"thoughts":2,
                             "tool":0,"total":510}}]}"#,
        );

        let mut conn = crate::db::open_db(&dir.path().join("t.db")).unwrap();
        let r = scan_gemini(&mut conn, &tmp_root, &projects_json);
        assert!(r.error.is_none(), "{:?}", r.error);
        assert_eq!(r.events_inserted, 4, "2 parent + 1 nested jsonl + 1 nested json");

        // The child's own sessionId is its identity, so it cannot collide with
        // the parent's Records, and it inherits the parent's Project.
        let (sid, project): (Option<String>, String) = conn
            .query_row(
                "SELECT session_id, project FROM events WHERE dedup_key = 'gemini:ldsu1h:c1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(sid, Some("ldsu1h".to_string()));
        assert_eq!(project, "/Users/dev/projects/alpha");
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM events WHERE dedup_key = 'gemini:e8fuq2:c2'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1,
            "the nested whole-file shape is missed today too"
        );
    }
}
