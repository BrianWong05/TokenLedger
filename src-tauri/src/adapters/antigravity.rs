// TokenLedger — Google Antigravity adapter.
//
// Antigravity (IDE agent and CLI) stores each Session as a SQLite
// database: `~/.gemini/antigravity/conversations/<uuid>.db` (IDE) and
// `~/.gemini/antigravity-cli/conversations/<uuid>.db` (CLI), same schema.
// Sibling `<uuid>.pb` files are encrypted Sessions — legacy *and* current
// format — that only the live language server can decrypt: Unreadable
// Artifacts, counted (never warned) so token totals can carry the ≥ marker.
// See docs/source-evidence/antigravity.md and ADR-0017.
//
// Each `gen_metadata` row is one generation (one API call) encoded as a
// protobuf blob. Google publishes no .proto, but the language server embeds its
// own descriptors, and the field numbers below were read straight out of them
// (`ChatModelMetadata`, `ModelUsageStats`) rather than guessed. Cross-checked
// against real databases: #3 == #9 + #10 on every row that carries all three.
//
//   gen_metadata.#1 (chatModel = ChatModelMetadata)
//     .#19 (string)             → model id (e.g. "gemini-3-flash-a")
//     .#9.#4 = {#1 sec, #2 ns}  → per-generation wall-clock timestamp
//     .#4 (usage = ModelUsageStats)
//       .#1 (varint)            → Model enum — an identifier, NOT a token count
//       .#2 (varint)            → input tokens
//       .#3 (varint)            → total output tokens (== #9 + #10)
//       .#4 (varint)            → cache-write tokens
//       .#5 (varint)            → cache-read tokens
//       .#9 (varint)            → thinking/reasoning tokens
//       .#10 (varint)           → response text tokens
//       .#11 (string)           → responseId (dedup key)
//   trajectory_metadata_blob.#2 = {#1 sec}    → conversation created-at
//   trajectory_metadata_blob.#1.#1 (string)   → workspace file:// URI
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use super::{file_state_of, unchanged};
use crate::db::{replace_file_events, set_file_state};
use crate::export_artifact::{self, ConversationExport};
use crate::proto::{message_field, string_field, varint_field};
use crate::types::{FileState, SourceScanResult, UsageEvent};
use crate::uri::file_uri_to_path;

/// Bump to force a full re-parse of every Antigravity DB on the next scan.
/// Stored in the file-state's otherwise-unused `byte_offset` (this adapter
/// re-reads whole files, so it never tracks a real offset), which makes the
/// re-scan self-clearing: the mismatch fires once, then the new version is
/// persisted. Beats a one-shot migration, which a dev run would consume.
/// v1 = wire aliases resolved to real model ids (see `resolve_model`).
/// v2 = usage read against the server's own descriptors: `#1` is the Model enum
/// and no longer inflates input, and `#9`/`#10` are reasoning/response the right
/// way round.
/// v3 = picker-label aliases from the `.pb` exports resolved (see `resolve_model`).
/// v4 = Model enums the server paired one-to-one with an alias resolved too
/// (see `resolve_model_enum`), so alias-less generations of those enums price.
/// v5 = enums identified by explicit single-enum Sessions (1008, 1012).
const PARSER_VERSION: i64 = 5;

/// Antigravity records internal wire aliases and picker labels, not Model ids.
/// `gemini-3-flash-a`/`-b` are the server's MODEL_PLACEHOLDER_M132/_M20 enum
/// values — both Gemini 3.5 Flash (High), despite the "3-flash" spelling;
/// mapping them to the `gemini-3-flash-preview` family would be wrong. The
/// `.pb` exports write a second vocabulary — the picker labels — where the
/// same line appears as `gemini-3-flash` (with `-agent` for its agent mode),
/// so those join it; the pro picker labels are the preview catalog rows with
/// a thinking-tier suffix, and the Claude thinking labels are the base Opus
/// Models.
/// `gemini-default` names whichever Gemini was the default when the row was
/// written, so it resolves against the event's own timestamp. Anything
/// unrecognized passes through untouched and simply lands in the unpriced
/// list under its raw name — which is the signal that Antigravity has
/// renamed a placeholder again.
fn resolve_model(raw: &str, ts: i64) -> String {
    match raw.to_lowercase().as_str() {
        "gemini-3-flash-a" | "gemini-3-flash-b" | "gemini-3-flash" | "gemini-3-flash-agent" => {
            "gemini-3.5-flash".to_string()
        }
        "gemini-3-flash-c" => "gemini-3.5-flash".to_string(),
        "gemini-3-pro-high" | "gemini-3-pro-low" => "gemini-3-pro-preview".to_string(),
        "gemini-3.1-pro-high" | "gemini-3.1-pro-low" => "gemini-3.1-pro-preview".to_string(),
        "claude-opus-4-5-thinking" => "claude-opus-4-5".to_string(),
        "claude-opus-4-6-thinking" => "claude-opus-4-6".to_string(),
        // Exclusive upper bounds; a row exactly on a boundary is the later era.
        "gemini-default" => match ts {
            _ if ts < 1742860800 => "gemini-2.0-flash".to_string(), // < 2025-03-25
            _ if ts < 1779148800 => "gemini-2.5-flash".to_string(), // < 2026-05-19
            _ => "gemini-3.5-flash".to_string(),
        },
        _ => raw.to_string(),
    }
}

/// Model enums that real exports paired one-to-one with a wire alias
/// (2026-08-10, one genuine install): the server named those generations
/// twice, and the pairing names the alias-less ones too. 1084 is the
/// "default" enum — era-based, exactly like `gemini-default`. 1008 and 1012
/// were never aliased but are identified by explicit single-enum Sessions
/// (32 picker sessions on gemini-3-pro-high, 3 on claude-opus-4-5-thinking;
/// the method cross-validates on every enum that has both signals). 1007
/// returns None and stays raw: the server never said what it is.
/// Each identified enum delegates to the canonical alias it was paired
/// with, so the alias table in `resolve_model` stays the single source of
/// truth.
fn resolve_model_enum(model_enum: u64, ts: i64) -> Option<String> {
    let alias = match model_enum {
        1008 => "gemini-3-pro-high", // explicit-picker Sessions
        1012 => "claude-opus-4-5-thinking", // explicit-picker Sessions
        1018 => "gemini-3-flash",
        1047 => "gemini-3-flash-c",
        1026 => "claude-opus-4-6-thinking",
        1035 => "claude-sonnet-4-6",
        1036 => "gemini-3.1-pro-low",
        1037 => "gemini-3.1-pro-high",
        1084 => "gemini-default",
        _ => return None,
    };
    Some(resolve_model(alias, ts))
}

pub fn scan_antigravity(conn: &mut Connection, roots: &[&Path]) -> SourceScanResult {
    let mut result = SourceScanResult::default();
    let mut dirs: Vec<Vec<PathBuf>> = Vec::with_capacity(roots.len());
    for root in roots {
        if root.is_file() {
            process_db(conn, root, &mut result);
            continue;
        }
        match fs::read_dir(root) {
            Ok(entries) => dirs.push(entries.flatten().map(|e| e.path()).collect()),
            Err(_) => continue, // missing dir → zero events, no error
        }
    }

    // Every export across every app data dir first, because a `.pb` can only be
    // judged once its export has been *read*, and the copy that vindicates it
    // may live in a different directory. The set records Sessions whose export
    // actually parsed, not merely those with a file of the right name: a
    // malformed export would otherwise silence the ≥ while contributing
    // nothing, and the total would drop with nothing left to say why.
    let mut stood_in_for: HashSet<String> = HashSet::new();
    for paths in &dirs {
        for path in paths {
            let Some(session) = export_artifact::session_id(path) else { continue };
            // Antigravity mirrors a Session across its app data dirs. Read it
            // once: the events carry the same dedup keys either way, so a second
            // read changes no total but would report inserts that never happened.
            if stood_in_for.contains(&session) {
                continue;
            }
            if process_export(conn, path, &mut result) {
                stood_in_for.insert(session);
            }
        }
    }

    for paths in &dirs {
        for path in paths {
            match path.extension().and_then(|e| e.to_str()) {
                Some("db") => process_db(conn, path, &mut result),
                // Encrypted, and still an Unreadable Artifact — but only while
                // no readable export stands in for it (ADR-0017, ADR-0018).
                Some("pb") if !pb_is_exported(path, &stood_in_for) => {
                    result.artifacts_unreadable += 1;
                    let mtime = file_state_of(path).mtime;
                    result.unreadable_max_mtime =
                        Some(result.unreadable_max_mtime.unwrap_or(i64::MIN).max(mtime));
                }
                _ => {}
            }
        }
    }
    result
}

fn pb_is_exported(pb: &Path, exported: &HashSet<String>) -> bool {
    pb.file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|id| exported.contains(id))
}

fn process_db(conn: &mut Connection, db_path: &Path, result: &mut SourceScanResult) {
    let state = FileState { byte_offset: PARSER_VERSION, ..file_state_of(db_path) };
    if unchanged(conn, db_path, &state) {
        return;
    }

    let path_str = db_path.to_string_lossy().to_string();
    let ro = match super::open_sqlite_artifact("antigravity", db_path) {
        Ok(c) => c,
        Err(e) => {
            result.error = Some(e);
            return;
        }
    };

    let session_id = db_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    let (created_ts, project) = read_trajectory_meta(&ro, &state);

    let blobs: Vec<Vec<u8>> = {
        let mut stmt = match ro.prepare("SELECT data FROM gen_metadata ORDER BY idx") {
            Ok(s) => s,
            Err(_) => return, // not a conversation db (table missing)
        };
        match stmt
            .query_map([], |r| r.get::<_, Vec<u8>>(0))
            .map(|rows| rows.flatten().collect())
        {
            Ok(b) => b,
            Err(_) => return,
        }
    };

    let mut events: Vec<UsageEvent> = Vec::new();
    for (idx, blob) in blobs.iter().enumerate() {
        match decode_generation(blob, idx, &session_id, created_ts, &project, &path_str) {
            Some(event) => {
                // A regenerated response can repeat a responseId within the
                // same conversation; first occurrence wins.
                if !events.iter().any(|e| e.dedup_key == event.dedup_key) {
                    events.push(event);
                }
            }
            None => result.lines_skipped += 1,
        }
    }

    // Nothing decoded is not the same as nothing consumed: a descriptor change
    // moves `usage` out from under the field numbers above, and the replace
    // below would delete Records this parser can no longer re-derive. Leaving
    // the db unstamped re-parses it next Scan.
    if events.is_empty() {
        return;
    }

    let n = events.len() as u64;
    if replace_file_events(conn, &path_str, &events).is_err() {
        result.error = Some(format!("failed to write events for {path_str}"));
        return;
    }
    result.events_inserted += n;
    let _ = set_file_state(conn, &path_str, state);
}

/// True when this export can stand in for its `.pb` — i.e. it parsed at a
/// schema we know. An export naming no generations still counts: "this Session
/// billed nothing" is an answer, and real installs do hold such Sessions, so
/// treating empty as failure would pin the ≥ on them for good.
///
/// Since TOKL-28 such an export still stands in, but is no longer *stamped*:
/// nothing on the wire separates a Session that billed nothing from one whose
/// counts were renamed away, so both are re-read on every Scan. The cost is a
/// small JSON re-parse per Scan, and a `lines_skipped` that recurs instead of
/// firing once; the alternative is deleting Records this parser can no longer
/// re-derive.
fn process_export(conn: &mut Connection, path: &Path, result: &mut SourceScanResult) -> bool {
    let state = FileState { byte_offset: PARSER_VERSION, ..file_state_of(path) };
    if unchanged(conn, path, &state) {
        // File state is only persisted after a successful parse, so an
        // unchanged export is one this Ledger has already accepted.
        return true;
    }
    let path_str = path.to_string_lossy().to_string();

    let export: ConversationExport = match fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<ConversationExport>(&raw).ok())
    {
        Some(export) if export.schema == export_artifact::SCHEMA => export,
        // A malformed instance of a *supported* shape: warn, per ADR-0015.
        _ => {
            result.error = Some(format!("antigravity: unreadable export {path_str}"));
            return false;
        }
    };

    let mut events: Vec<UsageEvent> = Vec::new();
    for (idx, generation) in export.generations.iter().enumerate() {
        if generation.input == 0
            && generation.output == 0
            && generation.cache_read == 0
            && generation.cache_write == 0
        {
            result.lines_skipped += 1;
            continue;
        }
        // Same key shape as the `.db` path, so a Session present as both an
        // export and a database can never be counted twice.
        let dedup_key = match generation.response_id.as_deref().filter(|r| !r.trim().is_empty()) {
            Some(rid) => format!("antigravity:{}:{rid}", export.conversation_id),
            None => format!("antigravity:{}:{idx}", export.conversation_id),
        };
        if events.iter().any(|e| e.dedup_key == dedup_key) {
            continue;
        }

        // The true Model of the request wins, then the Session-level picker
        // label (a fallback for exports that predate the per-generation
        // alias), then the enum.
        let resolved = |m: &Option<String>| {
            m.as_deref()
                .filter(|s| !s.trim().is_empty())
                .map(|s| resolve_model(s, generation.ts))
        };
        let model = resolved(&generation.model_alias)
            .or_else(|| resolved(&export.model))
            // A placeholder enum the server never paired with a name has no
            // published identity; surface the raw id so it lands in the
            // unpriced list instead of being guessed at.
            .or_else(|| {
                generation
                    .model_enum
                    .map(|e| resolve_model_enum(e, generation.ts)
                        .unwrap_or_else(|| format!("antigravity-model-{e}")))
            });

        events.push(UsageEvent {
            dedup_key,
            source: "antigravity".to_string(),
            timestamp: generation.ts,
            model,
            project: export.project.clone(),
            api_calls: 1,
            input_tokens: generation.input,
            output_tokens: generation.output,
            cache_read_tokens: generation.cache_read,
            cache_write_5m_tokens: generation.cache_write,
            cache_write_1h_tokens: 0,
            source_file: path_str.clone(),
            session_id: Some(export.conversation_id.clone()),
            reasoning_tokens: Some(generation.thinking),
            ctx: Default::default(),
        });
    }

    // Every count here is #[serde(default)], so a renamed field reads as zero
    // rather than failing the schema gate above — indistinguishable from a
    // Session that billed nothing, except that the replace below would delete
    // Records this parser can no longer re-derive. Still `true`: refusing to
    // write is not failing to read, and returning false would re-pin the ">="
    // on an export that is perfectly readable.
    if events.is_empty() {
        return true;
    }

    let n = events.len() as u64;
    if replace_file_events(conn, &path_str, &events).is_err() {
        result.error = Some(format!("failed to write events for {path_str}"));
        return false;
    }
    result.events_inserted += n;
    let _ = set_file_state(conn, &path_str, state);
    true
}

fn decode_generation(
    blob: &[u8],
    idx: usize,
    session_id: &str,
    created_ts: i64,
    project: &Option<String>,
    source_file: &str,
) -> Option<UsageEvent> {
    let chat_model = message_field(blob, 1)?;
    let usage = message_field(chat_model, 4)?;

    let to_i64 = |v: u64| i64::try_from(v).unwrap_or(i64::MAX);
    let input = to_i64(varint_field(usage, 2).unwrap_or(0));
    let cache_read = to_i64(varint_field(usage, 5).unwrap_or(0));
    let reasoning = to_i64(varint_field(usage, 9).unwrap_or(0));
    let response = to_i64(varint_field(usage, 10).unwrap_or(0));
    // #3 is the total output the API billed; #9 + #10 is that same number split.
    // Prefer the total and fall back to the parts, never sum all three.
    let output = match varint_field(usage, 3) {
        Some(total) => to_i64(total),
        None => reasoning.saturating_add(response),
    };
    if input == 0 && cache_read == 0 && output == 0 {
        return None;
    }

    let timestamp = message_field(chat_model, 9)
        .and_then(|gen| message_field(gen, 4))
        .and_then(proto_timestamp_secs)
        .filter(|&s| s > 0)
        .unwrap_or(created_ts);

    let model = resolve_model(
        string_field(chat_model, 19)
            .filter(|m| !m.trim().is_empty())
            .unwrap_or("unknown"),
        timestamp,
    );

    let dedup_key = string_field(usage, 11)
        .filter(|s| !s.trim().is_empty())
        .map(|rid| format!("antigravity:{session_id}:{rid}"))
        .unwrap_or_else(|| format!("antigravity:{session_id}:{idx}"));

    Some(UsageEvent {
        dedup_key,
        source: "antigravity".to_string(),
        timestamp,
        model: Some(model),
        project: project.clone(),
        api_calls: 1,
        input_tokens: input,
        output_tokens: output, // already total; `reasoning` is a subset of it
        cache_read_tokens: cache_read,
        cache_write_5m_tokens: 0, // Antigravity reports no cache-write side
        cache_write_1h_tokens: 0,
        source_file: source_file.to_string(),
        session_id: Some(session_id.to_string()),
        reasoning_tokens: Some(reasoning),
        ctx: Default::default(),
    })
}

// Conversation-level created-at (per-row fallback timestamp) and workspace.
fn read_trajectory_meta(ro: &Connection, state: &crate::types::FileState) -> (i64, Option<String>) {
    let blob: Option<Vec<u8>> = ro
        .query_row("SELECT data FROM trajectory_metadata_blob LIMIT 1", [], |r| r.get(0))
        .ok();

    let mut created = 0i64;
    let mut project = None;
    if let Some(blob) = &blob {
        created = message_field(blob, 2)
            .and_then(proto_timestamp_secs)
            .unwrap_or(0);
        project = message_field(blob, 1)
            .and_then(|folder| string_field(folder, 1))
            .and_then(file_uri_to_path);
    }
    if created <= 0 {
        created = state.mtime;
    }
    (created, project)
}

fn proto_timestamp_secs(ts: &[u8]) -> Option<i64> {
    i64::try_from(varint_field(ts, 1)?).ok()
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_db;
    use std::time::Duration;
    use tempfile::tempdir;

    // --- protobuf encoding helpers (tests only) ---
    fn varint(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
        out
    }

    fn f_varint(no: u64, v: u64) -> Vec<u8> {
        let mut out = varint(no << 3);
        out.extend(varint(v));
        out
    }

    fn f_len(no: u64, payload: &[u8]) -> Vec<u8> {
        let mut out = varint((no << 3) | 2);
        out.extend(varint(payload.len() as u64));
        out.extend_from_slice(payload);
        out
    }

    /// `model_enum` is `ModelUsageStats.#1` — an identifier Antigravity stores
    /// beside the counts, deliberately included so a regression that mistakes it
    /// for a token count shows up as inflated input.
    #[allow(clippy::too_many_arguments)]
    fn gen_blob(
        model: &str,
        ts_secs: i64,
        model_enum: u64,
        input: u64,
        cache_read: u64,
        reasoning: u64,
        response: u64,
        response_id: &str,
    ) -> Vec<u8> {
        let mut usage = Vec::new();
        usage.extend(f_varint(1, model_enum));
        usage.extend(f_varint(2, input));
        usage.extend(f_varint(5, cache_read));
        usage.extend(f_varint(9, reasoning));
        usage.extend(f_varint(10, response));
        usage.extend(f_len(11, response_id.as_bytes()));

        let ts = f_varint(1, ts_secs as u64);
        let gen_info = f_len(4, &ts);

        let mut chat_model = Vec::new();
        chat_model.extend(f_len(4, &usage));
        chat_model.extend(f_len(9, &gen_info));
        chat_model.extend(f_len(19, model.as_bytes()));

        f_len(1, &chat_model)
    }

    fn trajectory_blob(created_secs: i64, workspace_uri: &str) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(f_len(1, &f_len(1, workspace_uri.as_bytes())));
        out.extend(f_len(2, &f_varint(1, created_secs as u64)));
        out
    }

    fn build_db(path: &Path, gens: &[Vec<u8>], meta: Option<&[u8]>) {
        let db = Connection::open(path).unwrap();
        db.execute_batch(
            "CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB, size INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE trajectory_metadata_blob (id TEXT DEFAULT \"main\", data BLOB, PRIMARY KEY (id));",
        )
        .unwrap();
        for (i, g) in gens.iter().enumerate() {
            db.execute(
                "INSERT INTO gen_metadata (idx, data) VALUES (?1, ?2)",
                rusqlite::params![i as i64, g],
            )
            .unwrap();
        }
        if let Some(m) = meta {
            db.execute(
                "INSERT INTO trajectory_metadata_blob (id, data) VALUES ('main', ?1)",
                rusqlite::params![m],
            )
            .unwrap();
        }
    }

    #[test]
    fn decodes_generations_with_workspace_and_timestamps() {
        let convs = tempdir().unwrap();
        let db_path = convs.path().join("11111111-2222-3333-4444-555555555555.db");
        build_db(
            &db_path,
            &[
                gen_blob("gemini-3-flash-a", 1780300000, 1132, 500, 20000, 300, 150, "resp-1"),
                gen_blob("gemini-3-flash-a", 1780300060, 1132, 80, 21000, 100, 0, "resp-2"),
            ],
            Some(&trajectory_blob(1780299000, "file:///Users/dev/my%20app")),
        );

        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        let res = scan_antigravity(&mut conn, &[convs.path()]);
        assert!(res.error.is_none(), "{:?}", res.error);
        assert_eq!(res.events_inserted, 2);

        let (ts, model, project, input, output, cr, reasoning, sid): (
            i64, String, Option<String>, i64, i64, i64, Option<i64>, Option<String>,
        ) = conn
            .query_row(
                "SELECT timestamp, model, project, input_tokens, output_tokens,
                        cache_read_tokens, reasoning_tokens, session_id
                 FROM events WHERE dedup_key LIKE '%resp-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?,
                        r.get(5)?, r.get(6)?, r.get(7)?)),
            )
            .unwrap();
        assert_eq!(ts, 1780300000); // per-generation stamp, not created-at
        assert_eq!(model, "gemini-3.5-flash"); // wire alias resolved at parse time
        assert_eq!(project, Some("/Users/dev/my app".to_string())); // URI percent-decoded
        assert_eq!(input, 500); // #1 is the Model enum and must not reach input
        assert_eq!(output, 300 + 150); // #9 + #10, the API's total output
        assert_eq!(cr, 20000);
        assert_eq!(reasoning, Some(300)); // #9 is the thinking side, not #10
        assert_eq!(sid, Some("11111111-2222-3333-4444-555555555555".to_string()));
    }

    #[test]
    fn duplicate_response_ids_collapse_and_zero_rows_skip() {
        let convs = tempdir().unwrap();
        let db_path = convs.path().join("s.db");
        build_db(
            &db_path,
            &[
                gen_blob("m", 100, 0, 10, 0, 5, 0, "same"),
                gen_blob("m", 101, 0, 99, 0, 99, 0, "same"), // regeneration: same responseId
                gen_blob("m", 102, 0, 0, 0, 0, 0, "zeros"),  // no tokens at all
            ],
            None,
        );

        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        let res = scan_antigravity(&mut conn, &[convs.path()]);
        assert_eq!(res.events_inserted, 1);
        assert_eq!(res.lines_skipped, 1); // the all-zero row

        let (input,): (i64,) = conn
            .query_row("SELECT input_tokens FROM events", [], |r| Ok((r.get(0)?,)))
            .unwrap();
        assert_eq!(input, 10); // first occurrence wins
    }

    #[test]
    fn unchanged_db_is_skipped_and_growth_rescans() {
        let convs = tempdir().unwrap();
        let db_path = convs.path().join("s.db");
        build_db(&db_path, &[gen_blob("m", 100, 0, 10, 0, 5, 0, "r1")], None);

        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        assert_eq!(scan_antigravity(&mut conn, &[convs.path()]).events_inserted, 1);
        assert_eq!(scan_antigravity(&mut conn, &[convs.path()]).events_inserted, 0);

        // Conversation grows → whole-db reparse, replaced not duplicated.
        {
            let db = Connection::open(&db_path).unwrap();
            db.execute(
                "INSERT INTO gen_metadata (idx, data) VALUES (1, ?1)",
                rusqlite::params![gen_blob("m", 200, 0, 20, 0, 8, 0, "r2")],
            )
            .unwrap();
        }
        // SQLite may reuse pages (same size) within the same mtime second;
        // real scans are minutes apart, so simulate time passing.
        let f = std::fs::OpenOptions::new().write(true).open(&db_path).unwrap();
        f.set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
            .unwrap();
        let res = scan_antigravity(&mut conn, &[convs.path()]);
        assert_eq!(res.events_inserted, 2);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn wire_aliases_resolve_and_unknown_names_pass_through() {
        // Both flash placeholders are the same 3.5 Flash (High) catalog entry.
        assert_eq!(resolve_model("gemini-3-flash-a", 1780300000), "gemini-3.5-flash");
        assert_eq!(resolve_model("gemini-3-flash-b", 1780300000), "gemini-3.5-flash");
        assert_eq!(resolve_model("GEMINI-3-Flash-A", 1780300000), "gemini-3.5-flash");
        // The export picker labels: the IDE's 3-flash line is the 3.5 Flash
        // line, agent mode included; pro labels drop their thinking tier;
        // Claude thinking labels are the base Opus Models.
        assert_eq!(resolve_model("gemini-3-flash", 1780300000), "gemini-3.5-flash");
        assert_eq!(resolve_model("gemini-3-flash-agent", 1780300000), "gemini-3.5-flash");
        assert_eq!(resolve_model("gemini-3-flash-c", 1780300000), "gemini-3.5-flash");
        assert_eq!(resolve_model("gemini-3-pro-high", 1780300000), "gemini-3-pro-preview");
        assert_eq!(resolve_model("gemini-3-pro-low", 1780300000), "gemini-3-pro-preview");
        assert_eq!(resolve_model("gemini-3.1-pro-high", 1780300000), "gemini-3.1-pro-preview");
        assert_eq!(resolve_model("gemini-3.1-pro-low", 1780300000), "gemini-3.1-pro-preview");
        assert_eq!(resolve_model("claude-opus-4-5-thinking", 1780300000), "claude-opus-4-5");
        assert_eq!(resolve_model("claude-opus-4-6-thinking", 1780300000), "claude-opus-4-6");
        // gemini-default follows the era it was written in; boundaries are
        // exclusive upper bounds, so a row exactly on one is the later era.
        assert_eq!(resolve_model("gemini-default", 1742860799), "gemini-2.0-flash");
        assert_eq!(resolve_model("gemini-default", 1742860800), "gemini-2.5-flash");
        assert_eq!(resolve_model("gemini-default", 1779148799), "gemini-2.5-flash");
        assert_eq!(resolve_model("gemini-default", 1779148800), "gemini-3.5-flash");
        // Untouched: real ids, and any placeholder Antigravity renames next.
        assert_eq!(resolve_model("gemini-3.5-flash", 100), "gemini-3.5-flash");
        assert_eq!(resolve_model("gemini-3-flash-z", 100), "gemini-3-flash-z");
        assert_eq!(resolve_model("unknown", 100), "unknown");
    }

    // Enums the server paired one-to-one with an alias resolve even for
    // generations that carry no alias of their own; the default enum follows
    // its era; unpaired enums stay raw (the rename signal).
    #[test]
    fn paired_enums_resolve_and_unpaired_enums_stay_raw() {
        assert_eq!(resolve_model_enum(1037, 0).as_deref(), Some("gemini-3.1-pro-preview"));
        assert_eq!(resolve_model_enum(1036, 0).as_deref(), Some("gemini-3.1-pro-preview"));
        assert_eq!(resolve_model_enum(1018, 0).as_deref(), Some("gemini-3.5-flash"));
        assert_eq!(resolve_model_enum(1047, 0).as_deref(), Some("gemini-3.5-flash"));
        assert_eq!(resolve_model_enum(1026, 0).as_deref(), Some("claude-opus-4-6"));
        assert_eq!(resolve_model_enum(1035, 0).as_deref(), Some("claude-sonnet-4-6"));
        // Identified by explicit single-enum picker Sessions.
        assert_eq!(resolve_model_enum(1008, 0).as_deref(), Some("gemini-3-pro-preview"));
        assert_eq!(resolve_model_enum(1012, 0).as_deref(), Some("claude-opus-4-5"));
        // The default enum is era-based, exactly like the gemini-default alias.
        assert_eq!(resolve_model_enum(1084, 1742860799).as_deref(), Some("gemini-2.0-flash"));
        assert_eq!(resolve_model_enum(1084, 1780300000).as_deref(), Some("gemini-3.5-flash"));
        // Never paired, never explicit: no published identity, no guess.
        assert_eq!(resolve_model_enum(1007, 0), None);
    }

    #[test]
    fn a_parser_version_bump_reparses_an_otherwise_unchanged_db() {
        let convs = tempdir().unwrap();
        let db_path = convs.path().join("s.db");
        build_db(&db_path, &[gen_blob("gemini-default", 1780300000, 0, 10, 0, 5, 0, "r1")], None);

        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        assert_eq!(scan_antigravity(&mut conn, &[convs.path()]).events_inserted, 1);
        assert_eq!(scan_antigravity(&mut conn, &[convs.path()]).events_inserted, 0);

        // Rewind the stored version to pre-versioning, leaving size/mtime alone:
        // exactly the state a Ledger written by the previous parser is in.
        conn.execute(
            "UPDATE scanned_files SET byte_offset = 0 WHERE path = ?1",
            rusqlite::params![db_path.to_string_lossy()],
        )
        .unwrap();
        assert_eq!(scan_antigravity(&mut conn, &[convs.path()]).events_inserted, 1);
        // Re-parsed, not duplicated — and the row carries the resolved name.
        let (n, model): (i64, String) = conn
            .query_row("SELECT COUNT(*), MAX(model) FROM events", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(model, "gemini-3.5-flash");
        // And the bump is self-clearing: the next scan is quiet again.
        assert_eq!(scan_antigravity(&mut conn, &[convs.path()]).events_inserted, 0);
    }

    // Same contract on the export side: an export parsed by the previous
    // parser re-parses once the version mismatches, picking up new
    // resolutions without duplicating events.
    #[test]
    fn a_parser_version_bump_reparses_an_otherwise_unchanged_export() {
        let convs = tempdir().unwrap();
        std::fs::write(convs.path().join("s.pb"), b"\x99encrypted").unwrap();
        write_export(
            convs.path(),
            "s",
            r#"{"schema":1,"conversation_id":"s","generations":[
                 {"response_id":"r","ts":1780300000,"model_enum":1037,
                  "input":5,"output":1,"cache_read":0,"cache_write":0,"thinking":0}]}"#,
        );

        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        assert_eq!(scan_antigravity(&mut conn, &[convs.path()]).events_inserted, 1);
        assert_eq!(scan_antigravity(&mut conn, &[convs.path()]).events_inserted, 0);

        conn.execute(
            "UPDATE scanned_files SET byte_offset = 0 WHERE path = ?1",
            rusqlite::params![convs
                .path()
                .join(export_artifact::file_name("s"))
                .to_string_lossy()],
        )
        .unwrap();
        assert_eq!(scan_antigravity(&mut conn, &[convs.path()]).events_inserted, 1);
        let (n, model): (i64, String) = conn
            .query_row("SELECT COUNT(*), MAX(model) FROM events", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(model, "gemini-3.1-pro-preview");
        assert_eq!(scan_antigravity(&mut conn, &[convs.path()]).events_inserted, 0);
    }

    // .pb Sessions never parse and never warn — they are Unreadable Artifacts
    // (ADR-0017): counted, with the latest mtime kept so token totals can
    // carry the ≥ marker for windows their content could fall in.
    #[test]
    fn scans_multiple_roots_and_counts_pb_files_as_unreadable() {
        let ide = tempdir().unwrap();
        let cli = tempdir().unwrap();
        build_db(&ide.path().join("a.db"), &[gen_blob("m", 100, 0, 1, 0, 1, 0, "a")], None);
        build_db(&cli.path().join("b.db"), &[gen_blob("m", 100, 0, 2, 0, 2, 0, "b")], None);
        std::fs::write(ide.path().join("old.pb"), b"\x14\xae%\x8ca_encrypted").unwrap();
        std::fs::write(cli.path().join("new.pb"), b"\x99also_encrypted").unwrap();

        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        let res = scan_antigravity(&mut conn, &[ide.path(), cli.path()]);
        assert!(res.error.is_none());
        assert_eq!(res.events_inserted, 2);
        assert_eq!(res.artifacts_unreadable, 2);
        let expected = file_state_of(&ide.path().join("old.pb"))
            .mtime
            .max(file_state_of(&cli.path().join("new.pb")).mtime);
        assert_eq!(res.unreadable_max_mtime, Some(expected));
    }

    fn write_export(dir: &Path, id: &str, body: &str) {
        std::fs::write(dir.join(export_artifact::file_name(id)), body).unwrap();
    }

    // The whole point of the export path: a Session nothing could read offline
    // becomes ordinary events, and stops dragging the ≥ marker along with it.
    #[test]
    fn an_exported_pb_yields_events_and_stops_being_unreadable() {
        let convs = tempdir().unwrap();
        std::fs::write(convs.path().join("exported.pb"), b"\x99encrypted").unwrap();
        std::fs::write(convs.path().join("still-sealed.pb"), b"\x99encrypted").unwrap();
        write_export(
            convs.path(),
            "exported",
            r#"{"schema":1,"conversation_id":"exported","model":"gemini-3-flash-a",
                "project":"/Users/dev/app","generations":[
                  {"response_id":"r1","ts":1780300000,"input":500,"output":450,
                   "cache_read":20000,"cache_write":7,"thinking":300},
                  {"response_id":"r1","ts":1780300001,"input":9,"output":9,
                   "cache_read":0,"cache_write":0,"thinking":0},
                  {"response_id":"z","ts":1780300002,"input":0,"output":0,
                   "cache_read":0,"cache_write":0,"thinking":0}]}"#,
        );

        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        let res = scan_antigravity(&mut conn, &[convs.path()]);
        assert!(res.error.is_none(), "{:?}", res.error);
        assert_eq!(res.events_inserted, 1); // repeat responseId collapses
        assert_eq!(res.lines_skipped, 1); // the all-zero row
        assert_eq!(res.artifacts_unreadable, 1); // only the Session without an export

        let (ts, model, project, input, output, cr, cw, reasoning, sid): (
            i64, Option<String>, Option<String>, i64, i64, i64, i64, Option<i64>, Option<String>,
        ) = conn
            .query_row(
                "SELECT timestamp, model, project, input_tokens, output_tokens,
                        cache_read_tokens, cache_write_5m_tokens, reasoning_tokens, session_id
                 FROM events",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?,
                        r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?)),
            )
            .unwrap();
        assert_eq!(ts, 1780300000);
        assert_eq!(model.as_deref(), Some("gemini-3.5-flash")); // alias resolved, as in the .db path
        assert_eq!(project.as_deref(), Some("/Users/dev/app"));
        assert_eq!((input, output, cr, cw), (500, 450, 20000, 7));
        assert_eq!(reasoning, Some(300));
        assert_eq!(sid.as_deref(), Some("exported"));
    }

    // Both paths key on the responseId, so a Session that somehow arrives as a
    // database *and* an export contributes its generations exactly once.
    #[test]
    fn an_export_and_a_db_of_the_same_session_do_not_double_count() {
        let convs = tempdir().unwrap();
        build_db(
            &convs.path().join("dup.db"),
            &[gen_blob("m", 100, 1132, 10, 0, 3, 2, "shared")],
            None,
        );
        write_export(
            convs.path(),
            "dup",
            r#"{"schema":1,"conversation_id":"dup","generations":[
                 {"response_id":"shared","ts":100,"input":10,"output":5,
                  "cache_read":0,"cache_write":0,"thinking":3}]}"#,
        );

        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        let res = scan_antigravity(&mut conn, &[convs.path()]);
        assert!(res.error.is_none(), "{:?}", res.error);
        let events: i64 =
            conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0)).unwrap();
        assert_eq!(events, 1, "the shared responseId must collapse to one event");
    }

    // An export that cannot be read stands in for nothing. If the `.pb` stopped
    // counting merely because a file of the right *name* existed, its tokens
    // would leave the total with no ≥ to admit it — and the Decrypt button,
    // which keys off the unreadable count, would vanish exactly when it is the
    // remedy.
    #[test]
    fn an_unreadable_export_leaves_its_pb_unreadable() {
        let convs = tempdir().unwrap();
        std::fs::write(convs.path().join("s.pb"), b"\x99encrypted").unwrap();
        std::fs::write(convs.path().join("t.pb"), b"\x99encrypted").unwrap();
        write_export(convs.path(), "s", r#"{"schema":99,"conversation_id":"s","generations":[]}"#);
        write_export(convs.path(), "t", "{ not json");

        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        let res = scan_antigravity(&mut conn, &[convs.path()]);
        assert_eq!(res.events_inserted, 0);
        assert!(res.error.unwrap_or_default().contains("unreadable export"));
        assert_eq!(res.artifacts_unreadable, 2, "neither export can stand in for its .pb");
    }

    // The opposite trap: a Session that genuinely billed nothing exports an
    // empty list, and real installs hold such Sessions. Reading that as failure
    // would pin the ≥ on them permanently, with Decrypt offered forever and
    // never able to help.
    #[test]
    fn an_export_naming_no_generations_still_stands_in() {
        let convs = tempdir().unwrap();
        std::fs::write(convs.path().join("quiet.pb"), b"\x99encrypted").unwrap();
        write_export(
            convs.path(),
            "quiet",
            r#"{"schema":1,"conversation_id":"quiet","generations":[]}"#,
        );

        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        let res = scan_antigravity(&mut conn, &[convs.path()]);
        assert!(res.error.is_none(), "{:?}", res.error);
        assert_eq!(res.events_inserted, 0);
        assert_eq!(res.artifacts_unreadable, 0);

        // Since TOKL-28 an empty parse is left unstamped — nothing separates
        // this Session from one whose counts were renamed away — so it is
        // re-read on every Scan and must keep standing in each time.
        let export_path = convs.path().join(export_artifact::file_name("quiet"));
        assert!(
            crate::db::get_file_state(&conn, &export_path.to_string_lossy())
                .unwrap()
                .is_none(),
            "an empty parse must not be marked scanned"
        );
        let again = scan_antigravity(&mut conn, &[convs.path()]);
        assert_eq!(again.events_inserted, 0);
        assert_eq!(again.artifacts_unreadable, 0, "still stands in on the re-Scan");
    }

    // Antigravity keeps the same Session under several app data dirs, all of
    // which are scanned. Reading each copy would report inserts the Ledger
    // never made — the dedup key drops them — while the `.pb` in *either* dir
    // must still stop counting as unreadable.
    #[test]
    fn a_session_mirrored_across_app_dirs_is_read_once() {
        let ide = tempdir().unwrap();
        let app = tempdir().unwrap();
        let export = r#"{"schema":1,"conversation_id":"m","generations":[
             {"response_id":"r","ts":1780300000,"input":5,"output":1,
              "cache_read":0,"cache_write":0,"thinking":0}]}"#;
        for dir in [ide.path(), app.path()] {
            std::fs::write(dir.join("m.pb"), b"\x99encrypted").unwrap();
            write_export(dir, "m", export);
        }

        let ledger = tempdir().unwrap();
        let mut conn = open_db(&ledger.path().join("ledger.db")).unwrap();
        let res = scan_antigravity(&mut conn, &[ide.path(), app.path()]);
        assert!(res.error.is_none(), "{:?}", res.error);
        assert_eq!(res.events_inserted, 1, "the mirrored copy is not read again");
        assert_eq!(res.artifacts_unreadable, 0, "neither copy's .pb still counts");
        let events: i64 =
            conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0)).unwrap();
        assert_eq!(events, 1);
    }

    // An export in one app data dir vindicates the `.pb` in another, so the
    // verdict cannot be reached until every directory has been read.
    #[test]
    fn an_export_in_one_app_dir_covers_a_pb_in_another() {
        let with_pb = tempdir().unwrap();
        let with_export = tempdir().unwrap();
        std::fs::write(with_pb.path().join("split.pb"), b"\x99encrypted").unwrap();
        write_export(
            with_export.path(),
            "split",
            r#"{"schema":1,"conversation_id":"split","generations":[
                 {"response_id":"r","ts":1780300000,"input":5,"output":1,
                  "cache_read":0,"cache_write":0,"thinking":0}]}"#,
        );

        let ledger = tempdir().unwrap();
        let mut conn = open_db(&ledger.path().join("ledger.db")).unwrap();
        // `.pb` dir first, so a per-directory verdict would call it unreadable.
        let res = scan_antigravity(&mut conn, &[with_pb.path(), with_export.path()]);
        assert!(res.error.is_none(), "{:?}", res.error);
        assert_eq!(res.artifacts_unreadable, 0);
    }

    // A re-scan reads nothing (the file is unchanged) but must not forget that
    // the export was accepted, or the ≥ would flicker back on every scan.
    #[test]
    fn an_unchanged_export_still_stands_in_for_its_pb() {
        let convs = tempdir().unwrap();
        std::fs::write(convs.path().join("e.pb"), b"\x99encrypted").unwrap();
        write_export(
            convs.path(),
            "e",
            r#"{"schema":1,"conversation_id":"e","generations":[
                 {"response_id":"r","ts":1780300000,"input":5,"output":1,
                  "cache_read":0,"cache_write":0,"thinking":0}]}"#,
        );

        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        assert_eq!(scan_antigravity(&mut conn, &[convs.path()]).artifacts_unreadable, 0);

        let again = scan_antigravity(&mut conn, &[convs.path()]);
        assert_eq!(again.events_inserted, 0, "unchanged export is not re-read");
        assert_eq!(again.artifacts_unreadable, 0, "but it still stands in");
    }

    // Antigravity names most models only by a placeholder enum. Surfacing the
    // raw id keeps it unpriced and visible instead of silently mislabelled.
    #[test]
    fn a_nameless_model_surfaces_its_placeholder_id() {
        let convs = tempdir().unwrap();
        write_export(
            convs.path(),
            "n",
            r#"{"schema":1,"conversation_id":"n","generations":[
                 {"response_id":"r","ts":1780300000,"model_enum":1007,
                  "input":5,"output":1,"cache_read":0,"cache_write":0,"thinking":0}]}"#,
        );

        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        let res = scan_antigravity(&mut conn, &[convs.path()]);
        assert!(res.error.is_none(), "{:?}", res.error);
        let model: Option<String> =
            conn.query_row("SELECT model FROM events", [], |r| r.get(0)).unwrap();
        assert_eq!(model.as_deref(), Some("antigravity-model-1007"));
    }

    // The per-generation wire alias is the true Model; the Session-level
    // picker label is only the default. An export carrying both must book the
    // resolved alias, not the label.
    #[test]
    fn a_per_generation_alias_beats_the_session_label() {
        let convs = tempdir().unwrap();
        write_export(
            convs.path(),
            "a",
            r#"{"schema":1,"conversation_id":"a","model":"gemini-3-pro-high",
                "generations":[
                 {"response_id":"r","ts":1780300000,"model_enum":1020,
                  "model_alias":"gemini-3-flash-b",
                  "input":5,"output":1,"cache_read":0,"cache_write":0,"thinking":0}]}"#,
        );

        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        let res = scan_antigravity(&mut conn, &[convs.path()]);
        assert!(res.error.is_none(), "{:?}", res.error);
        let model: Option<String> =
            conn.query_row("SELECT model FROM events", [], |r| r.get(0)).unwrap();
        assert_eq!(model.as_deref(), Some("gemini-3.5-flash"));
    }

    // `#3` is the total the API billed; trusting the parts as well would double
    // the output of every generation that carries all three.
    #[test]
    fn a_total_output_field_wins_over_its_parts() {
        let mut usage = Vec::new();
        usage.extend(f_varint(1, 1132)); // Model enum
        usage.extend(f_varint(2, 40));
        usage.extend(f_varint(3, 90)); // total
        usage.extend(f_varint(9, 60)); // thinking
        usage.extend(f_varint(10, 30)); // response
        usage.extend(f_len(11, b"total"));
        let mut chat_model = Vec::new();
        chat_model.extend(f_len(4, &usage));
        chat_model.extend(f_len(9, &f_len(4, &f_varint(1, 1780300000))));
        chat_model.extend(f_len(19, b"m"));
        let blob = f_len(1, &chat_model);

        let convs = tempdir().unwrap();
        build_db(&convs.path().join("t.db"), &[blob], None);
        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        let res = scan_antigravity(&mut conn, &[convs.path()]);
        assert!(res.error.is_none(), "{:?}", res.error);
        let (input, output, reasoning): (i64, i64, Option<i64>) = conn
            .query_row("SELECT input_tokens, output_tokens, reasoning_tokens FROM events", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(input, 40);
        assert_eq!(output, 90, "#3, not #3 + #9 + #10");
        assert_eq!(reasoning, Some(60));
    }

    #[test]
    fn missing_roots_are_quiet() {
        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        let res = scan_antigravity(&mut conn, &[Path::new("/nonexistent/conversations")]);
        assert_eq!(res.events_inserted, 0);
        assert!(res.error.is_none());
    }

    /// `usage` under a field number the reader does not look at — the protobuf
    /// form of a rename, and what a descriptor change really looks like on the
    /// wire. Everything else about the generation is unchanged.
    fn gen_blob_usage_moved(model: &str, ts_secs: i64, input: u64, response: u64) -> Vec<u8> {
        let mut usage = Vec::new();
        usage.extend(f_varint(2, input));
        usage.extend(f_varint(10, response));

        let mut chat_model = Vec::new();
        chat_model.extend(f_len(6, &usage)); // was #4
        chat_model.extend(f_len(9, &f_len(4, &f_varint(1, ts_secs as u64))));
        chat_model.extend(f_len(19, model.as_bytes()));

        f_len(1, &chat_model)
    }

    // TOKL-28, the `.db` path: a conversation whose usage moved decodes to zero
    // generations. The Records it already booked must survive, and the db must
    // stay unstamped so the next Scan retries it.
    #[test]
    fn moved_usage_field_keeps_booked_db_records() {
        let convs = tempdir().unwrap();
        let db_path = convs.path().join("s.db");
        let booked_gen = gen_blob("gemini-3-flash-a", 1780300000, 1132, 500, 20000, 300, 150, "r1");
        build_db(&db_path, &[booked_gen], None);

        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        assert_eq!(scan_antigravity(&mut conn, &[convs.path()]).events_inserted, 1);
        let booked = file_state_of(&db_path);

        std::fs::remove_file(&db_path).unwrap();
        build_db(&db_path, &[gen_blob_usage_moved("gemini-3-flash-a", 1780300000, 500, 150)], None);
        let moved = file_state_of(&db_path);
        assert!(
            moved.size != booked.size || moved.mtime != booked.mtime,
            "db must look changed, or unchanged() skips it and this test proves nothing"
        );

        let res = scan_antigravity(&mut conn, &[convs.path()]);
        assert_eq!(res.events_inserted, 0);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0)).unwrap(),
            1,
            "the empty decode must not delete Records this parser can no longer re-derive"
        );

        let state = crate::db::get_file_state(&conn, &db_path.to_string_lossy()).unwrap().unwrap();
        assert_eq!(state.size, booked.size, "empty decode must not be marked scanned");
    }

    // TOKL-28, the export path. Every field on GenerationExport is
    // #[serde(default)], so a renamed count reads as zero rather than failing
    // the schema gate — it sails through as a Session that billed nothing.
    #[test]
    fn renamed_export_counts_keep_booked_records_and_still_stand_in() {
        let convs = tempdir().unwrap();
        std::fs::write(convs.path().join("exported.pb"), b"\x99encrypted").unwrap();
        let booked = r#"{"schema":1,"conversation_id":"exported","model":"gemini-3-flash-a",
                "project":"/Users/dev/app","generations":[
                  {"response_id":"r1","ts":1780300000,"input":500,"output":450,
                   "cache_read":20000,"cache_write":7,"thinking":300}]}"#;
        write_export(convs.path(), "exported", booked);

        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        let first = scan_antigravity(&mut conn, &[convs.path()]);
        assert_eq!(first.events_inserted, 1);
        assert_eq!(first.artifacts_unreadable, 0);
        let export_path = convs.path().join(export_artifact::file_name("exported"));
        let stamped = file_state_of(&export_path);

        // The companion renames its count fields; the reader defaults them to 0.
        let renamed = booked
            .replace("\"input\":", "\"input_tokens\":")
            .replace("\"output\":", "\"output_tokens\":")
            .replace("\"cache_read\":", "\"cache_read_tokens\":")
            .replace("\"cache_write\":", "\"cache_write_tokens\":");
        assert_ne!(renamed.len(), booked.len(), "size must differ, or unchanged() skips the file");
        write_export(convs.path(), "exported", &renamed);

        let res = scan_antigravity(&mut conn, &[convs.path()]);
        assert_eq!(res.events_inserted, 0);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0)).unwrap(),
            1,
            "the empty parse must not delete Records this parser can no longer re-derive"
        );
        // The export still stands in for its `.pb`: refusing to book from it is
        // not the same as failing to read it, and treating it as a failure would
        // re-pin the >= marker on this Session.
        assert_eq!(
            res.artifacts_unreadable, 0,
            "guarding the write must not un-stand-in the export"
        );

        let state =
            crate::db::get_file_state(&conn, &export_path.to_string_lossy()).unwrap().unwrap();
        assert_eq!(state.size, stamped.size, "empty parse must not be marked scanned");
    }
}
