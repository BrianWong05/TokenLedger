// TokenLedger — Google Antigravity adapter.
//
// Antigravity (IDE agent and CLI) stores each conversation as a SQLite
// database: `~/.gemini/antigravity/conversations/<uuid>.db` (IDE) and
// `~/.gemini/antigravity-cli/conversations/<uuid>.db` (CLI), same schema.
// Sibling `<uuid>.pb` files are encrypted legacy conversations only the
// live language server can decrypt — skipped.
//
// Each `gen_metadata` row is one generation (one API call) encoded as a
// protobuf blob. There is no published .proto; field numbers below follow
// tokscale's reverse engineering (verified against this machine's real
// databases: output #9 + thinking #10 = the API's total output):
//
//   gen_metadata.#1 (chatModel)
//     .#19 (string)             → model id (e.g. "gemini-3-flash-a")
//     .#9.#4 = {#1 sec, #2 ns}  → per-generation wall-clock timestamp
//     .#4 (usage)
//       .#1 (varint)            → fixed system-prompt input tokens
//       .#2 (varint)            → newly-processed (non-cached) input tokens
//       .#5 (varint)            → cache-read tokens
//       .#9 (varint)            → output text tokens
//       .#10 (varint)           → thinking/reasoning tokens
//       .#11 (string)           → responseId (dedup key)
//   trajectory_metadata_blob.#2 = {#1 sec}    → conversation created-at
//   trajectory_metadata_blob.#1.#1 (string)   → workspace file:// URI
use std::fs;
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use super::{file_state_of, percent_decode, unchanged};
use crate::db::{replace_file_events, set_file_state};
use crate::types::{SourceScanResult, UsageEvent};

pub fn scan_antigravity(conn: &mut Connection, roots: &[&Path]) -> SourceScanResult {
    let mut result = SourceScanResult::default();
    for root in roots {
        let entries = match fs::read_dir(root) {
            Ok(rd) => rd,
            Err(_) => continue, // missing dir → zero events, no error
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("db") {
                process_db(conn, &path, &mut result);
            }
        }
    }
    result
}

fn process_db(conn: &mut Connection, db_path: &Path, result: &mut SourceScanResult) {
    let state = file_state_of(db_path);
    if unchanged(conn, db_path, &state) {
        return;
    }

    let path_str = db_path.to_string_lossy().to_string();
    let uri = format!("file:{}?mode=ro", db_path.display());
    let ro = match Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    ) {
        Ok(c) => c,
        Err(e) => {
            result.error = Some(format!("antigravity: open failed: {e}"));
            return;
        }
    };
    let _ = ro.busy_timeout(Duration::from_millis(5000));

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

    let n = events.len() as u64;
    if replace_file_events(conn, &path_str, &events).is_err() {
        result.error = Some(format!("failed to write events for {path_str}"));
        return;
    }
    result.events_inserted += n;
    let _ = set_file_state(conn, &path_str, state);
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
    let system = to_i64(varint_field(usage, 1).unwrap_or(0));
    let input = system.saturating_add(to_i64(varint_field(usage, 2).unwrap_or(0)));
    let cache_read = to_i64(varint_field(usage, 5).unwrap_or(0));
    let output = to_i64(varint_field(usage, 9).unwrap_or(0));
    let reasoning = to_i64(varint_field(usage, 10).unwrap_or(0));
    if input == 0 && cache_read == 0 && output == 0 && reasoning == 0 {
        return None;
    }

    let timestamp = message_field(chat_model, 9)
        .and_then(|gen| message_field(gen, 4))
        .and_then(proto_timestamp_secs)
        .filter(|&s| s > 0)
        .unwrap_or(created_ts);

    let model = string_field(chat_model, 19)
        .filter(|m| !m.trim().is_empty())
        .unwrap_or("unknown")
        .to_string();

    let dedup_key = string_field(usage, 11)
        .filter(|s| !s.trim().is_empty())
        .map(|rid| format!("antigravity:{session_id}:{rid}"))
        .unwrap_or_else(|| format!("antigravity:{session_id}:{idx}"));

    Some(UsageEvent {
        dedup_key,
        source: "antigravity".to_string(),
        timestamp,
        model,
        project: project.clone(),
        api_calls: 1,
        input_tokens: input,
        output_tokens: output + reasoning, // reasoning folds into output
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
        // ponytail: macOS-shaped file:///abs/path URIs only; add Windows
        // drive/UNC handling if this ever runs elsewhere.
        project = message_field(blob, 1)
            .and_then(|folder| string_field(folder, 1))
            .and_then(|uri| uri.strip_prefix("file://"))
            .map(percent_decode)
            .filter(|p| p.starts_with('/'));
    }
    if created <= 0 {
        created = state.mtime;
    }
    (created, project)
}

fn proto_timestamp_secs(ts: &[u8]) -> Option<i64> {
    i64::try_from(varint_field(ts, 1)?).ok()
}

// ---------------------------------------------------------------------------
// Minimal protobuf wire-format reader (no schema dependency). Malformed data
// degrades to None, never panics.
// ---------------------------------------------------------------------------

fn read_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut result: u64 = 0;
    let mut shift = 0u32;
    while *pos < buf.len() {
        let byte = buf[*pos];
        *pos += 1;
        if shift >= 64 {
            return None;
        }
        result |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Some(result);
        }
        shift += 7;
    }
    None
}

/// Iterate top-level fields of one message, returning the first match of
/// `field_no` interpreted per `want_len` (LEN payload vs varint value).
fn find_field(buf: &[u8], field_no: u64, want_len: bool) -> Option<(u64, &[u8])> {
    let mut pos = 0usize;
    while pos < buf.len() {
        let key = read_varint(buf, &mut pos)?;
        let (no, wire) = (key >> 3, key & 7);
        match wire {
            0 => {
                let v = read_varint(buf, &mut pos)?;
                if no == field_no && !want_len {
                    return Some((v, &[]));
                }
            }
            1 => pos = pos.checked_add(8)?,
            2 => {
                let len = usize::try_from(read_varint(buf, &mut pos)?).ok()?;
                let end = pos.checked_add(len)?;
                if end > buf.len() {
                    return None;
                }
                if no == field_no && want_len {
                    return Some((0, &buf[pos..end]));
                }
                pos = end;
            }
            5 => pos = pos.checked_add(4)?,
            _ => return None, // groups/unknown wire types: bail
        }
    }
    None
}

fn varint_field(buf: &[u8], field_no: u64) -> Option<u64> {
    find_field(buf, field_no, false).map(|(v, _)| v)
}

fn message_field(buf: &[u8], field_no: u64) -> Option<&[u8]> {
    find_field(buf, field_no, true).map(|(_, b)| b)
}

fn string_field(buf: &[u8], field_no: u64) -> Option<&str> {
    message_field(buf, field_no).and_then(|b| std::str::from_utf8(b).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_db;
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

    fn gen_blob(
        model: &str,
        ts_secs: i64,
        sys: u64,
        input: u64,
        cache_read: u64,
        output: u64,
        thinking: u64,
        response_id: &str,
    ) -> Vec<u8> {
        let mut usage = Vec::new();
        usage.extend(f_varint(1, sys));
        usage.extend(f_varint(2, input));
        usage.extend(f_varint(5, cache_read));
        usage.extend(f_varint(9, output));
        usage.extend(f_varint(10, thinking));
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
        assert_eq!(model, "gemini-3-flash-a");
        assert_eq!(project, Some("/Users/dev/my app".to_string())); // URI percent-decoded
        assert_eq!(input, 1132 + 500); // system prompt + fresh input
        assert_eq!(output, 300 + 150); // thinking folds into output
        assert_eq!(cr, 20000);
        assert_eq!(reasoning, Some(150));
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
    fn scans_multiple_roots_and_ignores_pb_files() {
        let ide = tempdir().unwrap();
        let cli = tempdir().unwrap();
        build_db(&ide.path().join("a.db"), &[gen_blob("m", 100, 0, 1, 0, 1, 0, "a")], None);
        build_db(&cli.path().join("b.db"), &[gen_blob("m", 100, 0, 2, 0, 2, 0, "b")], None);
        std::fs::write(ide.path().join("old.pb"), b"\x14\xae%\x8ca_encrypted").unwrap();

        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        let res = scan_antigravity(&mut conn, &[ide.path(), cli.path()]);
        assert!(res.error.is_none());
        assert_eq!(res.events_inserted, 2);
    }

    #[test]
    fn missing_roots_are_quiet() {
        let app = tempdir().unwrap();
        let mut conn = open_db(&app.path().join("ledger.db")).unwrap();
        let res = scan_antigravity(&mut conn, &[Path::new("/nonexistent/conversations")]);
        assert_eq!(res.events_inserted, 0);
        assert!(res.error.is_none());
    }
}
