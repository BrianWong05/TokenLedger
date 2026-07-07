use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::adapters::claude::scan_claude;
use crate::adapters::codex::scan_codex;
use crate::adapters::gemini::scan_gemini;
use crate::adapters::hermes::scan_hermes;
use crate::db::prune_missing_files;
use crate::types::{ScanStatus, SourceScanResult, SourceStatus};

pub struct SourceRoots {
    pub claude: PathBuf,
    pub codex: PathBuf,
    pub gemini_tmp: PathBuf,
    pub gemini_projects_json: PathBuf,
    pub hermes_db: PathBuf,
}

impl SourceRoots {
    pub fn default_roots() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        SourceRoots {
            claude: home.join(".claude/projects"),
            codex: home.join(".codex/sessions"),
            gemini_tmp: home.join(".gemini/tmp"),
            gemini_projects_json: home.join(".gemini/projects.json"),
            hermes_db: home.join(".hermes/state.db"),
        }
    }
}

// Runs one adapter, converting a panic into a SourceStatus error so the
// remaining sources still run. Non-panic errors already arrive as
// SourceScanResult.error and pass straight through.
fn run_one(source: &str, f: impl FnOnce() -> SourceScanResult) -> SourceStatus {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => SourceStatus {
            source: source.to_string(),
            events_inserted: r.events_inserted,
            lines_skipped: r.lines_skipped,
            error: r.error,
        },
        Err(_) => SourceStatus {
            source: source.to_string(),
            events_inserted: 0,
            lines_skipped: 0,
            error: Some("adapter panicked".to_string()),
        },
    }
}

pub fn run_scan(conn: &mut Connection, roots: &SourceRoots) -> ScanStatus {
    let mut sources = Vec::with_capacity(4);
    sources.push(run_one("claude", || scan_claude(conn, &roots.claude)));
    sources.push(run_one("codex", || scan_codex(conn, &roots.codex)));
    sources.push(run_one("gemini", || {
        scan_gemini(conn, &roots.gemini_tmp, &roots.gemini_projects_json)
    }));
    sources.push(run_one("hermes", || scan_hermes(conn, &roots.hermes_db)));

    // Ledger hygiene only: drops scanned_files rows for vanished paths.
    // Never deletes events (see prune_missing_files contract). Best-effort.
    let _ = prune_missing_files(conn);

    let scanned_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    ScanStatus { sources, scanned_at }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_db;
    use std::fs;
    use std::path::PathBuf;

    // Minimal Claude assistant line (non-zero usage → one event ingested).
    // Shape matches real ~/.claude/projects/**/*.jsonl assistant records.
    const CLAUDE_LINE: &str = r#"{"type":"assistant","requestId":"req_test1","timestamp":"2026-07-01T10:00:00.000Z","cwd":"/Users/dev/projects/alpha","message":{"id":"msg_test1","model":"claude-opus-4-8","usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":10,"cache_creation_input_tokens":20,"cache_creation":{"ephemeral_5m_input_tokens":20,"ephemeral_1h_input_tokens":0}}}}"#;

    fn find<'a>(status: &'a ScanStatus, source: &str) -> &'a SourceStatus {
        status
            .sources
            .iter()
            .find(|s| s.source == source)
            .unwrap_or_else(|| panic!("missing source {source}"))
    }

    #[test]
    fn default_roots_live_under_home() {
        let r = SourceRoots::default_roots();
        assert!(r.claude.ends_with(".claude/projects"));
        assert!(r.codex.ends_with(".codex/sessions"));
        assert!(r.gemini_tmp.ends_with(".gemini/tmp"));
        assert!(r.gemini_projects_json.ends_with(".gemini/projects.json"));
        assert!(r.hermes_db.ends_with(".hermes/state.db"));
    }

    #[test]
    fn run_scan_isolates_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let base: PathBuf = tmp.path().to_path_buf();

        // Real Claude fixture: one project dir with one usage-bearing line.
        let claude_root = base.join("claude");
        fs::create_dir_all(claude_root.join("proj1")).unwrap();
        // Trailing newline required: the Claude adapter only consumes complete
        // newline-terminated lines (see adapters::claude resume semantics).
        fs::write(
            claude_root.join("proj1").join("session.jsonl"),
            format!("{CLAUDE_LINE}\n"),
        )
        .unwrap();

        // Everything else points at paths that do not exist.
        let roots = SourceRoots {
            claude: claude_root,
            codex: base.join("no-codex"),
            gemini_tmp: base.join("no-gemini"),
            gemini_projects_json: base.join("no-projects.json"),
            hermes_db: base.join("no-hermes.db"),
        };

        let mut conn = open_db(&base.join("ledger.db")).unwrap();
        let status = run_scan(&mut conn, &roots);

        assert_eq!(status.sources.len(), 4);
        assert!(status.scanned_at > 0);

        // Claude still ingests its event even though hermes errors.
        let claude = find(&status, "claude");
        assert_eq!(claude.events_inserted, 1);
        assert!(claude.error.is_none());

        // Missing directories → zero events, no error.
        let codex = find(&status, "codex");
        assert_eq!(codex.events_inserted, 0);
        assert!(codex.error.is_none());
        let gemini = find(&status, "gemini");
        assert_eq!(gemini.events_inserted, 0);
        assert!(gemini.error.is_none());

        // Nonexistent hermes DB → error string set; other sources unaffected.
        let hermes = find(&status, "hermes");
        assert!(hermes.error.is_some());
    }
}
