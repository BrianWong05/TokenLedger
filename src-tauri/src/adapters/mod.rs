pub mod antigravity;
pub mod claude;
pub mod claude_ctx;
pub mod cline;
pub mod codex;
pub mod ctx;
pub mod exec_class;
pub mod gemini;
pub mod goose;
pub mod grok;
pub mod hermes;
pub mod kilo;
pub mod opencode;
pub mod pi;
pub mod zed;

use std::path::{Path, PathBuf};

use crate::db::{get_file_state, upsert_events};
use crate::types::{FileState, UsageEvent};

pub(crate) fn upsert_events_count(
    conn: &mut rusqlite::Connection,
    events: &[UsageEvent],
) -> rusqlite::Result<u64> {
    let mut seen = std::collections::HashSet::new();
    let mut inserted = 0;
    for event in events {
        if !seen.insert(event.dedup_key.as_str()) {
            continue;
        }
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE dedup_key = ?1)",
            [&event.dedup_key],
            |row| row.get(0),
        )?;
        if !exists {
            inserted += 1;
        }
    }
    upsert_events(conn, events)?;
    Ok(inserted)
}

pub(crate) fn find_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    if dir.is_file() {
        if dir.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            out.push(dir.to_path_buf());
        }
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            find_jsonl(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

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

/// Normalize an epoch value a Source writer may store in seconds,
/// milliseconds, or microseconds into epoch seconds. Shared by the adapters
/// whose writers emit timestamps in more than one unit; callers that require a
/// strictly positive timestamp keep their own guard.
pub(crate) fn normalize_epoch(value: i64) -> i64 {
    let magnitude = value.unsigned_abs();
    if magnitude >= 1_000_000_000_000_000 {
        value / 1_000_000
    } else if magnitude >= 1_000_000_000_000 {
        value / 1_000
    } else {
        value
    }
}

pub(crate) fn unchanged(
    conn: &rusqlite::Connection,
    path: &Path,
    current: &FileState,
) -> bool {
    match get_file_state(conn, &path.to_string_lossy()) {
        // byte_offset carries the caller's parser version (0 where unused), so
        // bumping it re-parses files whose size/mtime never changed.
        Ok(Some(prev)) => {
            prev.size == current.size
                && prev.mtime == current.mtime
                && prev.byte_offset == current.byte_offset
        }
        _ => current.size == 0 && current.mtime == 0, // no state: only a missing file is "unchanged"
    }
}

/// Finds `segment`, spelt POSIX-style, in a path that came out of a log —
/// whichever separator the machine that wrote that log happens to use. A
/// Windows-written log spells `/.claude/worktrees/` as `\.claude\worktrees\`,
/// and looking only for the forward-slash spelling silently never fires there.
/// The index returned is into `path` itself: trading one ASCII byte for another
/// of the same width moves nothing.
/// ponytail: normalizes a copy per call, which is noise next to the JSON parse
/// that produced the path. Compare separators in place if a scan ever says
/// otherwise.
pub(crate) fn find_segment(path: &str, segment: &str) -> Option<usize> {
    path.replace('\\', "/").find(segment)
}

pub(crate) fn rollup_worktree(cwd: &str) -> String {
    match find_segment(cwd, "/.claude/worktrees/") {
        Some(i) => cwd[..i].to_string(),
        None => cwd.to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    // A cwd is copied out of a log, so it is spelt the way the machine that
    // wrote it spells paths — which need not be the one now reading it.
    #[test]
    fn a_worktree_rolls_up_in_either_flavour_of_path() {
        assert_eq!(
            rollup_worktree("/Users/dev/repo/.claude/worktrees/feat-x"),
            "/Users/dev/repo"
        );
        assert_eq!(
            rollup_worktree(r"C:\Users\dev\repo\.claude\worktrees\feat-x"),
            r"C:\Users\dev\repo"
        );
    }

    #[test]
    fn a_cwd_outside_a_worktree_is_left_alone() {
        assert_eq!(rollup_worktree("/Users/dev/repo"), "/Users/dev/repo");
        assert_eq!(rollup_worktree(r"C:\Users\dev\repo"), r"C:\Users\dev\repo");
        // ".claude/worktrees" as a plain directory name, not the marker path
        assert_eq!(rollup_worktree("/Users/dev/worktrees"), "/Users/dev/worktrees");
    }
}
