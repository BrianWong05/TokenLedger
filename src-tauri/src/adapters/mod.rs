pub mod antigravity;
pub mod claude;
pub mod claude_ctx;
pub mod cline;
pub mod codebuddy;
pub mod codex;
pub mod copilot;
pub mod ctx;
pub mod exec_class;
pub mod gemini;
pub mod goose;
pub mod grok;
pub mod hermes;
pub mod kilo;
pub mod opencode;
pub mod omp;
pub mod pi;
pub mod qoder;
pub mod workbuddy;
pub mod zed;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::db::{get_file_state, set_file_state, upsert_events};
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
    find_jsonl_inner(dir, out);
}

pub(crate) fn find_jsonl_by_file_identity(
    dir: &Path,
    out: &mut Vec<PathBuf>,
    aliases: &mut Vec<PathBuf>,
    seen: &mut HashSet<FileIdentity>,
) {
    let mut files = Vec::new();
    find_jsonl(dir, &mut files);
    // A symlink and its target share one identity, so only their order decides
    // which spelling wins — and the winner's file stem keys every event the
    // rollout mints (codex.rs:287), so a change of winner mints a permanent
    // second copy of the Session. Sort links last so the real file always wins
    // and the key cannot move when someone drops a link beside it.
    // Decorated so the flag costs one `symlink_metadata` per path rather than
    // one per comparison: this pass runs on every scan, including the no-op
    // ones where nothing is re-parsed.
    let mut files: Vec<(bool, PathBuf)> = files.into_iter().map(|p| (p.is_symlink(), p)).collect();
    files.sort();
    for (_, path) in files {
        // Insert outside the match guard: a guard only borrows what it binds,
        // and FileIdentity is a PathBuf off unix, so moving it here would not
        // compile there (E0507).
        match file_identity(&path) {
            Ok(identity) => {
                if seen.insert(identity) {
                    out.push(path);
                } else {
                    aliases.push(path);
                }
            }
            // An unreadable identity is never an alias: a transient stat
            // failure must not send a real rollout through the cleanup loop
            // that erases its scan state and Context.
            Err(_) => out.push(path),
        }
    }
}

#[cfg(unix)]
pub(crate) type FileIdentity = (u64, u64);
#[cfg(not(unix))]
pub(crate) type FileIdentity = PathBuf;

#[cfg(unix)]
fn file_identity(path: &Path) -> std::io::Result<FileIdentity> {
    use std::os::unix::fs::MetadataExt;

    let metadata = path.metadata()?;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn file_identity(path: &Path) -> std::io::Result<FileIdentity> {
    // ponytail: canonical paths cannot collapse Windows hard links; switch to
    // stable file IDs when Rust exposes them without an unstable API.
    path.canonicalize()
}

fn find_jsonl_inner(dir: &Path, out: &mut Vec<PathBuf>) {
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
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            find_jsonl_inner(&path, out);
        } else if path.is_file()
            && path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
        {
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
            .map(|d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
            .unwrap_or(0),
        byte_offset: 0,
    }
}

pub(crate) fn sqlite_file_states(
    path: &Path,
    parser_version: i64,
) -> [(PathBuf, FileState); 3] {
    let sidecar = |suffix: &str| {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        PathBuf::from(name)
    };
    [path.to_path_buf(), sidecar("-wal"), sidecar("-shm")].map(|path| {
        let mut state = file_state_of(&path);
        state.byte_offset = parser_version;
        (path, state)
    })
}

pub(crate) fn remember_file_states(
    conn: &rusqlite::Connection,
    states: &[(PathBuf, FileState)],
) -> rusqlite::Result<()> {
    for (path, state) in states {
        if path.is_file() {
            set_file_state(conn, &path.to_string_lossy(), *state)?;
        }
    }
    Ok(())
}

/// A readable handle on a Source's SQLite Artifact, or a Source-named
/// refusal. One place decides how the scan reads a live database another
/// program may be writing: read-only flags (a Scan only ever reads, ADR-0013),
/// a five-second busy wait so a Source mid-write means waiting, not failing,
/// and one failure wording (it had drifted five ways across the adapters, one
/// of which dropped the error entirely). The wait is stated here even though
/// rusqlite happens to default every connection to the same five seconds
/// (inner_connection.rs, sqlite3_busy_timeout(db, 5000)): eight adapters
/// restated that default by hand and one (workbuddy) leaned on it without
/// knowing — a library upgrade must not silently change how long a scan
/// waits. A plain path (not a `file:` URI) keeps Windows verbatim temp
/// paths, which can carry a `\\?\` prefix, working in both production and
/// tests. Parsing, skip strategy and persistence stay per-Source (ADR-0004)
/// — this is the read-side shell they all shared.
pub(crate) fn open_sqlite_artifact(
    source: &str,
    path: &Path,
) -> Result<rusqlite::Connection, String> {
    open_sqlite_artifact_waiting(source, path, std::time::Duration::from_secs(5))
}

/// The timeout-parameterised core — an internal seam so the contention test
/// need not sit out the production five seconds.
fn open_sqlite_artifact_waiting(
    source: &str,
    path: &Path,
    wait: std::time::Duration,
) -> Result<rusqlite::Connection, String> {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| format!("{source}: database open failed: {error}"))?;
    let _ = conn.busy_timeout(wait);
    Ok(conn)
}

/// Refuses an Artifact whose table lacks the columns the parse is about to
/// trust, naming the Source — the check every SQLite adapter runs between
/// opening and querying, so "this Source changed its schema" reads as a
/// malformed-Artifact warning instead of a parse mystery. Returns the table's
/// actual column set: one caller (Zed) branches on an optional column, and
/// the set is already in hand. A missing table yields an empty set from
/// PRAGMA table_info, so it refuses the same way a missing column does.
pub(crate) fn require_columns(
    source: &str,
    conn: &rusqlite::Connection,
    table: &str,
    columns: &[&str],
) -> Result<HashSet<String>, String> {
    let mut statement = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|error| format!("{source}: schema inspection failed: {error}"))?;
    let found = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| format!("{source}: schema inspection failed: {error}"))?
        .collect::<rusqlite::Result<HashSet<_>>>()
        .map_err(|error| format!("{source}: schema inspection failed: {error}"))?;
    if !columns.iter().all(|column| found.contains(*column)) {
        return Err(format!("{source}: unsupported database schema"));
    }
    Ok(found)
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

/// A project path is kept when it is rooted: absolute on this platform, or
/// POSIX-rooted (`/…`) the way the deterministic fixtures and several writers
/// spell it. On Windows a drive-rooted path is absolute; a POSIX-rooted one is
/// root-relative but still concrete, so it is kept rather than dropped.
/// Relative paths stay `None` rather than being guessed.
pub(crate) fn is_absolute_path(path: &str) -> bool {
    Path::new(path).is_absolute() || path.starts_with('/')
}

pub(crate) fn absolute_project(project: Option<&str>) -> Option<String> {
    let project = project?.trim();
    (!project.is_empty() && is_absolute_path(project)).then(|| project.to_string())
}

pub(crate) fn unchanged(conn: &rusqlite::Connection, path: &Path, current: &FileState) -> bool {
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

pub(crate) use crate::uri::percent_decode;

pub(crate) struct ClaudeShapedUsage {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write_5m: i64,
    pub cache_write_1h: i64,
}

/// One API call made inside a single Claude-shaped assistant `message`.
pub(crate) struct ClaudeCall {
    /// The iteration's own Model, when it reported one. None means it reported
    /// none, and the message's own `model` is the right answer.
    pub model: Option<String>,
    /// The call's position in `usage.iterations`. Positions come from the
    /// unfiltered array: an all-zero iteration books no Record, and the
    /// survivors keep the slot they were logged in rather than closing the gap.
    pub index: usize,
    pub usage: ClaudeShapedUsage,
}

/// What one Claude-shaped assistant `message` books. The distinction is
/// all-or-nothing per message, so it lives in the type rather than in an
/// Option every caller has to re-test.
pub(crate) enum ClaudeCalls {
    /// No Usage Record: an all-zero observation is not one.
    Nothing,
    /// One Record from the top-level figures, keyed as it always was.
    OneMessage(ClaudeShapedUsage),
    /// One Record per API call, each under its own Model and a suffixed key.
    /// Never empty.
    PerCall(Vec<ClaudeCall>),
}

/// Token figures from a Claude-Code-shaped assistant `message` object.
/// `input_tokens` is fresh input (cache reads and writes are separate fields);
/// `cache_creation_input_tokens` splits into the ephemeral 5m/1h buckets when
/// the `cache_creation` sub-object is present, and books whole to the 5m
/// bucket when it is absent. An all-zero observation (e.g. a `<synthetic>`
/// error placeholder) is not a Usage Record and returns None. Shared by every
/// Source whose writer emits this exact shape (ADR-0016 ethos: identical
/// shapes share one parsing rule).
pub(crate) fn claude_shaped_usage(message: &serde_json::Value) -> Option<ClaudeShapedUsage> {
    claude_shaped_buckets(&message["usage"])
}

/// The bucket math for one `usage`-shaped object — the top-level one, or a
/// single `iterations` entry, which carry the identical field set.
fn claude_shaped_buckets(usage: &serde_json::Value) -> Option<ClaudeShapedUsage> {
    let input = usage["input_tokens"].as_i64().unwrap_or(0);
    let output = usage["output_tokens"].as_i64().unwrap_or(0);
    let cache_read = usage["cache_read_input_tokens"].as_i64().unwrap_or(0);
    let cc_total = usage["cache_creation_input_tokens"].as_i64().unwrap_or(0);
    let cc = &usage["cache_creation"];
    let (cache_write_5m, cache_write_1h) = if cc.is_object() {
        (
            cc["ephemeral_5m_input_tokens"].as_i64().unwrap_or(0),
            cc["ephemeral_1h_input_tokens"].as_i64().unwrap_or(0),
        )
    } else {
        // sub-object absent: whole creation total is 5m-TTL
        (cc_total, 0)
    };

    if input == 0 && output == 0 && cache_read == 0 && cache_write_5m == 0 && cache_write_1h == 0 {
        return None;
    }

    Some(ClaudeShapedUsage {
        input,
        output,
        cache_read,
        cache_write_5m,
        cache_write_1h,
    })
}

/// What a Claude-shaped assistant `message` books, one entry per API call it
/// actually made (TOKL-26). The canonical description of `usage.iterations`;
/// other sites cite this one rather than restating it.
///
/// `usage.iterations` lists each call inside one assistant message, and it is a
/// model-fallback log: the first attempt and the fallback appear as separate
/// entries under DIFFERENT Models, so no single Usage Record can hold both.
///
/// The top-level object is not their rollup, and not consistently either
/// iteration's. Measured over every multi-call line on one machine: `input`,
/// `output`, `cache_read` and `cache_creation_input_tokens` are the LAST
/// iteration's (31/31), while the `cache_creation` split sub-object follows the
/// FIRST (5m 31/31, 1h 19/31). So reading only the top level booked one Request
/// of two, dropped the first call's tokens, and filed the first call's
/// cache-write TTL under the Model that served the fallback. Those ratios are
/// one machine's Artifact, not a contract — re-measure, do not trust this
/// paragraph as evidence.
///
/// Two or more entries → `PerCall`. One entry, an EMPTY array, or no array at
/// all → `OneMessage`, byte-for-byte the historical behaviour. That fallback is
/// load-bearing, not defensive: 2,600 Claude messages and 385 Qoder ones carry
/// `iterations: []` beside a non-zero token count, and a parser that trusted
/// the array length would book every one of them as zero Requests. The
/// single-entry case is not re-derived from the array either — it is the same
/// number, and re-deriving it only adds a way to be wrong.
pub(crate) fn claude_shaped_calls(message: &serde_json::Value) -> ClaudeCalls {
    if let Some(iterations) = message["usage"]["iterations"].as_array().filter(|a| a.len() > 1) {
        let calls: Vec<ClaudeCall> = iterations
            .iter()
            .enumerate()
            .filter_map(|(index, iteration)| {
                Some(ClaudeCall {
                    model: iteration["model"].as_str().filter(|m| !m.is_empty()).map(str::to_owned),
                    index,
                    usage: claude_shaped_buckets(iteration)?,
                })
            })
            .collect();
        // An array whose every entry is all-zero reports no call, but the
        // message was still billed at the top level — fall through to it rather
        // than book nothing. A Record that silently disappears is worse than
        // the floor this ticket set out to fix.
        if !calls.is_empty() {
            return ClaudeCalls::PerCall(calls);
        }
    }
    match claude_shaped_usage(message) {
        Some(usage) => ClaudeCalls::OneMessage(usage),
        None => ClaudeCalls::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded_db(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("artifact.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT, directory TEXT, tokens INTEGER);
             INSERT INTO session VALUES ('s1', '/w', 42);",
        )
        .unwrap();
        path
    }

    // A Scan only ever reads (ADR-0013): the handle the reader returns cannot
    // write the Source's Artifact even if a parse path tried.
    #[test]
    fn the_reader_cannot_write_the_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let path = seeded_db(temp.path());
        let ro = open_sqlite_artifact("kilo", &path).unwrap();
        let denied = ro.execute("INSERT INTO session VALUES ('s2', '/w', 1)", []);
        assert!(denied.is_err(), "a read-only handle accepted a write");
    }

    // The reason the wait exists: a Source mid-write holds the database for a
    // moment, and the scan must wait it out rather than fail the whole Source.
    // This passes whether the wait is this module's or the library default's —
    // the bounded test below is what pins the policy to this module.
    #[test]
    fn the_reader_waits_out_a_writer() {
        let temp = tempfile::tempdir().unwrap();
        let path = seeded_db(temp.path());
        let writer = rusqlite::Connection::open(&path).unwrap();
        writer.execute_batch("BEGIN EXCLUSIVE").unwrap();

        let ro = open_sqlite_artifact("kilo", &path).unwrap();
        let handle = std::thread::spawn(move || {
            ro.query_row("SELECT count(*) FROM session", [], |r| r.get::<_, i64>(0))
        });
        std::thread::sleep(std::time::Duration::from_millis(100));
        writer.execute_batch("COMMIT").unwrap();
        assert_eq!(handle.join().unwrap().unwrap(), 1);
    }

    // The wait is bounded, and it is THIS module's, not rusqlite's: the
    // 30-millisecond override below expires long before the library's own
    // 5-second default would, proving the reader sets the policy rather than
    // inheriting it — and a writer that never lets go surfaces as a busy
    // error rather than a hung scan. The elapsed assertion is load-bearing:
    // without it, deleting the busy_timeout call would leave the library
    // default to return the same error five seconds later and the test would
    // still pass. Through the internal seam so the test need not sit out the
    // production five seconds.
    #[test]
    fn the_wait_is_bounded_by_the_reader_not_the_library() {
        let temp = tempfile::tempdir().unwrap();
        let path = seeded_db(temp.path());
        let writer = rusqlite::Connection::open(&path).unwrap();
        writer.execute_batch("BEGIN EXCLUSIVE").unwrap();
        let ro =
            open_sqlite_artifact_waiting("kilo", &path, std::time::Duration::from_millis(30))
                .unwrap();
        let start = std::time::Instant::now();
        assert!(ro.query_row("SELECT count(*) FROM session", [], |r| r.get::<_, i64>(0)).is_err());
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "the 30ms override did not take: waited {:?} — the library default is deciding",
            start.elapsed()
        );
    }

    #[test]
    fn an_open_failure_names_the_source() {
        let missing = std::path::Path::new("/definitely/not/here/artifact.db");
        let err = open_sqlite_artifact("hermes", missing).unwrap_err();
        assert!(err.starts_with("hermes: database open failed: "), "{err}");
    }

    #[test]
    fn require_columns_returns_the_actual_set_and_tolerates_extras() {
        let temp = tempfile::tempdir().unwrap();
        let path = seeded_db(temp.path());
        let ro = open_sqlite_artifact("zed", &path).unwrap();
        let found = require_columns("zed", &ro, "session", &["id", "directory"]).unwrap();
        // The full set comes back, so a caller can branch on an optional column.
        assert!(found.contains("tokens"));
    }

    // A schema that lost a column, and a table that is missing outright, both
    // refuse the same way: this Source's Artifact is a shape the parse cannot
    // trust (a malformed instance of a supported shape — a warning, per the
    // glossary, not a new Artifact class).
    #[test]
    fn require_columns_refuses_missing_columns_and_missing_tables_alike() {
        let temp = tempfile::tempdir().unwrap();
        let path = seeded_db(temp.path());
        let ro = open_sqlite_artifact("qoder", &path).unwrap();
        assert_eq!(
            require_columns("qoder", &ro, "session", &["id", "vanished"]).unwrap_err(),
            "qoder: unsupported database schema"
        );
        assert_eq!(
            require_columns("qoder", &ro, "no_such_table", &["id"]).unwrap_err(),
            "qoder: unsupported database schema"
        );
    }

    // Not SQLite at all: the open may succeed (SQLite reads lazily), but the
    // first inspection refuses with the Source named.
    #[test]
    fn garbage_bytes_surface_as_a_named_refusal() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("garbage.db");
        std::fs::write(&path, b"not a database at all, not even close......").unwrap();
        let result = open_sqlite_artifact("goose", &path)
            .and_then(|ro| require_columns("goose", &ro, "session", &["id"]).map(|_| ()));
        let err = result.unwrap_err();
        assert!(err.starts_with("goose: schema inspection failed"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn jsonl_walk_follows_root_and_file_symlinks_but_skips_nested_directory_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let sessions = temp.path().join("sessions");
        std::fs::create_dir(&sessions).unwrap();
        std::fs::write(sessions.join("session.jsonl"), "{}\n").unwrap();
        // A plain non-JSONL file, so the extension half of the predicate is
        // pinned too: without it the walk would hand adapters any regular file.
        std::fs::write(sessions.join("notes.md"), "x\n").unwrap();
        let archived = temp.path().join("archived.jsonl");
        std::fs::write(&archived, "{}\n").unwrap();
        symlink(archived, sessions.join("linked.jsonl")).unwrap();
        symlink(".", sessions.join("loop")).unwrap();

        let configured_root = temp.path().join("configured-sessions");
        symlink(&sessions, &configured_root).unwrap();

        let mut files = Vec::new();
        find_jsonl(&configured_root, &mut files);
        files.sort();

        assert_eq!(
            files,
            vec![
                configured_root.join("linked.jsonl"),
                configured_root.join("session.jsonl"),
            ]
        );
    }

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
        assert_eq!(
            rollup_worktree("/Users/dev/worktrees"),
            "/Users/dev/worktrees"
        );
    }
}
