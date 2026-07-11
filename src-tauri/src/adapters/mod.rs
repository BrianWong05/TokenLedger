pub mod antigravity;
pub mod claude;
pub mod claude_ctx;
pub mod codex;
pub mod exec_class;
pub mod gemini;
pub mod grok;
pub mod hermes;

use std::path::Path;

use crate::db::get_file_state;
use crate::types::FileState;

// Shared by the grok/antigravity adapters' whole-file skip check.
pub(crate) fn file_state_of(path: &Path) -> FileState {
    let meta = std::fs::metadata(path).ok();
    FileState {
        size: meta.as_ref().map(|m| m.len() as i64).unwrap_or(0),
        mtime: meta
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        byte_offset: 0,
    }
}

pub(crate) fn unchanged(
    conn: &rusqlite::Connection,
    path: &Path,
    current: &FileState,
) -> bool {
    match get_file_state(conn, &path.to_string_lossy()) {
        Ok(Some(prev)) => prev.size == current.size && prev.mtime == current.mtime,
        _ => current.size == 0 && current.mtime == 0, // no state: only a missing file is "unchanged"
    }
}

pub(crate) fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
