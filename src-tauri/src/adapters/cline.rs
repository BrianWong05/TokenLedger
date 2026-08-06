use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::Value;

use crate::adapters::file_state_of;
use crate::db::{set_file_state, upsert_events};
use crate::time::iso_to_epoch;
use crate::types::{CtxTokens, FileState, SourceScanResult, UsageEvent};

const SOURCE: &str = "cline";
const PARSER_VERSION: i64 = 1;
const TASK_MESSAGE_FILES: &[&str] = &["ui_messages.json", "claude_messages.json"];
const REQUEST_ID_KEYS: &[&str] = &[
    "requestId",
    "request_id",
    "messageId",
    "message_id",
    "turnId",
    "turn_id",
    "id",
];

#[derive(Default)]
struct ParsedArtifact {
    events: Vec<UsageEvent>,
    lines_skipped: u64,
    malformed: bool,
}

#[derive(Clone, Default)]
struct Metadata {
    model: Option<String>,
    project: Option<String>,
}

#[derive(Clone, Default)]
struct TokenCounts {
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
}

impl TokenCounts {
    fn is_zero(&self) -> bool {
        self.input == 0 && self.output == 0 && self.cache_read == 0 && self.cache_write == 0
    }
}

#[derive(Clone, Default)]
struct UsageSnapshot {
    timestamp: i64,
    tokens: TokenCounts,
    model: Option<String>,
    project: Option<String>,
}

#[derive(Clone, Default)]
struct SessionRecord {
    request_id: Option<String>,
    snapshot: UsageSnapshot,
}

/// Scan Cline's passive local task and session artifacts.
///
/// The editor and CLI write different containers around the same request-level
/// usage facts. We only retain normalized counters and explicit metadata; task
/// prompts, responses, tool arguments, and other conversation content never
/// cross this boundary into the Ledger.
pub fn scan_cline(conn: &mut Connection, roots: &[PathBuf]) -> SourceScanResult {
    let mut files = Vec::new();
    let mut errors = Vec::new();
    for root in roots {
        discover_root(root, &mut files, &mut errors);
    }

    files.sort();
    files.dedup();

    let mut events = Vec::new();
    let mut event_indices = HashMap::new();
    let mut lines_skipped = 0;

    // Cline can persist one request in both editor and CLI artifacts. Re-read
    // every discovered file so a changed lower-priority artifact cannot
    // overwrite a higher-priority sibling merely because that sibling was
    // unchanged in this scan. FileState still records the parser version and
    // source metadata for diagnostics and future migrations.
    for path in files {
        let path_state = FileState {
            byte_offset: PARSER_VERSION,
            ..file_state_of(&path)
        };
        match parse_artifact(&path) {
            Ok(parsed) => {
                lines_skipped += parsed.lines_skipped;
                if parsed.malformed {
                    errors.push("cline: malformed usage message".to_string());
                }
                for event in parsed.events {
                    if let Some(index) = event_indices.get(&event.dedup_key).copied() {
                        merge_duplicate_event(&mut events[index], event);
                    } else {
                        event_indices.insert(event.dedup_key.clone(), events.len());
                        events.push(event);
                    }
                }
                if let Err(error) = set_file_state(conn, &path.to_string_lossy(), path_state) {
                    errors.push(format!("cline: Ledger file-state update failed: {error}"));
                }
            }
            Err(error) => errors.push(error),
        }
    }

    let events_inserted = if events.is_empty() {
        0
    } else {
        let before: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE source = ?1",
                [SOURCE],
                |row| row.get(0),
            )
            .unwrap_or(0);
        match upsert_events(conn, &events) {
            Ok(()) => {
                let after: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM events WHERE source = ?1",
                        [SOURCE],
                        |row| row.get(0),
                    )
                    .unwrap_or(before);
                (after - before).max(0) as u64
            }
            Err(error) => {
                errors.push(format!("cline: Ledger insert failed: {error}"));
                0
            }
        }
    };

    SourceScanResult {
        events_inserted,
        lines_skipped,
        error: (!errors.is_empty()).then(|| errors.join("; ")),
    }
}

fn merge_duplicate_event(existing: &mut UsageEvent, duplicate: UsageEvent) {
    let duplicate_wins = cline_artifact_priority(&duplicate.source_file)
        > cline_artifact_priority(&existing.source_file);
    let mut winner = if duplicate_wins {
        duplicate.clone()
    } else {
        existing.clone()
    };
    let fallback: &UsageEvent = if duplicate_wins {
        &*existing
    } else {
        &duplicate
    };
    if winner.model.is_none() {
        winner.model = fallback.model.clone();
    }
    if winner.project.is_none() {
        winner.project = fallback.project.clone();
    }
    if winner.session_id.is_none() {
        winner.session_id = fallback.session_id.clone();
    }
    *existing = winner;
}

fn cline_artifact_priority(source_file: &str) -> u8 {
    match Path::new(source_file)
        .file_name()
        .and_then(|name| name.to_str())
    {
        Some("claude_messages.json") => 0,
        Some("ui_messages.json") => 2,
        _ => 3,
    }
}

fn discover_root(root: &Path, files: &mut Vec<PathBuf>, errors: &mut Vec<String>) {
    if root.is_file() {
        let is_task_file = root
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| TASK_MESSAGE_FILES.contains(&name));
        let is_session_file = root.extension().and_then(|ext| ext.to_str()) == Some("json");
        if is_task_file || is_session_file {
            push_unique(files, root);
        }
        return;
    }
    if !root.is_dir() {
        return;
    }

    let name = root.file_name().and_then(|name| name.to_str());
    if name == Some("tasks") {
        walk_task_files(root, files, errors);
        return;
    }
    if name == Some("sessions") {
        walk_session_files(root, files, errors);
        return;
    }

    find_named_roots(root, "tasks", files, errors);
    find_named_roots(root, "sessions", files, errors);
}

fn find_named_roots(root: &Path, name: &str, files: &mut Vec<PathBuf>, errors: &mut Vec<String>) {
    if root.file_name().and_then(|value| value.to_str()) == Some(name) {
        if name == "tasks" {
            walk_task_files(root, files, errors);
        } else {
            walk_session_files(root, files, errors);
        }
        return;
    }

    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let mut children: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    children.sort();
    for child in children {
        if child.is_dir() {
            find_named_roots(&child, name, files, errors);
        }
    }
}

fn walk_task_files(root: &Path, files: &mut Vec<PathBuf>, errors: &mut Vec<String>) {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => {
            errors.push("cline: could not read task storage".to_string());
            return;
        }
    };
    let mut children: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    children.sort();
    for child in children {
        if child.is_dir() {
            walk_task_files(&child, files, errors);
        } else if child
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| TASK_MESSAGE_FILES.contains(&name))
        {
            push_unique(files, &child);
        }
    }
}

fn walk_session_files(root: &Path, files: &mut Vec<PathBuf>, errors: &mut Vec<String>) {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => {
            errors.push("cline: could not read CLI session storage".to_string());
            return;
        }
    };
    let mut children: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    children.sort();
    for child in children {
        if child.is_dir() {
            walk_session_files(&child, files, errors);
        } else if child.extension().and_then(|ext| ext.to_str()) == Some("json") {
            push_unique(files, &child);
        }
    }
}

fn push_unique(files: &mut Vec<PathBuf>, path: &Path) {
    let normalized = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !files.iter().any(|existing| existing == &normalized) {
        files.push(normalized);
    }
}

fn parse_artifact(path: &Path) -> Result<ParsedArtifact, String> {
    let content = fs::read_to_string(path)
        .map_err(|_| "cline: Source Artifact could not be read".to_string())?;
    let value: Value = serde_json::from_str(&content)
        .map_err(|_| "cline: malformed JSON Source Artifact".to_string())?;

    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if TASK_MESSAGE_FILES.contains(&filename) {
        let messages = value
            .as_array()
            .ok_or_else(|| "cline: unsupported editor task schema".to_string())?;
        let history = path
            .parent()
            .map(|parent| parent.join("api_conversation_history.json"));
        return Ok(parse_message_array(
            messages,
            &task_id_for(path),
            path,
            history.as_deref(),
            Metadata::default(),
        ));
    }

    parse_cli_session(&value, path)
}

fn parse_message_array(
    messages: &[Value],
    session_id: &str,
    source_file: &Path,
    history: Option<&Path>,
    hints: Metadata,
) -> ParsedArtifact {
    let history_metadata = history.map(metadata_from_history).unwrap_or_default();
    let mut started: Vec<SessionRecord> = Vec::new();
    let mut finished: Vec<SessionRecord> = Vec::new();
    let mut lines_skipped = 0;
    let mut malformed = false;

    for message in messages {
        let say = message.get("say").and_then(Value::as_str);
        if !matches!(say, Some("api_req_started" | "api_req_finished")) {
            continue;
        }

        let Some(text) = message.get("text").and_then(Value::as_str) else {
            malformed = true;
            lines_skipped += 1;
            continue;
        };
        let Ok(payload) = serde_json::from_str::<Value>(text) else {
            malformed = true;
            lines_skipped += 1;
            continue;
        };
        let tokens = match token_counts_from_value(&payload) {
            Ok(Some(tokens)) => tokens,
            Ok(None) => continue,
            Err(_) => {
                malformed = true;
                lines_skipped += 1;
                continue;
            }
        };
        if tokens.is_zero() {
            continue;
        }
        let Some(timestamp) =
            timestamp_from_value(message.get("ts").or_else(|| message.get("timestamp")))
        else {
            malformed = true;
            lines_skipped += 1;
            continue;
        };

        let snapshot = UsageSnapshot {
            timestamp,
            tokens,
            model: explicit_model(message),
            project: explicit_project(message),
        };
        let record = SessionRecord {
            request_id: request_identity(message, Some(&payload)),
            snapshot,
        };
        match say {
            Some("api_req_started") => started.push(record),
            Some("api_req_finished") => finished.push(record),
            _ => {}
        }
    }

    let model_hint = unique_string([hints.model, history_metadata.model].into_iter().flatten());
    let project_hint = unique_string(
        [hints.project, history_metadata.project]
            .into_iter()
            .flatten(),
    );
    let mut events = Vec::new();
    let request_count = started.len().max(finished.len());
    for index in 0..request_count {
        let record = started
            .get(index)
            .filter(|record| !record.snapshot.tokens.is_zero())
            .cloned()
            .or_else(|| finished.get(index).cloned());
        let Some(record) = record else {
            continue;
        };
        let request_key = record
            .request_id
            .clone()
            .unwrap_or_else(|| format!("index:{index}"));
        events.push(snapshot_event(
            format!("{SOURCE}:session:{session_id}:request:{request_key}"),
            session_id,
            source_file,
            record.snapshot,
            model_hint.clone(),
            project_hint.clone(),
        ));
    }

    ParsedArtifact {
        events,
        lines_skipped,
        malformed,
    }
}

fn parse_cli_session(value: &Value, source_file: &Path) -> Result<ParsedArtifact, String> {
    if value
        .get("version")
        .and_then(Value::as_i64)
        .is_some_and(|version| version > PARSER_VERSION)
    {
        return Err("cline: unsupported CLI session version".to_string());
    }

    let session_id = explicit_string(value, &["sessionId", "session_id", "id"])
        .or_else(|| {
            value
                .get("session")
                .and_then(|session| explicit_string(session, &["sessionId", "session_id", "id"]))
        })
        .or_else(|| {
            source_file
                .file_stem()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .ok_or_else(|| "cline: CLI session has no usable identity".to_string())?;
    let hints = Metadata {
        model: explicit_model(value),
        project: explicit_project(value),
    };

    if let Some(messages) = value.as_array() {
        if messages.iter().any(is_message_record) {
            return Ok(parse_message_array(
                messages,
                &session_id,
                source_file,
                None,
                hints,
            ));
        }
    }
    if let Some(messages) = value.get("messages").and_then(Value::as_array) {
        if messages.iter().any(is_message_record) {
            return Ok(parse_message_array(
                messages,
                &session_id,
                source_file,
                None,
                hints,
            ));
        }
    }

    Err("cline: unsupported CLI session schema".to_string())
}

fn is_message_record(value: &Value) -> bool {
    value.get("say").and_then(Value::as_str).is_some()
        && value.get("text").and_then(Value::as_str).is_some()
}

fn token_counts_from_value(value: &Value) -> Result<Option<TokenCounts>, ()> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };

    if !["tokensIn", "tokensOut", "cacheReads", "cacheWrites"]
        .iter()
        .any(|key| object.contains_key(*key))
    {
        return Ok(None);
    }

    Ok(Some(TokenCounts {
        input: first_number(object, &["tokensIn"])?.unwrap_or(0),
        output: first_number(object, &["tokensOut"])?.unwrap_or(0),
        cache_read: first_number(object, &["cacheReads"])?.unwrap_or(0),
        cache_write: first_number(object, &["cacheWrites"])?.unwrap_or(0),
    }))
}

fn first_number(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Result<Option<i64>, ()> {
    for key in keys {
        let Some(value) = object.get(*key) else {
            continue;
        };
        return match value {
            Value::Number(number) => number.as_i64().filter(|value| *value >= 0).ok_or(()),
            _ => Err(()),
        }
        .map(Some);
    }
    Ok(None)
}

fn snapshot_event(
    dedup_key: String,
    session_id: &str,
    source_file: &Path,
    snapshot: UsageSnapshot,
    model_hint: Option<String>,
    project_hint: Option<String>,
) -> UsageEvent {
    UsageEvent {
        dedup_key,
        source: SOURCE.to_string(),
        timestamp: snapshot.timestamp,
        model: snapshot.model.or(model_hint),
        project: snapshot.project.or(project_hint),
        api_calls: 1,
        input_tokens: snapshot.tokens.input,
        output_tokens: snapshot.tokens.output,
        cache_read_tokens: snapshot.tokens.cache_read,
        cache_write_5m_tokens: snapshot.tokens.cache_write,
        cache_write_1h_tokens: 0,
        source_file: source_file.to_string_lossy().into_owned(),
        session_id: Some(session_id.to_string()),
        reasoning_tokens: None,
        ctx: CtxTokens::default(),
    }
}

fn metadata_from_history(path: &Path) -> Metadata {
    let Ok(content) = fs::read_to_string(path) else {
        return Metadata::default();
    };
    let Ok(value) = serde_json::from_str::<Value>(&content) else {
        return Metadata::default();
    };

    let mut strings = Vec::new();
    collect_strings(&value, &mut strings);
    let models = strings
        .iter()
        .flat_map(|text| tagged_values(text, "model"))
        .filter_map(|value| clean_model(&value));
    let projects = strings.iter().flat_map(|text| {
        tagged_values(text, "cwd")
            .into_iter()
            .chain(tagged_values(text, "current_working_directory"))
            .chain(current_working_directory_values(text))
    });

    Metadata {
        model: unique_string(models),
        project: unique_string(projects.filter_map(|value| clean_project(&value))),
    }
}

fn collect_strings(value: &Value, strings: &mut Vec<String>) {
    match value {
        Value::String(value) => strings.push(value.clone()),
        Value::Array(values) => values
            .iter()
            .for_each(|value| collect_strings(value, strings)),
        Value::Object(values) => values
            .values()
            .for_each(|value| collect_strings(value, strings)),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn tagged_values(text: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut values = Vec::new();
    let mut cursor = 0;
    while let Some(start) = text[cursor..].find(&open) {
        let start = cursor + start + open.len();
        let Some(end) = text[start..].find(&close) else {
            break;
        };
        let end = start + end;
        values.push(text[start..end].trim().to_string());
        cursor = end + close.len();
    }
    values
}

fn current_working_directory_values(text: &str) -> Vec<String> {
    let marker = "# Current Working Directory (";
    let mut values = Vec::new();
    let mut cursor = 0;
    while let Some(start) = text[cursor..].find(marker) {
        let start = cursor + start + marker.len();
        let Some(end) = text[start..].find(')') else {
            break;
        };
        let end = start + end;
        values.push(text[start..end].trim().to_string());
        cursor = end + 1;
    }
    values
}

fn explicit_model(value: &Value) -> Option<String> {
    explicit_string(value, &["model", "modelId", "model_id"])
        .or_else(|| {
            value.get("modelInfo").and_then(|model_info| {
                explicit_string(model_info, &["model", "modelId", "model_id", "id", "name"])
            })
        })
        .or_else(|| {
            ["metadata", "session", "config"].iter().find_map(|key| {
                value
                    .get(*key)
                    .and_then(|nested| explicit_string(nested, &["model", "modelId", "model_id"]))
            })
        })
        .and_then(|model| clean_model(&model))
}

fn explicit_project(value: &Value) -> Option<String> {
    [
        "cwd",
        "workingDir",
        "working_dir",
        "currentWorkingDirectory",
        "project",
    ]
    .iter()
    .find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .and_then(clean_project)
    })
    .or_else(|| {
        ["metadata", "session", "workspace"].iter().find_map(|key| {
            value.get(*key).and_then(|nested| {
                [
                    "cwd",
                    "workingDir",
                    "working_dir",
                    "currentWorkingDirectory",
                    "project",
                ]
                .iter()
                .find_map(|field| {
                    nested
                        .get(*field)
                        .and_then(Value::as_str)
                        .and_then(clean_project)
                })
            })
        })
    })
}

fn explicit_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| match value.get(*key) {
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.trim().to_string()),
        Some(Value::Number(value)) => Some(value.to_string()),
        _ => None,
    })
}

fn request_identity(value: &Value, payload: Option<&Value>) -> Option<String> {
    explicit_string(value, REQUEST_ID_KEYS)
        .or_else(|| payload.and_then(|payload| explicit_string(payload, REQUEST_ID_KEYS)))
        .or_else(|| {
            value
                .as_object()
                .and_then(timestamp_from_object)
                .map(|timestamp| format!("ts:{timestamp}"))
        })
}

fn clean_model(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn clean_project(value: &str) -> Option<String> {
    let value = value.trim();
    let is_absolute = value.starts_with('/')
        || value.starts_with(r"\\")
        || (value.as_bytes().get(1) == Some(&b':')
            && value
                .as_bytes()
                .get(2)
                .is_some_and(|byte| *byte == b'/' || *byte == b'\\'));
    (is_absolute && !value.is_empty()).then(|| value.to_string())
}

fn unique_string(values: impl Iterator<Item = String>) -> Option<String> {
    let mut unique = Vec::new();
    for value in values {
        if !unique.contains(&value) {
            unique.push(value);
        }
    }
    (unique.len() == 1).then(|| unique.remove(0))
}

fn timestamp_from_object(object: &serde_json::Map<String, Value>) -> Option<i64> {
    [
        "ts",
        "timestamp",
        "createdAt",
        "created_at",
        "updatedAt",
        "updated_at",
        "time",
    ]
    .iter()
    .find_map(|key| timestamp_from_value(object.get(*key)))
}

fn timestamp_from_value(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(number) => number
            .as_i64()
            .map(normalize_epoch)
            .or_else(|| number.as_f64().map(|value| normalize_epoch(value as i64))),
        Value::String(value) => {
            iso_to_epoch(value).or_else(|| value.parse::<i64>().ok().map(normalize_epoch))
        }
        _ => None,
    }
}

fn normalize_epoch(value: i64) -> i64 {
    if value.abs() >= 1_000_000_000_000_000 {
        value / 1_000_000
    } else if value.abs() >= 1_000_000_000_000 {
        value / 1_000
    } else {
        value
    }
}

fn task_id_for(path: &Path) -> String {
    let components: Vec<String> = path
        .components()
        .filter_map(|component| component.as_os_str().to_str().map(str::to_string))
        .collect();
    if let Some(index) = components
        .iter()
        .rposition(|component| component == "tasks")
    {
        if let Some(task_id) = components.get(index + 1) {
            return task_id.clone();
        }
    }
    path.parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("unknown-task")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_db;
    use std::fs;

    #[test]
    fn scans_editor_task_usage_without_persisting_conversation_content() {
        let dir = tempfile::tempdir().unwrap();
        let task = dir.path().join("tasks/task-editor");
        fs::create_dir_all(&task).unwrap();
        fs::write(
            task.join("claude_messages.json"),
            r#"[
              {"type":"say","say":"api_req_started","ts":1780308000000,
               "text":"{\"request\":\"CLINE_PRIVATE_PROMPT_MARKER\"}"},
              {"type":"say","say":"api_req_finished","ts":1780308001000,
               "text":"{\"tokensIn\":100,\"tokensOut\":20,\"cacheReads\":10,\"cacheWrites\":5}"}
            ]"#,
        )
        .unwrap();
        fs::write(
            task.join("api_conversation_history.json"),
            r#"[{"content":"<model>cline-model</model><cwd>/Users/dev/cline</cwd>"}]"#,
        )
        .unwrap();

        let db_path = dir.path().join("ledger.db");
        let mut conn = open_db(&db_path).unwrap();
        let result = scan_cline(&mut conn, &[dir.path().join("tasks")]);

        assert_eq!(result.events_inserted, 1);
        assert_eq!(result.lines_skipped, 0);
        assert!(result.error.is_none());
        let row: (i64, i64, i64, i64, Option<String>, Option<String>, String) = conn
            .query_row(
                "SELECT input_tokens, output_tokens, cache_read_tokens, cache_write_5m_tokens, model, project, dedup_key FROM events",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)),
            )
            .unwrap();
        assert_eq!(
            row,
            (
                100,
                20,
                10,
                5,
                Some("cline-model".to_string()),
                Some("/Users/dev/cline".to_string()),
                "cline:session:task-editor:request:ts:1780308001".to_string()
            )
        );

        let bytes = fs::read(&db_path).unwrap();
        assert!(!bytes
            .windows("CLINE_PRIVATE_PROMPT_MARKER".len())
            .any(|window| window == b"CLINE_PRIVATE_PROMPT_MARKER"));
    }

    #[test]
    fn cli_sessions_and_editor_tasks_share_identity_and_rescan_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let editor_task = dir.path().join("editor/tasks/shared-session");
        let cli_sessions = dir.path().join("cli/sessions");
        fs::create_dir_all(&editor_task).unwrap();
        fs::create_dir_all(&cli_sessions).unwrap();
        fs::write(
            editor_task.join("ui_messages.json"),
            r#"[{"type":"say","say":"api_req_started","ts":1780308000000,"text":"{\"tokensIn\":100,\"tokensOut\":20}"}]"#,
        )
        .unwrap();
        fs::write(
            cli_sessions.join("shared-session.json"),
            r#"{"id":"shared-session","messages":[{"type":"say","say":"api_req_started","ts":1780308000000,"text":"{\"tokensIn\":200,\"tokensOut\":40}"}]}"#,
        )
        .unwrap();

        let mut conn = open_db(&dir.path().join("ledger.db")).unwrap();
        let first = scan_cline(
            &mut conn,
            &[dir.path().join("editor/tasks"), dir.path().join("cli")],
        );
        assert_eq!(first.events_inserted, 1);
        assert!(first.error.is_none());

        let second = scan_cline(
            &mut conn,
            &[dir.path().join("editor/tasks"), dir.path().join("cli")],
        );
        assert_eq!(second.events_inserted, 0);
        assert!(second.error.is_none());

        fs::write(
            editor_task.join("ui_messages.json"),
            r#"[{"type":"say","say":"api_req_started","ts":1780308000000,"text":"{\"tokensIn\":101,\"tokensOut\":21}"}]"#,
        )
        .unwrap();
        let third = scan_cline(
            &mut conn,
            &[dir.path().join("editor/tasks"), dir.path().join("cli")],
        );
        assert_eq!(third.events_inserted, 0);
        assert!(third.error.is_none());

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE source = 'cline'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        let totals: (i64, i64) = conn
            .query_row(
                "SELECT input_tokens, output_tokens FROM events WHERE source = 'cline'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(totals, (200, 40), "modern CLI snapshot should win overlap");
    }

    #[test]
    fn editor_server_and_cli_usage_reach_queries_pricing_and_unattributed_usage() {
        let dir = tempfile::tempdir().unwrap();
        let editor_task = dir.path().join(
            ".vscode-server/data/User/globalStorage/saoudrizwan.claude-dev/tasks/server-session",
        );
        let cli_sessions = dir.path().join(".cline/data/sessions");
        fs::create_dir_all(&editor_task).unwrap();
        fs::create_dir_all(&cli_sessions).unwrap();
        fs::write(
            editor_task.join("ui_messages.json"),
            r#"[{"type":"say","say":"api_req_started","ts":1780308000000,"text":"{\"tokensIn\":100,\"tokensOut\":10,\"cacheReads\":20,\"cacheWrites\":5}"}]"#,
        )
        .unwrap();
        fs::write(
            editor_task.join("api_conversation_history.json"),
            r#"[{"content":"<model>cline-editor-model</model><cwd>/Users/dev/cline-editor</cwd>"}]"#,
        )
        .unwrap();
        fs::write(
            cli_sessions.join("priced.json"),
            r#"{"id":"cli-priced","model":"cline-cli-model","cwd":"/Users/dev/cline-cli","messages":[{"type":"say","say":"api_req_started","ts":1780308001000,"text":"{\"tokensIn\":30,\"tokensOut\":5,\"cacheReads\":2,\"cacheWrites\":1}"}]}"#,
        )
        .unwrap();
        fs::write(
            cli_sessions.join("unattributed.json"),
            r#"{"id":"cli-unattributed","messages":[{"type":"say","say":"api_req_started","ts":1780308002000,"text":"{\"tokensIn\":7,\"tokensOut\":3}"}]}"#,
        )
        .unwrap();
        fs::write(
            cli_sessions.join("unpriced.json"),
            r#"{"id":"cli-unpriced","model":"cline-unpriced-model","messages":[{"type":"say","say":"api_req_started","ts":1780308003000,"text":"{\"tokensIn\":11,\"tokensOut\":2}"}]}"#,
        )
        .unwrap();

        let mut conn = open_db(&dir.path().join("ledger.db")).unwrap();
        let result = scan_cline(&mut conn, &[dir.path().to_path_buf()]);
        assert_eq!(result.events_inserted, 4);
        assert!(result.error.is_none());

        for model in ["cline-editor-model", "cline-cli-model"] {
            crate::pricing::set_override(
                &conn,
                model,
                crate::pricing::OverrideRates {
                    input: Some(1e-6),
                    output: Some(2e-6),
                    cache_read: Some(3e-7),
                    cache_write: Some(4e-7),
                },
            )
            .unwrap();
        }

        let filters = crate::queries::Filters {
            tools: vec![SOURCE.to_string()],
            ..Default::default()
        };
        let summary = crate::queries::summary(&conn, &filters).unwrap();
        assert_eq!(summary.total_tokens, 196);
        assert_eq!(summary.requests, 4);
        assert_eq!(summary.unattributed_tokens, 10);
        assert!(
            summary.cost.is_some(),
            "priced Cline usage still has a Cost"
        );
        assert!(
            summary.has_unpriced,
            "unknown Cline Model makes Cost Partial"
        );
        assert_eq!(summary.unpriced_models, vec!["cline-unpriced-model"]);

        let tools = crate::queries::breakdown(&conn, "tool", &filters).unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].key.as_deref(), Some(SOURCE));
        assert_eq!(tools[0].total_tokens, 196);
        assert_eq!(tools[0].unattributed_tokens, 10);
        assert!(tools[0].cost.is_some());

        let pricing = crate::pricing::model_pricing(&conn).unwrap();
        assert_eq!(pricing.len(), 3);
        assert!(pricing.iter().all(|row| row.tool == SOURCE));
    }

    #[test]
    fn malformed_editor_artifact_warns_without_blocking_a_valid_cli_session() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let sessions = dir.path().join("sessions");
        fs::create_dir_all(tasks.join("broken")).unwrap();
        fs::create_dir_all(&sessions).unwrap();
        fs::write(tasks.join("broken/ui_messages.json"), "{not-json").unwrap();
        fs::write(
            sessions.join("valid.json"),
            r#"{"sessionId":"valid","cwd":"relative-project","messages":[{"type":"say","say":"api_req_started","ts":1780308000000,"text":"{\"tokensIn\":3,\"tokensOut\":2}"}]}"#,
        )
        .unwrap();

        let mut conn = open_db(&dir.path().join("ledger.db")).unwrap();
        let result = scan_cline(&mut conn, &[tasks, sessions]);
        assert_eq!(result.events_inserted, 1);
        assert!(result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("cline")));
        let project: Option<String> = conn
            .query_row(
                "SELECT project FROM events WHERE source = 'cline'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(project, None, "relative projects must remain unattributed");
    }

    #[test]
    fn unsupported_cli_version_warns_without_blocking_other_cline_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::write(
            sessions.join("unsupported.json"),
            r#"{"version":99,"messages":[{"type":"say","say":"api_req_started","ts":1780308000000,"text":"{\"tokensIn\":999,\"tokensOut\":1}"}]}"#,
        )
        .unwrap();
        fs::write(
            sessions.join("supported.json"),
            r#"{"id":"supported","messages":[{"type":"say","say":"api_req_started","ts":1780308000000,"text":"{\"tokensIn\":3,\"tokensOut\":2}"}]}"#,
        )
        .unwrap();

        let mut conn = open_db(&dir.path().join("ledger.db")).unwrap();
        let result = scan_cline(&mut conn, &[sessions]);
        assert_eq!(result.events_inserted, 1);
        assert!(result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("unsupported CLI session version")));
        let total: i64 = conn
            .query_row(
                "SELECT SUM(input_tokens + output_tokens) FROM events WHERE source = 'cline'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(total, 5);
    }
}
