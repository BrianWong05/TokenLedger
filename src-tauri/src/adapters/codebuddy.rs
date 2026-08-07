//! CodeBuddy Source adapter (ADR-0016).
//!
//! CodeBuddy (CLI, IDE, VS Code plugin) writes the same Claude-Code-like
//! transcript shape as WorkBuddy to `~/.codebuddy/projects/**/*.jsonl`, so it
//! reuses the shared transcript parser — in the same way Oh My Pi shares pi's
//! parser. This module is thin by construction: the Source identity flavours
//! the dedup key and the artifact root comes from the catalog; every parsing
//! rule (line granularity, cache split, additive subagents, conservative
//! `summary`, ignored `credit`) lives in the shared parser and is proven by
//! the shared fixture family.

use crate::adapters::workbuddy::scan_transcript;
use crate::types::SourceScanResult;
use rusqlite::Connection;
use std::path::Path;

pub fn scan_codebuddy(conn: &mut Connection, projects_root: &Path) -> SourceScanResult {
    scan_transcript(conn, projects_root, "codebuddy")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_db;
    use rusqlite::Connection;
    use std::fs;

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn scan_root(conn: &mut Connection, root: &Path) -> SourceScanResult {
        scan_codebuddy(conn, root)
    }

    #[test]
    fn codebuddy_ingests_its_own_transcript_independently_of_workbuddy() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // CodeBuddy-shaped transcript: message with Anthropic-style usage, a
        // non-zero summary, a zero-token summary, and a non-usage line.
        write(
            root,
            "proj/cb-sess.jsonl",
            concat!(
                r#"{"type":"message","id":"cb-1","sessionId":"cb-sess","timestamp":1786092914695,"cwd":"/Users/dev/projects/alpha","message":{"usage":{"input_tokens":25190,"output_tokens":10,"total_tokens":25200,"cache_read_input_tokens":512}}}"#,
                "\n",
                r#"{"type":"summary","id":"cb-2","timestamp":1786092915000,"cwd":"/Users/dev/projects/alpha","providerData":{"model":"hy3","usage":{"requests":1,"inputTokens":100,"outputTokens":5,"totalTokens":105}}}"#,
                "\n",
                r#"{"type":"summary","id":"cb-3","timestamp":1786092916000,"providerData":{"usage":{"requests":1,"inputTokens":0,"outputTokens":0,"totalTokens":0}}}"#,
                "\n",
                r#"{"type":"file-history-snapshot","id":"cb-4","timestamp":1786092917000,"snapshot":{}}"#,
                "\n",
            ),
        );
        let mut conn = open_db(&root.join("ledger.db")).unwrap();
        let result = scan_root(&mut conn, &root.join("proj"));
        assert_eq!(result.events_inserted, 2, "message + non-zero summary only");
        assert!(result.error.is_none());

        // Both Records are codebuddy's; zero-token summary and non-usage lines
        // never become Records; the cache split matches the shared rule.
        let rows: Vec<(String, Option<String>, i64, i64)> = conn
            .prepare("SELECT source, model, input_tokens, cache_read_tokens FROM events ORDER BY dedup_key")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|(source, ..)| source == "codebuddy"));
        assert_eq!(rows[0], ("codebuddy".to_string(), None, 24678, 512));
        assert_eq!(
            rows[1],
            ("codebuddy".to_string(), Some("hy3".to_string()), 100, 0)
        );
    }

    #[test]
    fn codebuddy_roots_are_never_attributed_to_workbuddy() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            root,
            "proj/sess.jsonl",
            r#"{"type":"function_call","id":"x1","sessionId":"s","timestamp":1786091399000,"cwd":"/Users/dev/projects/alpha","providerData":{"model":"hy3","usage":{"requests":1,"inputTokens":500,"outputTokens":10,"totalTokens":510}}}"#,
        );
        let mut conn = open_db(&root.join("ledger.db")).unwrap();
        scan_root(&mut conn, &root.join("proj"));
        let only_codebuddy: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE source = 'codebuddy' AND NOT EXISTS \
                 (SELECT 1 FROM events AS w WHERE w.source = 'workbuddy')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(only_codebuddy, 1);
    }
}
