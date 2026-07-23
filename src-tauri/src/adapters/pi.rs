use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::ctx::{content_bytes, est};
use super::{file_state_of, find_jsonl, rollup_worktree, unchanged};
use crate::db::{add_ctx_tool_rows, clear_ctx_tools_for_file, insert_events, set_file_state};
use crate::time::iso_to_epoch;
use crate::types::{CtxTokens, FileState, SourceScanResult, UsageEvent};

// Bump to force a full re-parse of every pi Session when the parser changes.
// v2: tree ancestry — per-entry context composition + tool-call drill-down.
const PI_PARSER_VERSION: i64 = 2;

// A pi Session is a tree (entries carry id + parentId), not a transcript. Each
// Request's context is the content ACTIVE ON ITS OWN ANCESTOR PATH — a sibling
// branch never leaks in. `Comp` is the running estimate (bytes/4) folded along
// that path; `tool` ⊆ `msg`. System stays unknown for pi (never estimated), so
// billed context partitions across messages + reasoning only.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Comp {
    msg: i64,
    tool: i64,
    reas: i64,
}

// One entry's own contribution to the running composition. `user_reset` clears
// prior thinking: a genuine user turn strips it from context (a tool result does
// not — it continues the assistant's turn).
#[derive(Debug, Clone, Copy, Default)]
struct Delta {
    msg: i64,
    tool: i64,
    reas: i64,
    user_reset: bool,
}

fn fold(mut base: Comp, d: &Delta) -> Comp {
    if d.user_reset {
        base.reas = 0;
    }
    base.msg += d.msg;
    base.tool += d.tool;
    base.reas += d.reas;
    base
}

// Split `billed` (input + cache_read + cache_write) by the content active before
// a Request. Partition is EXACT over the two observable categories: messages
// takes the remainder, so messages + reasoning == billed. System, agents, MCP,
// and skills stay NULL — pi does not establish them. toolcalls ⊆ messages.
fn attribute_pi(before: Comp, billed: i64) -> CtxTokens {
    let total = before.msg + before.reas;
    if total <= 0 {
        return CtxTokens::default(); // nothing observed before this Request
    }
    let reasoning_share = billed * before.reas / total;
    let messages = billed - reasoning_share;
    CtxTokens {
        messages: Some(messages),
        system: None,
        // A zero share is unobservable at this billed scale: report it as NULL,
        // never as a fabricated 0 (matches the claude convention).
        reasoning: (reasoning_share > 0).then_some(reasoning_share),
        toolcalls: Some((billed * before.tool / total).min(messages)),
        agents: None,
        mcp: None,
        skills: None,
    }
}

fn nonempty(value: Option<&str>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty()).map(str::to_owned)
}

// Estimated size of a user message's content. Read transiently to size it; only
// the estimate leaves — prompt text, images, and custom content are discarded.
fn est_user_content(content: &Value) -> i64 {
    if let Some(s) = content.as_str() {
        return est(s.len());
    }
    let Some(blocks) = content.as_array() else {
        return 0;
    };
    blocks
        .iter()
        .filter(|b| b["type"].as_str() == Some("text"))
        .map(|b| est(content_bytes(&b["text"])))
        .sum()
}

// Estimated size of a tool result's content (text blocks only). Result bodies
// are sized then dropped; nothing about the command output is retained.
fn est_toolresult_content(content: &Value) -> i64 {
    est_user_content(content)
}

// (msg, reasoning, toolcalls, [(tool name, args est)]) for an assistant message.
// Thinking text feeds reasoning; tool-call ARGUMENTS are sized but never stored —
// only the raw tool NAME leaves, for the context drill-down.
fn est_assistant_content(content: &Value) -> (i64, i64, i64, Vec<(String, i64)>) {
    let mut msg = 0;
    let mut reas = 0;
    let mut tool = 0;
    let mut calls = Vec::new();
    let Some(blocks) = content.as_array() else {
        return (0, 0, 0, calls);
    };
    for b in blocks {
        match b["type"].as_str() {
            Some("text") => msg += est(content_bytes(&b["text"])),
            Some("thinking") => reas += est(content_bytes(&b["thinking"])),
            Some("toolCall") => {
                let n = est(content_bytes(&b["arguments"]));
                msg += n;
                tool += n;
                let name = b["name"].as_str().unwrap_or("unknown").to_string();
                calls.push((name, n));
            }
            _ => {}
        }
    }
    (msg, reas, tool, calls)
}

struct ParsedPiFile {
    events: Vec<UsageEvent>,
    // (tool name, est_tokens, calls, epoch_ts) — calls=1 for a tool call, 0 for
    // its result, matching the claude ctx_tools convention.
    tool_rows: Vec<(String, i64, i64, i64)>,
    lines_skipped: u64,
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
    // A changed pi file is reparsed from the top (no byte-offset resume), so the
    // per-tool drill-down is rebuilt from scratch: clear this file's rows first,
    // then re-add. Copied fork/clone tool activity is deduplicated in the Ledger
    // via dedup_key; the drill-down is per source_file and needs no cross-file
    // reconciliation here.
    clear_ctx_tools_for_file(conn, &source_file)
        .map_err(|error| format!("pi: metadata {}: {error}", path.display()))?;
    let inserted = insert_events(conn, &parsed.events)
        .map_err(|error| format!("pi: insert {}: {error}", path.display()))?;
    add_ctx_tool_rows(conn, "pi", &source_file, &parsed.tool_rows)
        .map_err(|error| format!("pi: metadata {}: {error}", path.display()))?;
    set_file_state(conn, &source_file, state)
        .map_err(|error| format!("pi: metadata {}: {error}", path.display()))?;
    Ok((inserted, parsed.lines_skipped))
}

// Non-zero exclusive token counts of a pi `usage` block, or None when the block
// is absent or an all-zero placeholder. cacheWrite1h is the 1h-TTL subset of
// cacheWrite; an absent split means all writes are short-retention.
struct PiUsage {
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write_5m: i64,
    cache_write_1h: i64,
    reasoning: Option<i64>,
}

impl PiUsage {
    fn billed(&self) -> i64 {
        self.input + self.cache_read + self.cache_write_5m + self.cache_write_1h
    }
}

fn parse_usage(usage: &Value) -> Option<PiUsage> {
    if !usage.is_object() {
        return None;
    }
    let input = usage["input"].as_i64().unwrap_or(0).max(0);
    let output = usage["output"].as_i64().unwrap_or(0).max(0);
    let cache_read = usage["cacheRead"].as_i64().unwrap_or(0).max(0);
    let cache_write = usage["cacheWrite"].as_i64().unwrap_or(0).max(0);
    let cache_write_1h = usage["cacheWrite1h"].as_i64().unwrap_or(0).clamp(0, cache_write);
    if input == 0 && output == 0 && cache_read == 0 && cache_write == 0 {
        return None; // all-zero placeholder: never a Request
    }
    Some(PiUsage {
        input,
        output,
        cache_read,
        cache_write_5m: cache_write - cache_write_1h,
        cache_write_1h,
        reasoning: usage["reasoning"].as_i64().map(|r| r.clamp(0, output)),
    })
}

// Millisecond message-completion time, else the ISO entry time, as epoch seconds.
fn event_timestamp(message_ts_ms: Option<i64>, entry_ts_iso: Option<&str>) -> Option<i64> {
    message_ts_ms
        .map(|millis| millis / 1000)
        .or_else(|| entry_ts_iso.and_then(iso_to_epoch))
}

// A legacy pi Session (no entry ids) still deduplicates: identity is a hash of
// stable usage-bearing fields, independent of the containing file and Session.
fn legacy_dedup_key(
    kind: &str,
    message_ts_ms: Option<i64>,
    fallback_ts: i64,
    entry_ts_iso: Option<&str>,
    model: Option<&str>,
    u: &PiUsage,
) -> String {
    let stable_fields = serde_json::to_vec(&(
        message_ts_ms.unwrap_or(fallback_ts * 1000),
        entry_ts_iso,
        model,
        u.input,
        u.output,
        u.cache_read,
        u.cache_write_5m,
        u.cache_write_1h,
        u.reasoning,
    ))
    .expect("legacy pi identity fields always serialize");
    let digest = Sha256::digest(stable_fields);
    format!("pi:legacy:{kind}:{digest:x}")
}

fn parse_file(content: &str, source_file: &str) -> ParsedPiFile {
    let mut events = Vec::new();
    let mut tool_rows: Vec<(String, i64, i64, i64)> = Vec::new();
    let mut lines_skipped = 0u64;
    let mut session_id: Option<String> = None;
    let mut project: Option<String> = None;

    // Tree state. Entries arrive parent-before-child (creation order), so the
    // parent's composition is already known when its children are seen. A modern
    // entry keys off its own id; a legacy entry (no id) falls back to the linear
    // previous entry.
    let mut comp_after: HashMap<String, Comp> = HashMap::new();
    let mut parent_of: HashMap<String, Option<String>> = HashMap::new();
    let mut established_model: HashMap<String, Option<String>> = HashMap::new();
    // Per-entry content contribution, so a compaction can rebuild the context of
    // its retained tail (the entries it kept) after discarding the older prefix.
    let mut delta_by_id: HashMap<String, Delta> = HashMap::new();
    let mut last_comp = Comp::default();

    for complete_record in content.split_inclusive('\n') {
        if !complete_record.ends_with('\n') {
            continue; // pi may still be writing the final record
        }
        let raw = complete_record.strip_suffix('\n').unwrap_or(complete_record);
        if raw.trim().is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(_) => {
                lines_skipped += 1;
                continue;
            }
        };

        let entry_type = v["type"].as_str().unwrap_or("");
        if entry_type == "session" {
            session_id = nonempty(v["id"].as_str());
            project = v["cwd"]
                .as_str()
                .filter(|cwd| Path::new(cwd).is_absolute())
                .map(rollup_worktree);
            continue;
        }

        let id = nonempty(v["id"].as_str());
        let parent_id = nonempty(v["parentId"].as_str());
        let entry_ts_iso = nonempty(v["timestamp"].as_str());
        // Content active immediately before this entry: its parent's composition
        // (tree), or the previous entry (legacy linear). A resolvable parentId
        // never leaks a sibling branch in.
        let base = match &parent_id {
            Some(p) => comp_after.get(p).copied().unwrap_or_default(),
            None if id.is_some() => Comp::default(), // modern branch root
            None => last_comp,                       // legacy linear fallback
        };
        let row_ts = event_timestamp(v["message"]["timestamp"].as_i64(), entry_ts_iso.as_deref())
            .or_else(|| entry_ts_iso.as_deref().and_then(iso_to_epoch))
            .unwrap_or(0);

        let mut delta = Delta::default();

        if entry_type == "model_change" {
            if let Some(id) = &id {
                established_model.insert(id.clone(), nonempty(v["modelId"].as_str()));
                parent_of.insert(id.clone(), parent_id.clone());
            }
            continue; // Model changes carry no context and no usage.
        }

        if entry_type == "compaction" {
            // pi's built-in compaction summarizes the branch prefix into `summary`
            // and keeps entries from firstKeptEntryId onward. It is one model call
            // (its own usage block) — a built-in summary (fromHook false) inherits
            // the active Model from its parent branch; an extension-provided one
            // (fromHook true) is left Unattributed (handled in #41). Any messages
            // nested inside this entry are never separate JSONL lines, so they are
            // inherently not counted as Usage Records.
            let from_hook = v["fromHook"].as_bool().unwrap_or(false);
            let first_kept = nonempty(v["firstKeptEntryId"].as_str());
            let summary_est = est(content_bytes(&v["summary"]));
            if let Some(u) = parse_usage(&v["usage"]) {
                // A built-in summary inherits the branch Model and attributes the
                // pre-compaction context. An extension-provided one (fromHook true)
                // is opaque: no Model and no context attribution (#41).
                let (model, before) = if from_hook {
                    (None, Comp::default())
                } else {
                    (active_model(&parent_of, &established_model, parent_id.as_deref()), base)
                };
                emit_summary(
                    &mut events,
                    &mut lines_skipped,
                    source_file,
                    &id,
                    entry_ts_iso.as_deref(),
                    model,
                    before,
                    &u,
                    &session_id,
                    &project,
                );
            }
            // Descendants see the summary plus the retained tail in place of the
            // superseded prefix. Historical Usage Records from that prefix stay.
            let retained =
                rebuild_retained(&parent_of, &delta_by_id, parent_id.as_deref(), first_kept.as_deref());
            let after = Comp {
                msg: summary_est + retained.msg,
                tool: retained.tool,
                reas: retained.reas,
            };
            if let Some(id) = &id {
                comp_after.insert(id.clone(), after);
                parent_of.insert(id.clone(), parent_id);
                // A nested compaction inside a future retained window loses its
                // summary weight here. ponytail: rare; revisit if pi nests them.
                delta_by_id.insert(id.clone(), Delta::default());
            }
            last_comp = after;
            continue;
        }

        if entry_type == "message" {
            let message = &v["message"];
            match message["role"].as_str().unwrap_or("") {
                "user" => {
                    delta.msg = est_user_content(&message["content"]);
                    delta.user_reset = true;
                }
                "assistant" => {
                    let (m, r, t, calls) = est_assistant_content(&message["content"]);
                    delta = Delta { msg: m, tool: t, reas: r, user_reset: false };
                    let model =
                        nonempty(message["responseModel"].as_str()).or_else(|| nonempty(message["model"].as_str()));
                    if let Some(id) = &id {
                        established_model.insert(id.clone(), model.clone());
                    }
                    let usage = &message["usage"];
                    match parse_usage(usage) {
                        Some(u) => emit_assistant(
                            &mut events,
                            &mut lines_skipped,
                            source_file,
                            &id,
                            entry_ts_iso.as_deref(),
                            message["timestamp"].as_i64(),
                            model,
                            base,
                            &u,
                            &session_id,
                            &project,
                        ),
                        // A usage block present but all-zero is a placeholder: skip
                        // and count it. An absent usage block is simply not a Request.
                        None if usage.is_object() => lines_skipped += 1,
                        None => {}
                    }
                    // Tool CALLS: one call each; arguments are sized then dropped.
                    for (name, e) in calls {
                        tool_rows.push((name, e, 1, row_ts));
                    }
                }
                "toolResult" => {
                    let e = est_toolresult_content(&message["content"]);
                    delta = Delta { msg: e, tool: e, reas: 0, user_reset: false };
                    let name = message["toolName"].as_str().unwrap_or("unknown").to_string();
                    tool_rows.push((name, e, 0, row_ts));
                    // A usage-bearing tool result reports nested model work with no
                    // trustworthy Model — one Unattributed Usage Record (#41). An
                    // all-zero block is ordinary retained context, not a Request.
                    if let Some(u) = parse_usage(&message["usage"]) {
                        emit_tool_result(
                            &mut events,
                            &mut lines_skipped,
                            source_file,
                            &id,
                            entry_ts_iso.as_deref(),
                            message["timestamp"].as_i64(),
                            &u,
                            &session_id,
                            &project,
                        );
                    }
                }
                _ => {}
            }
        }

        let after = fold(base, &delta);
        if let Some(id) = &id {
            comp_after.insert(id.clone(), after);
            parent_of.insert(id.clone(), parent_id);
            delta_by_id.insert(id.clone(), delta);
        }
        last_comp = after;
    }

    ParsedPiFile {
        events,
        tool_rows,
        lines_skipped,
    }
}

// The Model active on a branch: walk the ancestor path from `start` and return
// the nearest entry that establishes one (a model_change's modelId, or the model
// an assistant reported). A sibling branch's changes never appear on this path.
fn active_model(
    parent_of: &HashMap<String, Option<String>>,
    established_model: &HashMap<String, Option<String>>,
    start: Option<&str>,
) -> Option<String> {
    let mut cursor = start.map(str::to_owned);
    while let Some(id) = cursor {
        if let Some(Some(model)) = established_model.get(&id) {
            return Some(model.clone());
        }
        cursor = parent_of.get(&id).cloned().flatten();
    }
    None
}

// The content of a compaction's retained tail: the entries from firstKeptEntryId
// up to the compaction's parent, re-folded from scratch (root→leaf order). The
// older prefix before firstKeptEntryId is discarded — the summary replaces it.
fn rebuild_retained(
    parent_of: &HashMap<String, Option<String>>,
    delta_by_id: &HashMap<String, Delta>,
    parent: Option<&str>,
    first_kept: Option<&str>,
) -> Comp {
    let Some(first_kept) = first_kept else {
        return Comp::default(); // nothing kept: the summary stands alone
    };
    let mut chain = Vec::new();
    let mut cursor = parent.map(str::to_owned);
    let mut reached = false;
    while let Some(id) = cursor {
        let is_first_kept = id == first_kept;
        chain.push(id.clone());
        if is_first_kept {
            reached = true;
            break;
        }
        cursor = parent_of.get(&id).cloned().flatten();
    }
    if !reached {
        return Comp::default(); // firstKeptEntryId not on the path: keep only summary
    }
    let mut c = Comp::default();
    for id in chain.iter().rev() {
        if let Some(d) = delta_by_id.get(id) {
            c = fold(c, d);
        }
    }
    c
}

#[allow(clippy::too_many_arguments)]
fn push_pi_event(
    events: &mut Vec<UsageEvent>,
    source_file: &str,
    dedup_key: String,
    timestamp: i64,
    model: Option<String>,
    u: &PiUsage,
    session_id: &Option<String>,
    project: &Option<String>,
    ctx: CtxTokens,
) {
    events.push(UsageEvent {
        dedup_key,
        source: "pi".to_string(),
        timestamp,
        model,
        project: project.clone(),
        api_calls: 1,
        input_tokens: u.input,
        output_tokens: u.output,
        cache_read_tokens: u.cache_read,
        cache_write_5m_tokens: u.cache_write_5m,
        cache_write_1h_tokens: u.cache_write_1h,
        source_file: source_file.to_string(),
        session_id: session_id.clone(),
        reasoning_tokens: u.reasoning,
        ctx,
    });
}

#[allow(clippy::too_many_arguments)]
fn emit_assistant(
    events: &mut Vec<UsageEvent>,
    lines_skipped: &mut u64,
    source_file: &str,
    id: &Option<String>,
    entry_ts_iso: Option<&str>,
    message_ts_ms: Option<i64>,
    model: Option<String>,
    before: Comp,
    u: &PiUsage,
    session_id: &Option<String>,
    project: &Option<String>,
) {
    let Some(timestamp) = event_timestamp(message_ts_ms, entry_ts_iso) else {
        *lines_skipped += 1; // usage with no usable time is dropped, not guessed
        return;
    };
    let dedup_key = match (id.as_deref(), entry_ts_iso) {
        (Some(id), Some(ts)) => format!("pi:message:{id}:{ts}"),
        _ => legacy_dedup_key("message", message_ts_ms, timestamp, entry_ts_iso, model.as_deref(), u),
    };
    let ctx = attribute_pi(before, u.billed());
    push_pi_event(events, source_file, dedup_key, timestamp, model, u, session_id, project, ctx);
}

// Compaction / branch-summary usage. Its Request time is the Session entry time
// (there is no separate message-completion time), and it uses the canonical token
// mapping — pi's logged cost is ignored, like every other Source.
#[allow(clippy::too_many_arguments)]
fn emit_summary(
    events: &mut Vec<UsageEvent>,
    lines_skipped: &mut u64,
    source_file: &str,
    id: &Option<String>,
    entry_ts_iso: Option<&str>,
    model: Option<String>,
    before: Comp,
    u: &PiUsage,
    session_id: &Option<String>,
    project: &Option<String>,
) {
    let Some(timestamp) = entry_ts_iso.and_then(iso_to_epoch) else {
        *lines_skipped += 1;
        return;
    };
    let dedup_key = match (id.as_deref(), entry_ts_iso) {
        (Some(id), Some(ts)) => format!("pi:compaction:{id}:{ts}"),
        _ => legacy_dedup_key("compaction", None, timestamp, entry_ts_iso, model.as_deref(), u),
    };
    let ctx = attribute_pi(before, u.billed());
    push_pi_event(events, source_file, dedup_key, timestamp, model, u, session_id, project, ctx);
}

// A usage-bearing tool result: nested model work with no trustworthy Model, left
// Unattributed (no Model, no context attribution). Its time is the message
// completion time with an entry-time fallback. One block is one observable
// Request — a source-observable lower bound, since a block may aggregate several
// hidden calls that pi does not separate.
#[allow(clippy::too_many_arguments)]
fn emit_tool_result(
    events: &mut Vec<UsageEvent>,
    lines_skipped: &mut u64,
    source_file: &str,
    id: &Option<String>,
    entry_ts_iso: Option<&str>,
    message_ts_ms: Option<i64>,
    u: &PiUsage,
    session_id: &Option<String>,
    project: &Option<String>,
) {
    let Some(timestamp) = event_timestamp(message_ts_ms, entry_ts_iso) else {
        *lines_skipped += 1;
        return;
    };
    let dedup_key = match (id.as_deref(), entry_ts_iso) {
        (Some(id), Some(ts)) => format!("pi:message:{id}:{ts}"),
        _ => legacy_dedup_key("toolresult", message_ts_ms, timestamp, entry_ts_iso, None, u),
    };
    push_pi_event(events, source_file, dedup_key, timestamp, None, u, session_id, project, CtxTokens::default());
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
        // 3 assistant Requests + 1 Unattributed tool-result Request.
        assert_eq!(parsed.events.len(), 4);
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
        // Context is the ancestor path before this Request: only the parent user
        // message is active, so all billed input attributes to messages. pi never
        // estimates a system prompt, and there is no prior-turn reasoning yet.
        assert_eq!(first.ctx.messages, Some(135), "billed = input+cacheRead+cacheWrite");
        assert_eq!(first.ctx.system, None);
        assert_eq!(first.ctx.reasoning, None);
        assert_eq!(first.ctx.toolcalls, Some(0));
        assert_eq!(first.ctx.mcp, None);
        assert_eq!(first.ctx.skills, None);

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

        // The usage-bearing tool result is Unattributed: no Model, no context
        // attribution, timed by its own message-completion time.
        let tool_result = &parsed.events[3];
        assert_eq!(tool_result.model, None, "tool-result usage guesses no Model");
        assert_eq!(tool_result.timestamp, 1_782_907_206, "tool-result message time");
        assert_eq!(tool_result.input_tokens, 900);
        assert_eq!(tool_result.output_tokens, 900);
        assert_eq!(tool_result.cache_read_tokens, 900);
        assert_eq!(tool_result.cache_write_5m_tokens, 900);
        assert_eq!(tool_result.reasoning_tokens, None);
        assert_eq!(tool_result.ctx, CtxTokens::default(), "opaque nested work: no ctx");
        assert!(tool_result.dedup_key.starts_with("pi:message:e5f6a7b8:"));
    }

    // A tree with two branches off one shared node. Branch A is abandoned; branch
    // B carries a model_change, thinking, and a tool call. Proves: every branch's
    // usage is counted; a Request's context follows only its ancestor path (a
    // sibling's tool/reasoning content never leaks); reasoning and tool-call
    // context attribute; and the canonical partition holds — all through the
    // production scan + Ledger queries.
    const BRANCH_SESSION: &str = concat!(
        r#"{"type":"session","version":3,"id":"branch-session","cwd":"/projects/branch"}"#, "\n",
        r#"{"type":"message","id":"u1","parentId":null,"timestamp":"2026-07-01T00:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":"kick off the shared conversation here"}],"timestamp":1782864001000}}"#, "\n",
        r#"{"type":"message","id":"a2","parentId":"u1","timestamp":"2026-07-01T00:00:02.000Z","message":{"role":"assistant","model":"base-model","content":[{"type":"text","text":"acknowledged, shared base reply"}],"usage":{"input":10,"output":5,"cacheRead":0,"cacheWrite":0},"timestamp":1782864002000}}"#, "\n",
        r#"{"type":"message","id":"u2a","parentId":"a2","timestamp":"2026-07-01T00:00:03.000Z","message":{"role":"user","content":[{"type":"text","text":"branch A question, short and toolless"}],"timestamp":1782864003000}}"#, "\n",
        r#"{"type":"message","id":"a3a","parentId":"u2a","timestamp":"2026-07-01T00:00:04.000Z","message":{"role":"assistant","model":"branch-a-model","content":[],"usage":{"input":40,"output":10,"cacheRead":0,"cacheWrite":0},"timestamp":1782864004000}}"#, "\n",
        r#"{"type":"message","id":"u2b","parentId":"a2","timestamp":"2026-07-01T00:00:05.000Z","message":{"role":"user","content":[{"type":"text","text":"branch B question that needs a tool"}],"timestamp":1782864005000}}"#, "\n",
        r#"{"type":"model_change","id":"mcb","parentId":"u2b","modelId":"branch-b-model","provider":"anthropic","timestamp":"2026-07-01T00:00:05.500Z"}"#, "\n",
        r#"{"type":"message","id":"a3b","parentId":"mcb","timestamp":"2026-07-01T00:00:06.000Z","message":{"role":"assistant","model":"branch-b-model","content":[{"type":"thinking","thinking":"let me reason about which tool to run and why it fits the branch B request"},{"type":"toolCall","id":"tcb","name":"grep","arguments":{"pattern":"needle","path":"/projects/branch"}}],"usage":{"input":50,"output":20,"cacheRead":0,"cacheWrite":0},"timestamp":1782864006000}}"#, "\n",
        r#"{"type":"message","id":"rb","parentId":"a3b","timestamp":"2026-07-01T00:00:07.000Z","message":{"role":"toolResult","toolCallId":"tcb","toolName":"grep","content":[{"type":"text","text":"match at line 1, line 2, line 3, line 4, line 5, line 6, line 7"}],"timestamp":1782864007000}}"#, "\n",
        r#"{"type":"message","id":"a4b","parentId":"rb","timestamp":"2026-07-01T00:00:08.000Z","message":{"role":"assistant","model":"branch-b-model","content":[],"usage":{"input":200,"output":10,"cacheRead":0,"cacheWrite":0},"timestamp":1782864008000}}"#, "\n",
    );

    #[test]
    fn every_branch_is_counted_while_context_follows_only_its_ancestor_path() {
        let sessions = tempfile::tempdir().unwrap();
        let app = tempfile::tempdir().unwrap();
        fs::write(sessions.path().join("branch.jsonl"), BRANCH_SESSION).unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();

        let result = scan_root(&mut conn, sessions.path());
        assert!(result.error.is_none());
        // a2 + a3a (abandoned branch) + a3b + a4b: every branch's usage lands.
        assert_eq!(
            queries::summary(&conn, &Filters::default()).unwrap().requests,
            4,
            "abandoned branch A usage is still counted",
        );

        // Sibling isolation: branch A's leaf saw no tool content, so its toolcall
        // context is zero; branch B's later Request inherited the grep call + its
        // result, so its toolcall context is nonzero.
        let toolcalls_of = |input: i64| -> (Option<i64>, Option<i64>) {
            conn.query_row(
                "SELECT ctx_toolcalls, ctx_reasoning FROM events WHERE source='pi' AND input_tokens = ?1",
                [input],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
        };
        let (a3a_tool, a3a_reas) = toolcalls_of(40); // branch A leaf
        assert_eq!(a3a_tool, Some(0), "branch A never saw branch B's tool call");
        assert_eq!(a3a_reas, None, "branch A never saw branch B's thinking");

        let (a4b_tool, a4b_reas) = toolcalls_of(200); // branch B, after the tool loop
        assert!(a4b_tool.unwrap() > 0, "branch B inherited its own tool-call context");
        assert!(a4b_reas.unwrap() > 0, "branch B inherited its own reasoning context");

        // Every attributed pi event partitions billed exactly (messages + system +
        // reasoning) with tool ⊆ messages — the canonical invariants.
        crate::invariants::assert_partition_exact(&conn);
        crate::invariants::assert_secondary_subset(&conn);

        // Tool drill-down exposes the raw tool name + a call count, no arguments.
        let tools = queries::ctx_tools(&conn, &Filters::default()).unwrap();
        let grep = tools
            .iter()
            .find(|t| t.source == "pi" && t.name == "grep")
            .expect("grep tool call surfaced in the drill-down");
        assert!(grep.calls >= 1 && grep.est_tokens > 0);
    }

    // A branch that runs a tool, then a built-in compaction (fromHook false) with
    // a model_change just before it, then a post-compaction Request. Proves: the
    // compaction is one Request whose Model is inherited from the branch's active
    // model_change and whose time is the entry time; the descendant's context uses
    // the summary + retained tail (the pre-compaction tool prefix is gone) while
    // the pre-compaction Records survive; and a re-scan does not duplicate.
    const COMPACTION_SESSION: &str = concat!(
        r#"{"type":"session","version":3,"id":"comp-session","cwd":"/projects/comp"}"#, "\n",
        r#"{"type":"message","id":"u1","parentId":null,"timestamp":"2026-07-01T00:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":"start the long conversation that will later be compacted away"}],"timestamp":1782864001000}}"#, "\n",
        r#"{"type":"message","id":"a1","parentId":"u1","timestamp":"2026-07-01T00:00:02.000Z","message":{"role":"assistant","model":"turn-model","content":[{"type":"toolCall","id":"tc1","name":"search","arguments":{"pattern":"in the pre-compaction prefix"}}],"usage":{"input":20,"output":5,"cacheRead":0,"cacheWrite":0},"timestamp":1782864002000}}"#, "\n",
        r#"{"type":"message","id":"r1","parentId":"a1","timestamp":"2026-07-01T00:00:03.000Z","message":{"role":"toolResult","toolCallId":"tc1","toolName":"search","content":[{"type":"text","text":"a large search result that only lives in the superseded prefix here"}],"timestamp":1782864003000}}"#, "\n",
        r#"{"type":"message","id":"a2","parentId":"r1","timestamp":"2026-07-01T00:00:04.000Z","message":{"role":"assistant","model":"turn-model","content":[{"type":"text","text":"kept reply"}],"usage":{"input":30,"output":5,"cacheRead":0,"cacheWrite":0},"timestamp":1782864004000}}"#, "\n",
        r#"{"type":"model_change","id":"mc","parentId":"a2","modelId":"changed-model","provider":"anthropic","timestamp":"2026-07-01T00:00:04.500Z"}"#, "\n",
        r#"{"type":"compaction","id":"cmp","parentId":"mc","timestamp":"2026-07-01T00:00:05.000Z","firstKeptEntryId":"a2","summary":"short summary","tokensBefore":900,"details":{"readFiles":[],"modifiedFiles":[]},"usage":{"input":500,"output":40,"cacheRead":0,"cacheWrite":0},"fromHook":false}"#, "\n",
        r#"{"type":"message","id":"u3","parentId":"cmp","timestamp":"2026-07-01T00:00:06.000Z","message":{"role":"user","content":[{"type":"text","text":"continue after compaction"}],"timestamp":1782864006000}}"#, "\n",
        r#"{"type":"message","id":"a3","parentId":"u3","timestamp":"2026-07-01T00:00:07.000Z","message":{"role":"assistant","model":"turn-model","content":[],"usage":{"input":40,"output":10,"cacheRead":0,"cacheWrite":0},"timestamp":1782864007000}}"#, "\n",
    );

    #[test]
    fn built_in_compaction_is_a_request_that_reshapes_descendant_context() {
        let sessions = tempfile::tempdir().unwrap();
        let app = tempfile::tempdir().unwrap();
        fs::write(sessions.path().join("comp.jsonl"), COMPACTION_SESSION).unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();

        let result = scan_root(&mut conn, sessions.path());
        assert!(result.error.is_none());
        // a1 + a2 + compaction + a3.
        assert_eq!(
            queries::summary(&conn, &Filters::default()).unwrap().requests,
            4,
            "the compaction usage block is one observable Request",
        );

        // The compaction inherits the active Model from its parent branch's most
        // recent model_change, and its time is the Session entry time.
        let (model, ts): (Option<String>, i64) = conn
            .query_row(
                "SELECT model, timestamp FROM events WHERE source='pi' AND input_tokens = 500",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(model.as_deref(), Some("changed-model"), "built-in summary inherits the branch Model");
        assert_eq!(ts, iso_to_epoch("2026-07-01T00:00:05.000Z").unwrap());

        // Context: the pre-compaction Request (a2, input 30) saw the search tool in
        // its prefix; the post-compaction Request (a3, input 40) sees the summary +
        // retained tail instead, so no tool context survives.
        let toolcalls_of = |input: i64| -> Option<i64> {
            conn.query_row(
                "SELECT ctx_toolcalls FROM events WHERE source='pi' AND input_tokens = ?1",
                [input],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert!(toolcalls_of(30).unwrap() > 0, "pre-compaction Request saw the tool prefix");
        assert_eq!(toolcalls_of(40), Some(0), "summary replaced the superseded tool prefix");

        crate::invariants::assert_partition_exact(&conn);
        crate::invariants::assert_secondary_subset(&conn);

        // Pre-compaction Records persist, and a second scan adds nothing.
        let second = scan_root(&mut conn, sessions.path());
        assert_eq!(second.events_inserted, 0, "re-scan does not duplicate compaction usage");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM events WHERE source='pi'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 4, "pre-compaction history is retained");
    }

    // Auxiliary usage with no trustworthy Model: a usage-bearing tool result and
    // an extension-provided summary (compaction fromHook true). A zero-usage tool
    // result is ordinary context, not a Request. Proves both become Unattributed
    // Records and a selection of only Unattributed usage shows no Cost, not $0.
    const AUXILIARY_SESSION: &str = concat!(
        r#"{"type":"session","version":3,"id":"aux-session","cwd":"/projects/aux"}"#, "\n",
        r#"{"type":"message","id":"u1","parentId":null,"timestamp":"2026-07-01T00:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":"run a subagent tool that reports nested usage"}],"timestamp":1782864001000}}"#, "\n",
        r#"{"type":"message","id":"tr","parentId":"u1","timestamp":"2026-07-01T00:00:02.000Z","message":{"role":"toolResult","toolCallId":"c1","toolName":"agent","content":[{"type":"text","text":"nested result body that is never persisted"}],"usage":{"input":700,"output":300,"cacheRead":0,"cacheWrite":0},"timestamp":1782864002500}}"#, "\n",
        r#"{"type":"compaction","id":"cmp","parentId":"tr","timestamp":"2026-07-01T00:00:03.000Z","firstKeptEntryId":"tr","summary":"extension summary","tokensBefore":1000,"details":{},"usage":{"input":400,"output":50,"cacheRead":0,"cacheWrite":0},"fromHook":true}"#, "\n",
        r#"{"type":"message","id":"trz","parentId":"cmp","timestamp":"2026-07-01T00:00:04.000Z","message":{"role":"toolResult","toolCallId":"c2","toolName":"read","content":[{"type":"text","text":"a plain result with no usage block"}],"timestamp":1782864004000}}"#, "\n",
    );

    #[test]
    fn tool_result_and_extension_summary_usage_are_unattributed() {
        let parsed = parse_file(AUXILIARY_SESSION, "/fixtures/aux.jsonl");
        // tool result (usage) + extension summary; the zero-usage tool result is not
        // a Request.
        assert_eq!(parsed.events.len(), 2);

        let tool_result = parsed
            .events
            .iter()
            .find(|e| e.input_tokens == 700)
            .unwrap();
        assert_eq!(tool_result.model, None);
        assert_eq!(tool_result.timestamp, 1_782_864_002, "message time wins for tool results");
        assert_eq!(tool_result.ctx, CtxTokens::default());

        let summary = parsed.events.iter().find(|e| e.input_tokens == 400).unwrap();
        assert_eq!(summary.model, None, "extension summary guesses no Model");
        assert_eq!(summary.timestamp, 1_782_864_003, "summary uses the entry time");
        assert!(summary.dedup_key.starts_with("pi:compaction:cmp:"));
        assert_eq!(summary.ctx, CtxTokens::default(), "extension detail is opaque");

        // Through the Ledger: a selection of only Unattributed usage shows no Cost
        // (not $0) and no Unpriced Model — the tokens still count.
        let sessions = tempfile::tempdir().unwrap();
        let app = tempfile::tempdir().unwrap();
        fs::write(sessions.path().join("aux.jsonl"), AUXILIARY_SESSION).unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        assert!(scan_root(&mut conn, sessions.path()).error.is_none());

        let s = queries::summary(&conn, &Filters::default()).unwrap();
        assert_eq!(s.requests, 2);
        assert_eq!(s.total_tokens, 700 + 300 + 400 + 50);
        assert_eq!(s.unattributed_tokens, 700 + 300 + 400 + 50);
        assert_eq!(s.cost, None, "all-Unattributed selection has no Cost, never $0");
        assert!(!s.has_unpriced, "Unattributed Usage is not an Unpriced Model");
    }

    #[test]
    fn active_model_follows_ancestry_and_isolates_sibling_branches() {
        let mut parent_of: HashMap<String, Option<String>> = HashMap::new();
        let mut established: HashMap<String, Option<String>> = HashMap::new();
        // root ─┬─ mcA (model_change A) ── leafA
        //       └─ mcB (model_change B)
        parent_of.insert("root".into(), None);
        established.insert("root".into(), None);
        parent_of.insert("mcA".into(), Some("root".into()));
        established.insert("mcA".into(), Some("model-A".into()));
        parent_of.insert("mcB".into(), Some("root".into()));
        established.insert("mcB".into(), Some("model-B".into()));
        parent_of.insert("leafA".into(), Some("mcA".into()));
        established.insert("leafA".into(), None);

        assert_eq!(
            active_model(&parent_of, &established, Some("leafA")),
            Some("model-A".into()),
            "walks up to the nearest Model change on its own branch",
        );
        assert_eq!(
            active_model(&parent_of, &established, Some("mcB")),
            Some("model-B".into()),
            "sibling branch resolves independently",
        );
        assert_eq!(active_model(&parent_of, &established, Some("root")), None);
        assert_eq!(active_model(&parent_of, &established, None), None);
    }
}
