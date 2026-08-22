//! Oh My Pi Source adapter.
//!
//! Oh My Pi (omp) writes the same session-tree shape as pi to
//! `~/.omp/agent/sessions/**/*.jsonl`, so it reuses the shared session parser
//! from `adapters::pi` (precedent: WorkBuddy/CodeBuddy sharing one parser per ADR-0016).

use std::path::PathBuf;
use rusqlite::Connection;
use crate::types::SourceScanResult;
use super::pi::scan_pi_sessions;

pub fn scan_omp(conn: &mut Connection, session_roots: &[PathBuf]) -> SourceScanResult {
    scan_pi_sessions(conn, session_roots, "omp")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use crate::db::open_db;

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn omp_ingests_its_own_session_independently_of_pi() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            root,
            "sessions/session-omp.jsonl",
            include_str!("fixtures/omp/basic-session.jsonl"),
        );
        let mut conn = open_db(&root.join("ledger.db")).unwrap();
        let result = scan_omp(&mut conn, &[root.join("sessions")]);
        assert_eq!(result.events_inserted, 1);
        assert!(result.error.is_none());

        let rows: Vec<(String, Option<String>, i64)> = conn
            .prepare("SELECT source, model, input_tokens FROM events")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], ("omp".to_string(), Some("claude-3-5-sonnet".to_string()), 100));
    }

    #[test]
    fn omp_roots_are_never_attributed_to_pi() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            root,
            "sessions/session-omp.jsonl",
            concat!(
                r#"{"type":"session","version":3,"id":"session-omp","timestamp":"2026-07-01T12:00:00.000Z","cwd":"/Users/dev/projects/alpha"}"#,
                "\n",
                r#"{"type":"message","id":"a1","parentId":null,"timestamp":"2026-07-01T12:00:02.000Z","message":{"role":"assistant","content":[],"provider":"anthropic","model":"m1","usage":{"input":50,"output":20},"stopReason":"stop","timestamp":1782907202000}}"#,
                "\n",
            ),
        );
        let mut conn = open_db(&root.join("ledger.db")).unwrap();
        scan_omp(&mut conn, &[root.join("sessions")]);
        let only_omp: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE source = 'omp' AND NOT EXISTS \
                 (SELECT 1 FROM events AS p WHERE p.source = 'pi')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(only_omp, 1);
    }
}
