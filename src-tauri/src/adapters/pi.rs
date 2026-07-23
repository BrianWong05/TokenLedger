use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::{file_state_of, find_jsonl, rollup_worktree, unchanged};
use crate::db::{insert_events, set_file_state};
use crate::time::iso_to_epoch;
use crate::types::{CtxTokens, FileState, SourceScanResult, UsageEvent};

const PI_PARSER_VERSION: i64 = 1;

#[derive(Deserialize)]
#[serde(tag = "type")]
enum SessionLine {
    #[serde(rename = "session")]
    Session {
        #[serde(default, deserialize_with = "optional_string")]
        id: Option<String>,
        #[serde(default, deserialize_with = "optional_string")]
        cwd: Option<String>,
    },
    #[serde(rename = "message")]
    Message {
        #[serde(default, deserialize_with = "optional_string")]
        id: Option<String>,
        #[serde(default, deserialize_with = "optional_string")]
        timestamp: Option<String>,
        message: MessageFields,
    },
    #[serde(other)]
    Other,
}

// Deliberately excludes content, thinking, images, tool arguments, errors, and
// tool-result details: serde discards them before a Usage Record can be built.
#[derive(Deserialize)]
struct MessageFields {
    role: String,
    timestamp: Option<i64>,
    #[serde(default, deserialize_with = "optional_string")]
    model: Option<String>,
    #[serde(
        default,
        rename = "responseModel",
        deserialize_with = "optional_string"
    )]
    response_model: Option<String>,
    usage: Option<UsageFields>,
}

#[derive(Deserialize)]
struct UsageFields {
    #[serde(default)]
    input: i64,
    #[serde(default)]
    output: i64,
    #[serde(default, rename = "cacheRead")]
    cache_read: i64,
    #[serde(default, rename = "cacheWrite")]
    cache_write: i64,
    #[serde(rename = "cacheWrite1h")]
    cache_write_1h: Option<i64>,
    reasoning: Option<i64>,
}

struct ParsedPiFile {
    events: Vec<UsageEvent>,
    lines_skipped: u64,
}

fn optional_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value.as_str().map(str::to_owned))
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
}

pub fn scan_pi(conn: &mut rusqlite::Connection, session_roots: &[PathBuf]) -> SourceScanResult {
    let mut roots_seen = HashSet::new();
    let mut files_seen = HashSet::new();
    let mut files = Vec::new();
    for root in session_roots {
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        if !roots_seen.insert(root.clone()) {
            continue;
        }
        let mut discovered = Vec::new();
        find_jsonl(&root, &mut discovered);
        for path in discovered {
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            if files_seen.insert(path.clone()) {
                files.push(path);
            }
        }
    }
    files.sort();

    let mut result = SourceScanResult::default();
    for path in files {
        match scan_file(conn, &path) {
            Ok((inserted, skipped)) => {
                result.events_inserted += inserted;
                result.lines_skipped += skipped;
            }
            Err(error) => match result.error.as_mut() {
                Some(previous) => {
                    previous.push_str("; ");
                    previous.push_str(&error);
                }
                None => result.error = Some(error),
            },
        }
    }
    result
}

fn scan_file(conn: &mut rusqlite::Connection, path: &Path) -> Result<(u64, u64), String> {
    let state = FileState {
        byte_offset: PI_PARSER_VERSION,
        ..file_state_of(path)
    };
    if unchanged(conn, path, &state) {
        return Ok((0, 0));
    }

    let source_file = path.to_string_lossy().to_string();
    let content = std::fs::read_to_string(path)
        .map_err(|error| format!("pi: read {}: {error}", path.display()))?;
    let parsed = parse_file(&content, &source_file);
    let inserted = insert_events(conn, &parsed.events)
        .map_err(|error| format!("pi: insert {}: {error}", path.display()))?;
    set_file_state(conn, &source_file, state)
        .map_err(|error| format!("pi: metadata {}: {error}", path.display()))?;
    Ok((inserted, parsed.lines_skipped))
}

fn parse_file(content: &str, source_file: &str) -> ParsedPiFile {
    let mut events = Vec::new();
    let mut lines_skipped = 0;
    let mut session_id: Option<String> = None;
    let mut project: Option<String> = None;

    for complete_record in content.split_inclusive('\n') {
        if !complete_record.ends_with('\n') {
            continue; // pi may still be writing the final record
        }
        let raw = complete_record
            .strip_suffix('\n')
            .unwrap_or(complete_record);
        if raw.trim().is_empty() {
            continue;
        }
        let line: SessionLine = match serde_json::from_str(raw) {
            Ok(line) => line,
            Err(_) => {
                lines_skipped += 1;
                continue;
            }
        };
        match line {
            SessionLine::Session { id, cwd } => {
                session_id = nonempty(id);
                project = cwd
                    .filter(|cwd| Path::new(cwd).is_absolute())
                    .map(|cwd| rollup_worktree(&cwd));
            }
            SessionLine::Message {
                id,
                timestamp,
                message,
            } if message.role == "assistant" => {
                let Some(usage) = message.usage else {
                    continue;
                };
                let input = usage.input.max(0);
                let output = usage.output.max(0);
                let cache_read = usage.cache_read.max(0);
                let cache_write = usage.cache_write.max(0);
                let cache_write_1h = usage.cache_write_1h.unwrap_or(0).clamp(0, cache_write);
                let cache_write_5m = cache_write - cache_write_1h;
                if input == 0 && output == 0 && cache_read == 0 && cache_write == 0 {
                    lines_skipped += 1;
                    continue;
                }
                let message_timestamp = message.timestamp;
                let entry_timestamp = nonempty(timestamp);
                let event_timestamp = message_timestamp
                    .map(|millis| millis / 1000)
                    .or_else(|| entry_timestamp.as_deref().and_then(iso_to_epoch));
                let Some(event_timestamp) = event_timestamp else {
                    lines_skipped += 1;
                    continue;
                };
                let model = nonempty(message.response_model).or_else(|| nonempty(message.model));
                let id = nonempty(id);
                let dedup_key = match (id.as_deref(), entry_timestamp.as_deref()) {
                    (Some(id), Some(timestamp)) => format!("pi:message:{id}:{timestamp}"),
                    _ => {
                        let stable_fields = serde_json::to_vec(&(
                            message_timestamp.unwrap_or(event_timestamp * 1000),
                            entry_timestamp.as_deref(),
                            model.as_deref(),
                            input,
                            output,
                            cache_read,
                            cache_write_5m,
                            cache_write_1h,
                            usage.reasoning,
                        ))
                        .expect("legacy pi identity fields always serialize");
                        let digest = Sha256::digest(stable_fields);
                        format!("pi:legacy:message:{digest:x}")
                    }
                };
                events.push(UsageEvent {
                    dedup_key,
                    source: "pi".to_string(),
                    timestamp: event_timestamp,
                    model,
                    project: project.clone(),
                    api_calls: 1,
                    input_tokens: input,
                    output_tokens: output,
                    cache_read_tokens: cache_read,
                    cache_write_5m_tokens: cache_write_5m,
                    cache_write_1h_tokens: cache_write_1h,
                    source_file: source_file.to_string(),
                    session_id: session_id.clone(),
                    reasoning_tokens: usage.reasoning.map(|r| r.clamp(0, output)),
                    ctx: CtxTokens::default(),
                });
            }
            _ => {}
        }
    }

    ParsedPiFile {
        events,
        lines_skipped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{get_file_state, open_db, prune_missing_files, set_file_state};
    use crate::queries::{self, Filters};
    use std::fs;
    use std::io::Write;

    const BASIC_SESSION: &str = include_str!("fixtures/pi/basic-session.jsonl");

    fn assistant(entry_fields: &str, model: &str, timestamp_ms: i64, input: i64) -> String {
        format!(
            r#"{{"type":"message"{entry_fields},"message":{{"role":"assistant","model":"{model}","usage":{{"input":{input},"output":1,"cacheRead":0,"cacheWrite":0}},"timestamp":{timestamp_ms}}}}}"#,
        )
    }

    fn scan_root(conn: &mut rusqlite::Connection, root: &Path) -> SourceScanResult {
        scan_pi(conn, &[root.to_path_buf()])
    }

    fn write_session(root: &Path, name: &str, session_id: &str, model: &str, input: i64) {
        fs::create_dir_all(root).unwrap();
        let header = format!(
            r#"{{"type":"session","version":3,"id":"{session_id}","cwd":"/projects/{session_id}"}}"#,
        );
        let entry_fields =
            format!(r#","id":"{session_id}-assistant","timestamp":"2026-07-01T00:00:01.000Z""#,);
        let usage = assistant(&entry_fields, model, 1_782_864_001_000 + input, input);
        let content = format!("{header}\n{usage}\n");
        fs::write(root.join(name), content).unwrap();
    }

    #[test]
    fn scans_distinct_roots_once_and_treats_missing_roots_as_empty() {
        let corpus = tempfile::tempdir().unwrap();
        let app = tempfile::tempdir().unwrap();
        let standard = corpus.path().join("standard");
        let session_override = corpus.path().join("session-override");
        let agent_override = corpus.path().join("agent-override/sessions");
        write_session(&standard, "standard.jsonl", "standard", "standard-model", 1);
        write_session(
            &session_override,
            "session.jsonl",
            "session-override",
            "session-model",
            2,
        );
        write_session(
            &agent_override,
            "agent.jsonl",
            "agent-override",
            "agent-model",
            3,
        );
        fs::OpenOptions::new()
            .append(true)
            .open(standard.join("standard.jsonl"))
            .unwrap()
            .write_all(b"{malformed complete record\n")
            .unwrap();

        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        let result = scan_pi(
            &mut conn,
            &[
                standard.clone(),
                session_override,
                agent_override,
                standard.join("."),
                corpus.path().join("missing"),
            ],
        );
        assert_eq!(result.events_inserted, 3);
        assert_eq!(result.lines_skipped, 1, "equivalent roots are visited once");
        assert!(result.error.is_none());
        assert_eq!(
            queries::summary(&conn, &Filters::default())
                .unwrap()
                .requests,
            3
        );

        let unchanged = scan_pi(&mut conn, &[standard]);
        assert_eq!(unchanged.events_inserted, 0);
        assert_eq!(unchanged.lines_skipped, 0);
    }

    #[test]
    fn continues_after_an_unreadable_pi_file() {
        let sessions = tempfile::tempdir().unwrap();
        let app = tempfile::tempdir().unwrap();
        write_session(sessions.path(), "a-valid.jsonl", "valid-a", "model-a", 1);
        fs::write(sessions.path().join("b-broken.jsonl"), [0xff, b'\n']).unwrap();
        write_session(sessions.path(), "c-valid.jsonl", "valid-c", "model-c", 2);

        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        let result = scan_pi(&mut conn, &[sessions.path().to_path_buf()]);
        assert_eq!(result.events_inserted, 2);
        assert!(result.error.as_deref().is_some_and(|error| {
            error.contains("b-broken.jsonl") && error.contains("pi: read")
        }));
        assert_eq!(
            queries::summary(&conn, &Filters::default())
                .unwrap()
                .requests,
            2
        );
    }

    #[test]
    fn incrementally_reparses_changed_files_and_never_prunes_ledger_usage() {
        let sessions = tempfile::tempdir().unwrap();
        let app = tempfile::tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        let path = sessions.path().join("growing.jsonl");
        let first_content = format!(
            "{}\n{{malformed complete record\n{}\n",
            r#"{"type":"session","version":3,"id":"growing-session","cwd":"/projects/growing"}"#,
            assistant(
                r#","id":"first","timestamp":"2026-07-01T00:00:01.000Z""#,
                "growing-model",
                1_782_864_001_000,
                1,
            ),
        );
        fs::write(&path, first_content).unwrap();
        let source_file = fs::canonicalize(&path)
            .unwrap()
            .to_string_lossy()
            .to_string();

        let first = scan_root(&mut conn, sessions.path());
        assert_eq!(first.events_inserted, 1);
        assert_eq!(first.lines_skipped, 1);
        let first_state = get_file_state(&conn, &source_file).unwrap().unwrap();
        assert_eq!(first_state.byte_offset, PI_PARSER_VERSION);

        let unchanged = scan_root(&mut conn, sessions.path());
        assert_eq!(unchanged.events_inserted, 0);
        assert_eq!(
            unchanged.lines_skipped, 0,
            "unchanged file was not reparsed"
        );

        let appended = assistant(
            r#","id":"second","timestamp":"2026-07-01T00:00:02.000Z""#,
            "growing-model",
            1_782_864_002_000,
            2,
        );
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(format!("{appended}\n").as_bytes())
            .unwrap();

        let changed = scan_root(&mut conn, sessions.path());
        assert_eq!(changed.events_inserted, 1, "only appended usage is new");
        assert_eq!(
            changed.lines_skipped, 1,
            "changed file reparses from byte zero"
        );
        assert_eq!(
            queries::summary(&conn, &Filters::default())
                .unwrap()
                .requests,
            2
        );

        let current = get_file_state(&conn, &source_file).unwrap().unwrap();
        set_file_state(
            &conn,
            &source_file,
            crate::types::FileState {
                byte_offset: PI_PARSER_VERSION - 1,
                ..current
            },
        )
        .unwrap();
        let parser_changed = scan_root(&mut conn, sessions.path());
        assert_eq!(parser_changed.events_inserted, 0);
        assert_eq!(
            parser_changed.lines_skipped, 1,
            "parser version invalidates metadata"
        );
        assert_eq!(
            get_file_state(&conn, &source_file)
                .unwrap()
                .unwrap()
                .byte_offset,
            PI_PARSER_VERSION,
        );

        fs::remove_file(&path).unwrap();
        let missing = scan_root(&mut conn, sessions.path());
        assert_eq!(missing.events_inserted, 0);
        assert!(missing.error.is_none());
        assert_eq!(prune_missing_files(&conn).unwrap(), 1);
        assert!(get_file_state(&conn, &source_file).unwrap().is_none());
        assert_eq!(
            queries::summary(&conn, &Filters::default())
                .unwrap()
                .requests,
            2,
            "source disappearance never deletes Ledger Usage Records",
        );
    }

    #[test]
    fn scans_supported_and_legacy_records_while_isolating_complete_damage_and_live_tail() {
        let sessions = tempfile::tempdir().unwrap();
        let app = tempfile::tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();

        let v1 = format!(
            "{}\n{}\n",
            r#"{"type":"session","version":1,"id":"session-v1","cwd":"/projects/v1"}"#,
            assistant(
                r#","timestamp":"2026-07-01T00:00:01.000Z""#,
                "legacy-model",
                1_782_864_001_000,
                1,
            ),
        );
        fs::write(sessions.path().join("v1.jsonl"), &v1).unwrap();
        let legacy_copy = v1.replace(
            r#""id":"session-v1","cwd":"/projects/v1""#,
            r#""id":"copied-v1","cwd":"/projects/copied""#,
        );
        fs::write(sessions.path().join("z-copied-v1.jsonl"), legacy_copy).unwrap();

        let v2 = format!(
            "{}\n{}\n{}\n",
            r#"{"type":"session","version":2,"id":"session-v2","cwd":"/projects/v2","futureHeaderField":true}"#,
            r#"{"type":"future_entry","id":"ignored","future":{"private":"discarded"}}"#,
            assistant(
                r#","id":"v2-assistant","parentId":null,"timestamp":"2026-07-01T00:00:02.000Z","futureEntryField":true"#,
                "v2-model",
                1_782_864_002_000,
                2,
            ),
        );
        fs::write(sessions.path().join("v2.jsonl"), v2).unwrap();

        let v3 = format!(
            "{}\n{{malformed complete record\n{}\n{}",
            r#"{"type":"session","version":3,"id":"session-v3","cwd":"/projects/v3"}"#,
            assistant(
                r#","id":"v3-assistant","parentId":null,"timestamp":"2026-07-01T00:00:03.000Z""#,
                "v3-model",
                1_782_864_003_000,
                3,
            ),
            assistant(
                r#","id":"live-tail","parentId":"v3-assistant","timestamp":"2026-07-01T00:00:04.000Z""#,
                "live-tail-model",
                1_782_864_004_000,
                4,
            ),
        );
        fs::write(sessions.path().join("v3.jsonl"), v3).unwrap();

        let damaged_session = format!(
            "{}\n{}\n",
            r#"{"type":"session","version":3,"id":17,"cwd":"/projects/intact"}"#,
            assistant(
                r#","id":"damaged-session","timestamp":"2026-07-01T00:00:05.000Z""#,
                "damaged-session-model",
                1_782_864_005_000,
                5,
            ),
        );
        fs::write(
            sessions.path().join("damaged-session.jsonl"),
            damaged_session,
        )
        .unwrap();
        let damaged_project = format!(
            "{}\n{}\n",
            r#"{"type":"session","version":3,"id":"intact-session","cwd":["not","a","path"]}"#,
            assistant(
                r#","id":"damaged-project","timestamp":"2026-07-01T00:00:06.000Z""#,
                "damaged-project-model",
                1_782_864_006_000,
                6,
            ),
        );
        fs::write(
            sessions.path().join("damaged-project.jsonl"),
            damaged_project,
        )
        .unwrap();

        let result = scan_root(&mut conn, sessions.path());
        assert_eq!(
            result.events_inserted, 5,
            "copied legacy usage has stable file-independent identity",
        );
        assert_eq!(
            result.lines_skipped, 1,
            "only complete malformed records count"
        );
        assert!(result.error.is_none());

        let summary = queries::summary(&conn, &Filters::default()).unwrap();
        assert_eq!(summary.requests, 5);
        let sessions: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT session_id) FROM events WHERE source = 'pi'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sessions, 4, "only damaged Session identity is unassociated");

        let models = queries::breakdown(&conn, "model", &Filters::default()).unwrap();
        let keys: Vec<_> = models.iter().filter_map(|row| row.key.as_deref()).collect();
        assert!(keys.contains(&"legacy-model"));
        assert!(keys.contains(&"v2-model"));
        assert!(keys.contains(&"v3-model"));
        assert!(keys.contains(&"damaged-session-model"));
        assert!(keys.contains(&"damaged-project-model"));
        assert!(!keys.contains(&"live-tail-model"));

        let damaged_session_associations: (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT session_id, project FROM events WHERE model = 'damaged-session-model'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            damaged_session_associations,
            (None, Some("/projects/intact".to_string())),
        );
        let damaged_project_associations: (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT session_id, project FROM events WHERE model = 'damaged-project-model'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            damaged_project_associations,
            (Some("intact-session".to_string()), None),
        );
    }

    #[test]
    fn falls_back_to_entry_time_and_rejects_relative_project_identity() {
        let session = concat!(
            "{\"type\":\"session\",\"version\":3,\"id\":\"s\",\"timestamp\":\"2026-07-01T12:00:00.000Z\",\"cwd\":\"relative/project\"}\n",
            "{\"type\":\"message\",\"id\":\"a\",\"parentId\":null,\"timestamp\":\"2026-07-01T12:00:07.000Z\",\"message\":{\"role\":\"assistant\",\"model\":\"m\",\"usage\":{\"input\":1,\"output\":0,\"cacheRead\":0,\"cacheWrite\":0},\"stopReason\":\"stop\"}}\n",
        );
        let parsed = parse_file(session, "fixture.jsonl");
        assert_eq!(parsed.events.len(), 1);
        assert_eq!(parsed.events[0].timestamp, 1_782_907_207);
        assert_eq!(parsed.events[0].session_id.as_deref(), Some("s"));
        assert_eq!(parsed.events[0].project, None);
    }

    #[test]
    fn parses_nonzero_assistant_usage_without_retaining_private_content_fields() {
        let parsed = parse_file(BASIC_SESSION, "/fixtures/basic-session.jsonl");
        assert_eq!(parsed.events.len(), 3);
        assert_eq!(parsed.lines_skipped, 2, "zero placeholder + malformed line");

        let first = &parsed.events[0];
        assert_eq!(
            first.dedup_key,
            "pi:message:a1b2c3d4:2026-07-01T12:00:02.000Z"
        );
        assert_eq!(first.source, "pi");
        assert_eq!(first.timestamp, 1_782_907_202, "message timestamp wins");
        assert_eq!(first.model.as_deref(), Some("pi-response-model"));
        assert_eq!(
            first.project.as_deref(),
            Some("/Users/dev/projects/pi-demo")
        );
        assert_eq!(first.session_id.as_deref(), Some("session-basic-pi"));
        assert_eq!(first.api_calls, 1);
        assert_eq!(first.input_tokens, 100);
        assert_eq!(first.output_tokens, 50);
        assert_eq!(first.cache_read_tokens, 20);
        assert_eq!(first.cache_write_5m_tokens, 10);
        assert_eq!(first.cache_write_1h_tokens, 5);
        assert_eq!(first.reasoning_tokens, Some(10));
        assert_eq!(first.ctx, Default::default());

        let aborted = &parsed.events[1];
        assert_eq!(aborted.model.as_deref(), Some("pi-fallback-model"));
        assert_eq!(aborted.timestamp, 1_782_907_203);
        assert_eq!(
            aborted.reasoning_tokens, None,
            "absent remains not reported"
        );

        let failed = &parsed.events[2];
        assert_eq!(failed.model.as_deref(), Some("pi-error-model"));
        assert_eq!(failed.timestamp, 1_782_907_204);
        assert_eq!(
            failed.reasoning_tokens,
            Some(0),
            "reported zero remains reported"
        );
        assert_eq!(failed.cache_write_5m_tokens, 4);
        assert_eq!(failed.cache_write_1h_tokens, 0);
    }
}
