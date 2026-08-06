use std::ffi::OsStr;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::adapters::antigravity::scan_antigravity;
use crate::adapters::claude::scan_claude;
use crate::adapters::codex::scan_codex;
use crate::adapters::gemini::scan_gemini;
use crate::adapters::grok::scan_grok;
use crate::adapters::hermes::scan_hermes;
use crate::adapters::pi::scan_pi;
use crate::db::prune_missing_files;
use crate::source_catalog;
use crate::types::{ScanStatus, SourceScanResult, SourceStatus};

pub struct SourceRoots {
    pub claude: PathBuf,
    pub codex: PathBuf,
    pub gemini_tmp: PathBuf,
    pub gemini_projects_json: PathBuf,
    pub hermes_db: PathBuf,
    pub grok_sessions: PathBuf,
    // IDE and CLI conversation dirs share one SQLite schema; both scanned.
    pub antigravity_conversations: PathBuf,
    pub antigravity_cli_conversations: PathBuf,
    pub pi_sessions: Vec<PathBuf>,
}

impl SourceRoots {
    pub fn default_roots() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let session_dir = pi_environment_value("session-dir");
        let agent_dir = pi_environment_value("agent-dir");
        Self::from_home_and_pi_env(&home, session_dir.as_deref(), agent_dir.as_deref())
    }

    fn from_home_and_pi_env(
        home: &Path,
        session_dir: Option<&OsStr>,
        agent_dir: Option<&OsStr>,
    ) -> Self {
        let mut pi_sessions = vec![catalog_root(home, "pi", "sessions")];
        append_pi_override(&mut pi_sessions, home, "session-dir", session_dir);
        append_pi_override(&mut pi_sessions, home, "agent-dir", agent_dir);
        SourceRoots {
            claude: catalog_root(home, "claude", "projects"),
            codex: catalog_root(home, "codex", "sessions"),
            gemini_tmp: catalog_root(home, "gemini", "tmp"),
            gemini_projects_json: catalog_root(home, "gemini", "projects"),
            hermes_db: catalog_root(home, "hermes", "state"),
            grok_sessions: catalog_root(home, "grok", "sessions"),
            antigravity_conversations: catalog_root(home, "antigravity", "conversations"),
            antigravity_cli_conversations: catalog_root(home, "antigravity", "cli-conversations"),
            pi_sessions,
        }
    }
}

fn catalog_root(home: &Path, source: &str, artifact: &str) -> PathBuf {
    let path = source_catalog::artifact(source, artifact)
        .and_then(|artifact| artifact.path.as_deref())
        .unwrap_or_else(|| panic!("source catalog must define {source}.{artifact} path"));
    home.join(path)
}

fn pi_environment_value(artifact: &str) -> Option<std::ffi::OsString> {
    let environment = source_catalog::artifact("pi", artifact)
        .and_then(|artifact| artifact.environment.as_deref())
        .unwrap_or_else(|| panic!("source catalog must define pi.{artifact} environment"));
    std::env::var_os(environment)
}

fn append_pi_override(
    pi_sessions: &mut Vec<PathBuf>,
    home: &Path,
    artifact_id: &str,
    value: Option<&OsStr>,
) {
    let Some(path) = value.and_then(|value| visible_pi_path(home, value)) else {
        return;
    };
    let artifact = source_catalog::artifact("pi", artifact_id)
        .unwrap_or_else(|| panic!("source catalog must define pi.{artifact_id}"));
    pi_sessions.push(match artifact.suffix.as_deref() {
        Some(suffix) => path.join(suffix),
        None => path,
    });
}

fn visible_pi_path(home: &Path, value: &OsStr) -> Option<PathBuf> {
    if value.is_empty() {
        return None;
    }
    let path = Path::new(value);
    if path == Path::new("~") {
        return Some(home.to_path_buf());
    }
    match path.strip_prefix("~") {
        Ok(rest) => Some(home.join(rest)),
        Err(_) => Some(path.to_path_buf()),
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
    let catalog = source_catalog::catalog();
    let mut sources = Vec::with_capacity(catalog.sources.len());
    for source in &catalog.sources {
        let status = match source.key.as_str() {
            "claude" => run_one(&source.key, || scan_claude(conn, &roots.claude)),
            "codex" => run_one(&source.key, || scan_codex(conn, &roots.codex)),
            "gemini" => run_one(&source.key, || {
                scan_gemini(conn, &roots.gemini_tmp, &roots.gemini_projects_json)
            }),
            "hermes" => run_one(&source.key, || scan_hermes(conn, &roots.hermes_db)),
            "grok" => run_one(&source.key, || scan_grok(conn, &roots.grok_sessions)),
            "antigravity" => run_one(&source.key, || {
                scan_antigravity(
                    conn,
                    &[
                        roots.antigravity_conversations.as_path(),
                        roots.antigravity_cli_conversations.as_path(),
                    ],
                )
            }),
            "pi" => run_one(&source.key, || scan_pi(conn, &roots.pi_sessions)),
            _ => SourceStatus {
                source: source.key.clone(),
                events_inserted: 0,
                lines_skipped: 0,
                error: Some("unsupported source catalog entry".to_string()),
            },
        };
        sources.push(status);
    }

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
    use crate::db::{get_file_state, open_db};
    use crate::pricing::{self, OverrideRates};
    use crate::queries::{self, Filters};
    use std::fs;
    use std::path::PathBuf;

    // Minimal Claude assistant line (non-zero usage → one event ingested).
    // Shape matches real ~/.claude/projects/**/*.jsonl assistant records.
    const CLAUDE_LINE: &str = r#"{"type":"assistant","requestId":"req_test1","timestamp":"2026-07-01T10:00:00.000Z","cwd":"/Users/dev/projects/alpha","message":{"id":"msg_test1","model":"claude-opus-4-8","usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":10,"cache_creation_input_tokens":20,"cache_creation":{"ephemeral_5m_input_tokens":20,"ephemeral_1h_input_tokens":0}}}}"#;
    const PI_SESSION: &str = include_str!("adapters/fixtures/pi/basic-session.jsonl");

    fn find<'a>(status: &'a ScanStatus, source: &str) -> &'a SourceStatus {
        status
            .sources
            .iter()
            .find(|s| s.source == source)
            .unwrap_or_else(|| panic!("missing source {source}"))
    }

    #[test]
    fn catalog_describes_the_existing_sources_and_artifact_roots() {
        let catalog = crate::source_catalog::catalog();
        assert_eq!(
            catalog.sources.iter().map(|source| source.key.as_str()).collect::<Vec<_>>(),
            ["claude", "codex", "gemini", "hermes", "grok", "antigravity", "pi"],
        );
        assert!(catalog.sources.iter().all(|source| {
            !source.label.is_empty()
                && !source.aliases.is_empty()
                && source.color.starts_with('#')
                && !source.icon.is_empty()
                && source.platforms == ["all"]
                && source.prerequisite.is_none()
                && source.capabilities.model
                && source.capabilities.project
                && source.capabilities.session
                && source.capabilities.token_categories
        }));
        assert_eq!(
            catalog.sources.iter().filter(|source| source.capabilities.context)
                .map(|source| source.key.as_str()).collect::<Vec<_>>(),
            ["claude", "codex", "pi"],
        );
        assert_eq!(
            catalog.sources.iter().flat_map(|source| {
                source.artifacts.iter().filter_map(move |artifact| artifact.path.as_deref()
                    .map(|path| (source.key.as_str(), artifact.id.as_str(), path)))
            }).collect::<Vec<_>>(),
            [
                ("claude", "projects", ".claude/projects"),
                ("codex", "sessions", ".codex/sessions"),
                ("gemini", "tmp", ".gemini/tmp"),
                ("gemini", "projects", ".gemini/projects.json"),
                ("hermes", "state", ".hermes/state.db"),
                ("grok", "sessions", ".grok/sessions"),
                ("antigravity", "conversations", ".gemini/antigravity/conversations"),
                ("antigravity", "cli-conversations", ".gemini/antigravity-cli/conversations"),
                ("pi", "sessions", ".pi/agent/sessions"),
            ],
        );
        assert!(catalog.sources.iter().flat_map(|source| &source.artifacts).all(|artifact| {
            matches!(artifact.kind.as_str(), "directory" | "file")
                && artifact.platforms == ["all"]
                && artifact.prerequisite.is_none()
        }));

        let claude = crate::source_catalog::source("claude").unwrap();
        assert_eq!(claude.source, "Claude Code");
        assert!(claude.artifacts.iter().any(|artifact| artifact.path.as_deref() == Some(".claude/projects")));

        let hermes = crate::source_catalog::source("hermes").unwrap();
        assert_eq!(hermes.artifacts[0].path.as_deref(), Some(".hermes/state.db"));

        let pi = crate::source_catalog::source("pi").unwrap();
        assert_eq!(pi.artifacts.iter().map(|artifact| artifact.id.as_str()).collect::<Vec<_>>(), ["sessions", "session-dir", "agent-dir"]);
        assert_eq!(pi.artifacts[0].path.as_deref(), Some(".pi/agent/sessions"));
        assert_eq!(pi.artifacts[1].environment.as_deref(), Some("PI_CODING_AGENT_SESSION_DIR"));
        assert_eq!(pi.artifacts[2].environment.as_deref(), Some("PI_CODING_AGENT_DIR"));
        assert_eq!(pi.artifacts[2].suffix.as_deref(), Some("sessions"));
    }

    #[test]
    fn pi_roots_include_standard_and_visible_session_and_agent_overrides() {
        use std::ffi::OsStr;

        let home = tempfile::tempdir().unwrap();
        let roots = SourceRoots::from_home_and_pi_env(
            home.path(),
            Some(OsStr::new("~/custom-sessions")),
            Some(OsStr::new("~/custom-agent")),
        );
        assert_eq!(
            roots.pi_sessions,
            vec![
                home.path().join(".pi/agent/sessions"),
                home.path().join("custom-sessions"),
                home.path().join("custom-agent/sessions"),
            ],
        );
    }

    #[test]
    fn default_roots_live_under_home() {
        let r = SourceRoots::default_roots();
        assert!(r.claude.ends_with(".claude/projects"));
        assert!(r.codex.ends_with(".codex/sessions"));
        assert!(r.gemini_tmp.ends_with(".gemini/tmp"));
        assert!(r.gemini_projects_json.ends_with(".gemini/projects.json"));
        assert!(r.hermes_db.ends_with(".hermes/state.db"));
        assert!(r.grok_sessions.ends_with(".grok/sessions"));
        assert!(r
            .antigravity_conversations
            .ends_with(".gemini/antigravity/conversations"));
        assert!(r
            .antigravity_cli_conversations
            .ends_with(".gemini/antigravity-cli/conversations"));
        assert!(r.pi_sessions[0].ends_with(".pi/agent/sessions"));
    }

    #[test]
    fn run_scan_ingests_pi_fixture_through_every_ledger_surface() {
        std::env::set_var("TZ", "UTC");
        let tmp = tempfile::tempdir().unwrap();
        let base: PathBuf = tmp.path().to_path_buf();
        let pi_root = base.join("pi-sessions");
        let project_dir = pi_root.join("--Users-dev-projects-pi-demo--");
        fs::create_dir_all(&project_dir).unwrap();
        let session_path = project_dir.join("session.jsonl");
        fs::write(&session_path, PI_SESSION).unwrap();
        let source_file = fs::canonicalize(&session_path)
            .unwrap()
            .to_string_lossy()
            .to_string();

        let roots = SourceRoots {
            claude: base.join("no-claude"),
            codex: base.join("no-codex"),
            gemini_tmp: base.join("no-gemini"),
            gemini_projects_json: base.join("no-projects.json"),
            hermes_db: base.join("no-hermes.db"),
            grok_sessions: base.join("no-grok"),
            antigravity_conversations: base.join("no-antigravity"),
            antigravity_cli_conversations: base.join("no-antigravity-cli"),
            pi_sessions: vec![pi_root],
        };

        let db_path = base.join("ledger.db");
        let mut conn = open_db(&db_path).unwrap();
        conn.execute(
            "INSERT INTO prices (model, input_per_tok, output_per_tok, cache_read_per_tok, cache_write_5m_per_tok, cache_write_1h_per_tok) \
             VALUES ('pi-response-model', 0.000002, 0.000010, 0.0000005, 0.0000025, 0.000004)",
            [],
        ).unwrap();
        pricing::set_override(&conn, "pi-fallback-model", OverrideRates {
            input: Some(0.000001),
            output: Some(0.000002),
            cache_read: None,
            cache_write: None,
        }).unwrap();

        let status = run_scan(&mut conn, &roots);
        assert_eq!(status.sources.len(), 7);
        assert_eq!(status.sources.last().unwrap().source, "pi");
        let pi = find(&status, "pi");
        // 3 assistant Requests + 1 Unattributed tool-result Request.
        assert_eq!(pi.events_inserted, 4);
        assert_eq!(pi.lines_skipped, 2);
        assert!(pi.error.is_none());

        // Totals include the Unattributed tool result (input/output/cacheRead 900
        // each + 900 short cache write); its 3600 tokens count but carry no Cost.
        let summary = queries::summary(&conn, &Filters::default()).unwrap();
        assert_eq!(summary.input_tokens, 1035);
        assert_eq!(summary.output_tokens, 962);
        assert_eq!(summary.cache_read_tokens, 923);
        assert_eq!(summary.cache_write_tokens, 919);
        assert_eq!(summary.total_tokens, 3839);
        assert_eq!(summary.requests, 4);
        assert!((summary.cost.unwrap() - 0.000805).abs() < 1e-12, "Unattributed usage adds no Cost");
        assert_eq!(summary.unattributed_tokens, 3600);
        assert!(summary.has_unpriced);
        assert_eq!(summary.unpriced_models, vec!["pi-error-model".to_string()]);

        let source_rows = queries::breakdown(&conn, "tool", &Filters::default()).unwrap();
        let pi_source = source_rows.iter().find(|r| r.key.as_deref() == Some("pi")).unwrap();
        assert_eq!(pi_source.total_tokens, 3839);
        assert_eq!(pi_source.requests, 4);
        assert!((pi_source.cost.unwrap() - 0.000805).abs() < 1e-12);
        assert!(pi_source.has_unpriced);

        // The model breakdown keeps a null-Model row for the Unattributed usage,
        // distinct from the three real pi Models.
        let model_rows = queries::breakdown(&conn, "model", &Filters::default()).unwrap();
        assert_eq!(model_rows.len(), 4);
        assert!(model_rows.iter().all(|r| r.source.as_deref() == Some("pi")));
        assert!(model_rows.iter().all(|r| r.key.as_deref() != Some("pi-selected-model")));
        assert_eq!(model_rows.iter().filter(|r| r.key.is_none()).count(), 1);
        let response = model_rows
            .iter()
            .find(|r| r.key.as_deref() == Some("pi-response-model"))
            .unwrap();
        assert_eq!(response.reasoning_tokens, Some(10));

        let projects = queries::breakdown(&conn, "project", &Filters::default()).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].key.as_deref(), Some("/Users/dev/projects/pi-demo"));
        assert_eq!(projects[0].total_tokens, 3839);

        let series = queries::series(&conn, &Filters::default(), "day").unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].source, "pi");
        assert_eq!(series[0].total_tokens, 3839);
        assert_eq!(series[0].requests, 4);
        assert_eq!(series[0].cache_write_tokens, 919);
        assert!((series[0].cost - 0.000805).abs() < 1e-12);
        assert_eq!(series[0].by_model.len(), 3, "per-model series omits the null Model");

        let pricing_rows = pricing::model_pricing(&conn).unwrap();
        for model in ["pi-response-model", "pi-fallback-model", "pi-error-model"] {
            let row = pricing_rows.iter().find(|r| r.model == model).unwrap();
            assert_eq!(row.tool, "pi");
        }
        assert!(pricing_rows.iter().all(|r| r.model != "pi-selected-model"));

        let second = run_scan(&mut conn, &roots);
        assert_eq!(find(&second, "pi").events_inserted, 0, "repeat scan is idempotent");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM events WHERE source = 'pi'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 4);

        fs::remove_file(session_path).unwrap();
        let after_disappearance = run_scan(&mut conn, &roots);
        assert_eq!(find(&after_disappearance, "pi").events_inserted, 0);
        assert!(find(&after_disappearance, "pi").error.is_none());
        assert!(get_file_state(&conn, &source_file).unwrap().is_none());
        let retained: i64 = conn
            .query_row("SELECT COUNT(*) FROM events WHERE source = 'pi'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(retained, 4, "missing source file never prunes Ledger usage");
        drop(conn);

        let mut durable_bytes = Vec::new();
        for suffix in ["", "-wal", "-shm"] {
            if let Ok(bytes) = fs::read(format!("{}{}", db_path.display(), suffix)) {
                durable_bytes.extend(bytes);
            }
        }
        for private in [
            "PRIVATE_PROMPT_SHOULD_NOT_PERSIST",
            "PRIVATE_RESPONSE_SHOULD_NOT_PERSIST",
            "PRIVATE_REASONING_SHOULD_NOT_PERSIST",
            "PRIVATE_IMAGE_SHOULD_NOT_PERSIST",
            "PRIVATE_TOOL_ARG_SHOULD_NOT_PERSIST",
            "PRIVATE_TOOL_RESULT_SHOULD_NOT_PERSIST",
            "PRIVATE_ERROR_SHOULD_NOT_PERSIST",
        ] {
            assert!(
                !durable_bytes.windows(private.len()).any(|w| w == private.as_bytes()),
                "private fixture content reached the Ledger: {private}",
            );
        }
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

        // A broken pi file sorts before a valid one. The valid file must still
        // reach the Ledger, and the six existing Sources must still report.
        let pi_root = base.join("pi");
        fs::create_dir_all(&pi_root).unwrap();
        fs::write(pi_root.join("a-broken.jsonl"), [0xff, b'\n']).unwrap();
        fs::write(pi_root.join("b-valid.jsonl"), PI_SESSION).unwrap();

        // Everything else points at paths that do not exist.
        let roots = SourceRoots {
            claude: claude_root,
            codex: base.join("no-codex"),
            gemini_tmp: base.join("no-gemini"),
            gemini_projects_json: base.join("no-projects.json"),
            hermes_db: base.join("no-hermes.db"),
            grok_sessions: base.join("no-grok"),
            antigravity_conversations: base.join("no-antigravity"),
            antigravity_cli_conversations: base.join("no-antigravity-cli"),
            pi_sessions: vec![pi_root],
        };

        let mut conn = open_db(&base.join("ledger.db")).unwrap();
        let status = run_scan(&mut conn, &roots);

        assert_eq!(status.sources.len(), 7);
        assert_eq!(status.sources.last().unwrap().source, "pi");
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

        // Missing directory-shaped roots → zero events, no error.
        let grok = find(&status, "grok");
        assert_eq!(grok.events_inserted, 0);
        assert!(grok.error.is_none());
        let antigravity = find(&status, "antigravity");
        assert_eq!(antigravity.events_inserted, 0);
        assert!(antigravity.error.is_none());
        let pi = find(&status, "pi");
        assert_eq!(pi.events_inserted, 4, "valid pi file survives broken sibling");
        assert!(pi
            .error
            .as_deref()
            .is_some_and(|error| error.contains("a-broken.jsonl")));

        let pi_requests: i64 = conn
            .query_row(
                "SELECT SUM(api_calls) FROM events WHERE source = 'pi'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pi_requests, 4);
    }
}
