use serde::Deserialize;
use std::path::Path;

use super::{find_jsonl, rollup_worktree};
use crate::db::insert_events;
use crate::time::iso_to_epoch;
use crate::types::{CtxTokens, SourceScanResult, UsageEvent};

#[derive(Deserialize)]
#[serde(tag = "type")]
enum SessionLine {
    #[serde(rename = "session")]
    Session {
        id: Option<String>,
        cwd: Option<String>,
    },
    #[serde(rename = "message")]
    Message {
        id: String,
        timestamp: String,
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
    model: Option<String>,
    #[serde(rename = "responseModel")]
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

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
}

pub fn scan_pi(conn: &mut rusqlite::Connection, sessions_root: &Path) -> SourceScanResult {
    let mut files = Vec::new();
    find_jsonl(sessions_root, &mut files);
    files.sort();

    let mut result = SourceScanResult::default();
    for path in files {
        let source_file = path.to_string_lossy().to_string();
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) => {
                result.error = Some(format!("pi: read {}: {e}", path.display()));
                return result;
            }
        };
        let parsed = parse_file(&content, &source_file);
        result.lines_skipped += parsed.lines_skipped;
        match insert_events(conn, &parsed.events) {
            Ok(inserted) => result.events_inserted += inserted,
            Err(e) => {
                result.error = Some(format!("pi: insert {}: {e}", path.display()));
                return result;
            }
        }
    }
    result
}

fn parse_file(content: &str, source_file: &str) -> ParsedPiFile {
    let mut events = Vec::new();
    let mut lines_skipped = 0;
    let mut session_id: Option<String> = None;
    let mut project: Option<String> = None;

    for raw in content.lines() {
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
                if input + output + cache_read + cache_write == 0 {
                    lines_skipped += 1;
                    continue;
                }
                let event_timestamp = message
                    .timestamp
                    .map(|millis| millis / 1000)
                    .or_else(|| iso_to_epoch(&timestamp));
                let Some(event_timestamp) = event_timestamp else {
                    lines_skipped += 1;
                    continue;
                };
                let model = nonempty(message.response_model).or_else(|| nonempty(message.model));
                events.push(UsageEvent {
                    dedup_key: format!("pi:message:{id}:{timestamp}"),
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

    const BASIC_SESSION: &str = include_str!("fixtures/pi/basic-session.jsonl");

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
