use crate::db::{get_file_state, replace_file_events, set_file_state};
use crate::time::iso_to_epoch;
use crate::types::{FileState, SourceScanResult, UsageEvent};
use rusqlite::Connection;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

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
}

pub fn scan_gemini(conn: &mut Connection, tmp_root: &Path, projects_json: &Path) -> SourceScanResult {
    let mut result = SourceScanResult::default();
    let reverse = load_reverse_map(projects_json);

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
                process_file(conn, &path, &project, &mut result);
            }
        }
    }
    result
}

fn process_file(conn: &mut Connection, path: &Path, project: &str, result: &mut SourceScanResult) {
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
    if let Ok(Some(state)) = get_file_state(conn, &path_str) {
        if state.size == size && state.mtime == mtime {
            return;
        }
    }

    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => {
            result.lines_skipped += 1;
            return;
        }
    };
    let session: SessionFile = match serde_json::from_str(&content) {
        Ok(s) => s,
        Err(_) => {
            result.lines_skipped += 1;
            return;
        }
    };

    let mut events = Vec::new();
    for m in &session.messages {
        let tokens = match &m.tokens {
            Some(t) => t,
            None => continue, // non-token messages contribute nothing
        };
        let ts = match iso_to_epoch(&m.timestamp) {
            Some(t) => t,
            None => {
                result.lines_skipped += 1;
                continue;
            }
        };
        events.push(UsageEvent {
            dedup_key: format!("gemini:{}:{}", session.session_id, m.id),
            source: "gemini".to_string(),
            timestamp: ts,
            model: m.model.clone().unwrap_or_else(|| "unknown".to_string()),
            project: Some(project.to_string()),
            api_calls: 1,
            input_tokens: (tokens.input - tokens.cached).max(0),
            output_tokens: tokens.output + tokens.thoughts,
            cache_read_tokens: tokens.cached,
            cache_write_5m_tokens: 0,
            cache_write_1h_tokens: 0,
            source_file: path_str.clone(),
            session_id: Some(session.session_id.clone()),
            reasoning_tokens: Some(tokens.thoughts),
            ctx: Default::default(),
        });
    }

    let n = events.len() as u64;
    if replace_file_events(conn, &path_str, &events).is_err() {
        result.error = Some(format!("failed to write events for {}", path_str));
        return;
    }
    result.events_inserted += n;
    let _ = set_file_state(conn, &path_str, FileState { size, mtime, byte_offset: 0 });
}

/// projects.json is `{"projects": {realPath: friendlyName}}`; build friendly → real.
fn load_reverse_map(projects_json: &Path) -> HashMap<String, String> {
    #[derive(Deserialize)]
    struct Projects {
        projects: HashMap<String, String>,
    }
    let mut map = HashMap::new();
    if let Ok(content) = fs::read_to_string(projects_json) {
        if let Ok(p) = serde_json::from_str::<Projects>(&content) {
            for (real, friendly) in p.projects {
                map.insert(friendly, real);
            }
        }
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
          "tokens": { "input": 500, "output": 100, "cached": 0, "thoughts": 0, "tool": 0, "total": 600 } }
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
        assert!(r.error.is_none());

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
}
