use super::unchanged;
use crate::db::{replace_file_events, set_file_state};
use crate::time::iso_to_epoch;
use crate::types::{FileState, SourceScanResult, UsageEvent};
use rusqlite::Connection;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

const MALFORMED_ARTIFACT_WARNING: &str = "gemini: malformed or unsupported chat Artifact";

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
        let project_dir = tmp_root
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let project = resolve_project(project_dir, &reverse);
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
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            if name.starts_with("session-") && name.ends_with(".json") {
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
    let session: SessionFile = match serde_json::from_str(&content) {
        Ok(s) => s,
        Err(_) => {
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

        // Unstamped: the state still describes the pre-rename file, so the next
        // Scan re-reads it instead of treating the empty read as settled. This
        // is the whole proof that the retry is real — `unchanged()` keys on
        // size and mtime, and the stored pair no longer matches the file.
        let state = crate::db::get_file_state(&conn, &path_str).unwrap().unwrap();
        assert_eq!(state.size, SESSION_ALPHA.len() as i64);
        assert_ne!(state.size, moved.len() as i64, "empty parse must not be marked scanned");
    }
}
