use std::ffi::{OsStr, OsString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::adapters::antigravity::scan_antigravity;
use crate::adapters::claude::scan_claude;
use crate::adapters::cline::scan_cline;
use crate::adapters::codex::scan_codex;
use crate::adapters::gemini::scan_gemini;
use crate::adapters::goose::scan_goose;
use crate::adapters::grok::scan_grok;
use crate::adapters::hermes::scan_hermes;
use crate::adapters::opencode::scan_opencode;
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
    pub goose_sessions: Vec<PathBuf>,
    pub pi_sessions: Vec<PathBuf>,
    pub opencode_data: PathBuf,
    pub opencode_legacy: PathBuf,
    pub opencode_db: Option<PathBuf>,
    pub cline: Vec<PathBuf>,
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
        Self::from_home_and_pi_env_with_cline(
            home,
            session_dir,
            agent_dir,
            std::env::var_os("HERMES_HOME").as_deref(),
            gemini_environment_value().as_deref(),
            grok_environment_value().as_deref(),
            environment_value("cline", "cli-data").as_deref(),
            environment_value("cline", "cli-sandbox").as_deref(),
        )
    }

    #[cfg(test)]
    fn from_home_and_pi_env_with_hermes_and_gemini_and_grok(
        home: &Path,
        session_dir: Option<&OsStr>,
        agent_dir: Option<&OsStr>,
        hermes_home: Option<&OsStr>,
        gemini_home: Option<&OsStr>,
        grok_home: Option<&OsStr>,
    ) -> Self {
        Self::from_home_and_pi_env_with_cline(
            home,
            session_dir,
            agent_dir,
            hermes_home,
            gemini_home,
            grok_home,
            None,
            None,
        )
    }

    fn from_home_and_pi_env_with_cline(
        home: &Path,
        session_dir: Option<&OsStr>,
        agent_dir: Option<&OsStr>,
        hermes_home: Option<&OsStr>,
        gemini_home: Option<&OsStr>,
        grok_home: Option<&OsStr>,
        cline_data: Option<&OsStr>,
        cline_sandbox_data: Option<&OsStr>,
    ) -> Self {
        let gemini_home = gemini_home_for(home, gemini_home);
        let mut pi_sessions = vec![catalog_root(home, "pi", "sessions")];
        append_pi_override(&mut pi_sessions, home, "session-dir", session_dir);
        append_pi_override(&mut pi_sessions, home, "agent-dir", agent_dir);
        let goose_sessions = goose_session_roots(
            home,
            environment_value("goose", "root").as_deref(),
            std::env::consts::OS,
        );
        let (opencode_data, opencode_legacy, opencode_db) = opencode_roots(
            home,
            environment_value("opencode", "data").as_deref(),
            environment_value("opencode", "db").as_deref(),
            environment_value("opencode", "xdg-data").as_deref(),
        );
        SourceRoots {
            claude: catalog_root(home, "claude", "projects"),
            codex: catalog_root(home, "codex", "sessions"),
            gemini_tmp: gemini_home.join(source_catalog::artifact_filename("gemini", "tmp")),
            gemini_projects_json: gemini_home
                .join(source_catalog::artifact_filename("gemini", "projects")),
            hermes_db: hermes_home_for(home, hermes_home)
                .join(source_catalog::artifact_filename("hermes", "state")),
            grok_sessions: grok_home_for(home, grok_home)
                .join(source_catalog::artifact_filename("grok", "sessions")),
            antigravity_conversations: catalog_root(home, "antigravity", "conversations"),
            antigravity_cli_conversations: catalog_root(home, "antigravity", "cli-conversations"),
            goose_sessions,
            pi_sessions,
            opencode_data,
            opencode_legacy,
            opencode_db,
            cline: cline_roots(home, std::env::consts::OS, cline_data, cline_sandbox_data),
        }
    }
}

fn catalog_root(home: &Path, source: &str, artifact: &str) -> PathBuf {
    let path = source_catalog::artifact(source, artifact)
        .and_then(|artifact| artifact.path.as_deref())
        .unwrap_or_else(|| panic!("source catalog must define {source}.{artifact} path"));
    home.join(path)
}

fn catalog_root_for_platform(
    home: &Path,
    source: &str,
    artifact: &str,
    platform: &str,
) -> Option<PathBuf> {
    let definition = source_catalog::artifact(source, artifact)?;
    if !definition
        .platforms
        .iter()
        .any(|supported| supported == "all" || supported == platform)
    {
        return None;
    }
    definition.path.as_deref().map(|path| home.join(path))
}

fn cline_roots(
    home: &Path,
    platform: &str,
    cli_data: Option<&OsStr>,
    cli_sandbox_data: Option<&OsStr>,
) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for artifact in [
        "editor-code-macos",
        "editor-code-insiders-macos",
        "editor-code-linux",
        "editor-code-insiders-linux",
        "editor-code-windows",
        "editor-code-insiders-windows",
        "editor-server",
        "editor-server-insiders",
    ] {
        if let Some(path) = catalog_root_for_platform(home, "cline", artifact, platform) {
            push_unique_root(&mut roots, path);
        }
    }

    let cli_root = cli_data
        .and_then(|value| visible_path(home, value))
        .or_else(|| cli_sandbox_data.and_then(|value| visible_path(home, value)))
        .or_else(|| catalog_root_for_platform(home, "cline", "cli-default-data", platform));
    if let Some(path) = cli_root {
        push_unique_root(&mut roots, path);
    }
    roots
}

fn push_unique_root(roots: &mut Vec<PathBuf>, path: PathBuf) {
    let normalized = normalized_path(&path);
    if !roots.iter().any(|root| normalized_path(root) == normalized) {
        roots.push(path);
    }
}

fn normalized_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn environment_value(source: &str, artifact: &str) -> Option<OsString> {
    let environment = source_catalog::artifact(source, artifact)
        .and_then(|artifact| artifact.environment.as_deref())
        .unwrap_or_else(|| panic!("source catalog must define {source}.{artifact} environment"));
    std::env::var_os(environment)
}

fn pi_environment_value(artifact: &str) -> Option<OsString> {
    environment_value("pi", artifact)
}

fn gemini_environment_value() -> Option<OsString> {
    environment_value("gemini", "tmp")
}

fn grok_environment_value() -> Option<OsString> {
    environment_value("grok", "sessions")
}

fn goose_session_roots(home: &Path, value: Option<&OsStr>, platform: &str) -> Vec<PathBuf> {
    if let Some(root) = value
        .and_then(|value| visible_path(home, value))
        .filter(|path| path.is_absolute())
    {
        return vec![root.join("data/sessions")];
    }

    let current = match platform {
        "macos" => home.join("Library/Application Support/Block/goose/data/sessions"),
        "windows" => home.join("AppData/Roaming/Block/goose/data/sessions"),
        _ => home.join(".local/share/goose/sessions"),
    };
    let mut roots = vec![current];

    // Goose's pre-1.10 JSONL sessions lived in the Unix-like data directory.
    // On macOS this is distinct from the current application-support root;
    // Linux already uses the same path, and Windows has no separate documented
    // legacy location.
    if platform == "macos" {
        roots.push(home.join(".local/share/goose/sessions"));
    }

    roots.dedup();
    roots
}

fn opencode_roots(
    home: &Path,
    data_override: Option<&OsStr>,
    database_override: Option<&OsStr>,
    xdg_data_home: Option<&OsStr>,
) -> (PathBuf, PathBuf, Option<PathBuf>) {
    let data = data_override
        .and_then(|value| visible_path(home, value))
        .filter(|path| path.is_absolute())
        .or_else(|| {
            xdg_data_home
                .and_then(|value| visible_path(home, value))
                .filter(|path| path.is_absolute())
                .map(|path| path.join("opencode"))
        })
        .unwrap_or_else(|| home.join(".local/share/opencode"));
    let database = database_override
        .and_then(|value| visible_path(home, value))
        .filter(|path| path.is_absolute());
    let legacy = data.join("storage");
    (data, legacy, database)
}

fn hermes_home_for(home: &Path, value: Option<&OsStr>) -> PathBuf {
    if let Some(path) = value.and_then(|value| visible_path(home, value)) {
        return path;
    }
    catalog_root(home, "hermes", "state")
        .parent()
        .expect("Hermes state artifact must have a parent directory")
        .to_path_buf()
}

fn gemini_home_for(home: &Path, value: Option<&OsStr>) -> PathBuf {
    let gemini_dir = catalog_artifact_parent("gemini", "tmp");
    if let Some(path) = value.and_then(|value| visible_path(home, value)) {
        return path.join(gemini_dir);
    }
    home.join(gemini_dir)
}

fn grok_home_for(home: &Path, value: Option<&OsStr>) -> PathBuf {
    if let Some(path) = value.and_then(|value| visible_path(home, value)) {
        return path;
    }
    home.join(catalog_artifact_parent("grok", "sessions"))
}

fn catalog_artifact_parent(source: &str, artifact: &str) -> PathBuf {
    let path = source_catalog::artifact(source, artifact)
        .and_then(|artifact| artifact.path.as_deref())
        .unwrap_or_else(|| panic!("source catalog must define {source}.{artifact} path"));
    Path::new(path)
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!("source catalog artifact {source}.{artifact} path must have a parent")
        })
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
    visible_path(home, value)
}

fn visible_path(home: &Path, value: &OsStr) -> Option<PathBuf> {
    let value = value.to_string_lossy();
    let value = value.trim();
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
    run_scan_sources(conn, roots, &catalog.sources, std::env::consts::OS)
}

fn run_scan_sources(
    conn: &mut Connection,
    roots: &SourceRoots,
    catalog_sources: &[source_catalog::SourceDefinition],
    target_platform: &str,
) -> ScanStatus {
    let mut sources = Vec::with_capacity(catalog_sources.len());
    for source in catalog_sources {
        let status = match source_catalog::availability(source, target_platform) {
            Err(error) => unavailable_source_status(&source.key, error),
            Ok(()) => match source.key.as_str() {
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
                "goose" => run_one(&source.key, || scan_goose(conn, &roots.goose_sessions)),
                "pi" => run_one(&source.key, || scan_pi(conn, &roots.pi_sessions)),
                "opencode" => run_one(&source.key, || {
                    scan_opencode(
                        conn,
                        &roots.opencode_data,
                        &roots.opencode_legacy,
                        roots.opencode_db.as_deref(),
                    )
                }),
                "cline" => run_one(&source.key, || scan_cline(conn, &roots.cline)),
                _ => SourceStatus {
                    source: source.key.clone(),
                    events_inserted: 0,
                    lines_skipped: 0,
                    error: Some("unsupported source catalog entry".to_string()),
                },
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

    ScanStatus {
        sources,
        scanned_at,
    }
}

fn unavailable_source_status(source: &str, error: String) -> SourceStatus {
    SourceStatus {
        source: source.to_string(),
        events_inserted: 0,
        lines_skipped: 0,
        error: Some(error),
    }
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
            catalog
                .sources
                .iter()
                .map(|source| source.key.as_str())
                .collect::<Vec<_>>(),
            [
                "claude",
                "codex",
                "gemini",
                "hermes",
                "grok",
                "antigravity",
                "goose",
                "opencode",
                "cline",
                "pi"
            ],
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
            catalog
                .sources
                .iter()
                .filter(|source| source.capabilities.context)
                .map(|source| source.key.as_str())
                .collect::<Vec<_>>(),
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
                ("goose", "sessions", ".local/share/goose/sessions"),
                ("goose", "sessions-macos", "Library/Application Support/Block/goose/data/sessions"),
                ("goose", "sessions-windows", "AppData/Roaming/Block/goose/data/sessions"),
                ("opencode", "data", ".local/share/opencode"),
                ("opencode", "db", ".local/share/opencode/opencode.db"),
                ("opencode", "channel-db", ".local/share/opencode/opencode-<channel>.db"),
                ("opencode", "legacy-storage", ".local/share/opencode/storage"),
                ("opencode", "xdg-data", "opencode"),
                ("cline", "editor-code-macos", "Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev/tasks"),
                ("cline", "editor-code-insiders-macos", "Library/Application Support/Code - Insiders/User/globalStorage/saoudrizwan.claude-dev/tasks"),
                ("cline", "editor-code-linux", ".config/Code/User/globalStorage/saoudrizwan.claude-dev/tasks"),
                ("cline", "editor-code-insiders-linux", ".config/Code - Insiders/User/globalStorage/saoudrizwan.claude-dev/tasks"),
                ("cline", "editor-code-windows", "AppData/Roaming/Code/User/globalStorage/saoudrizwan.claude-dev/tasks"),
                ("cline", "editor-code-insiders-windows", "AppData/Roaming/Code - Insiders/User/globalStorage/saoudrizwan.claude-dev/tasks"),
                ("cline", "editor-server", ".vscode-server/data/User/globalStorage/saoudrizwan.claude-dev/tasks"),
                ("cline", "editor-server-insiders", ".vscode-server-insiders/data/User/globalStorage/saoudrizwan.claude-dev/tasks"),
                ("cline", "cli-default-data", ".cline/data"),
                ("pi", "sessions", ".pi/agent/sessions"),
            ],
        );
        assert!(catalog
            .sources
            .iter()
            .flat_map(|source| &source.artifacts)
            .all(|artifact| {
                matches!(artifact.kind.as_str(), "directory" | "file")
                    && !artifact.platforms.is_empty()
                    && artifact.prerequisite.is_none()
            }));

        let claude = crate::source_catalog::source("claude").unwrap();
        assert_eq!(claude.source, "Claude Code");
        assert!(claude
            .artifacts
            .iter()
            .any(|artifact| artifact.path.as_deref() == Some(".claude/projects")));

        let hermes = crate::source_catalog::source("hermes").unwrap();
        assert_eq!(
            hermes.artifacts[0].path.as_deref(),
            Some(".hermes/state.db")
        );

        let gemini = crate::source_catalog::source("gemini").unwrap();
        assert_eq!(
            gemini
                .artifacts
                .iter()
                .find(|artifact| artifact.id == "tmp")
                .and_then(|artifact| artifact.environment.as_deref()),
            Some("GEMINI_CLI_HOME")
        );

        let grok = crate::source_catalog::source("grok").unwrap();
        assert_eq!(
            grok.artifacts
                .iter()
                .find(|artifact| artifact.id == "sessions")
                .and_then(|artifact| artifact.environment.as_deref()),
            Some("GROK_HOME")
        );

        let pi = crate::source_catalog::source("pi").unwrap();
        assert_eq!(
            pi.artifacts
                .iter()
                .map(|artifact| artifact.id.as_str())
                .collect::<Vec<_>>(),
            ["sessions", "session-dir", "agent-dir"]
        );
        assert_eq!(pi.artifacts[0].path.as_deref(), Some(".pi/agent/sessions"));
        assert_eq!(
            pi.artifacts[1].environment.as_deref(),
            Some("PI_CODING_AGENT_SESSION_DIR")
        );
        assert_eq!(
            pi.artifacts[2].environment.as_deref(),
            Some("PI_CODING_AGENT_DIR")
        );
        assert_eq!(pi.artifacts[2].suffix.as_deref(), Some("sessions"));

        let goose = crate::source_catalog::source("goose").unwrap();
        assert_eq!(goose.source, "Goose");
        assert_eq!(goose.aliases, ["Block Goose"]);
        assert_eq!(
            goose
                .artifacts
                .iter()
                .map(|artifact| artifact.id.as_str())
                .collect::<Vec<_>>(),
            ["sessions", "sessions-macos", "sessions-windows", "root"]
        );
        assert_eq!(
            goose.artifacts[0].path.as_deref(),
            Some(".local/share/goose/sessions")
        );
        assert_eq!(
            goose.artifacts[3].environment.as_deref(),
            Some("GOOSE_PATH_ROOT")
        );
        assert_eq!(goose.artifacts[3].suffix.as_deref(), Some("data/sessions"));

        let opencode = crate::source_catalog::source("opencode").unwrap();
        assert_eq!(opencode.source, "OpenCode");
        assert_eq!(opencode.aliases, ["OpenCode CLI"]);
        assert_eq!(
            opencode
                .artifacts
                .iter()
                .map(|artifact| artifact.id.as_str())
                .collect::<Vec<_>>(),
            ["data", "db", "channel-db", "legacy-storage", "xdg-data"]
        );
        assert_eq!(
            opencode.artifacts[0].environment.as_deref(),
            Some("OPENCODE_DATA_DIR")
        );
        assert_eq!(
            opencode.artifacts[1].environment.as_deref(),
            Some("OPENCODE_DB")
        );
        assert_eq!(opencode.artifacts[3].suffix.as_deref(), Some("storage"));
        assert_eq!(
            opencode.artifacts[4].environment.as_deref(),
            Some("XDG_DATA_HOME")
        );

        let cline = crate::source_catalog::source("cline").unwrap();
        assert_eq!(cline.source, "Cline");
        assert_eq!(cline.aliases, ["Cline CLI", "Cline VS Code"]);
        assert_eq!(
            cline
                .artifacts
                .iter()
                .find(|artifact| artifact.id == "cli-data")
                .and_then(|artifact| artifact.environment.as_deref()),
            Some("CLINE_DATA_DIR")
        );
        assert_eq!(
            cline
                .artifacts
                .iter()
                .find(|artifact| artifact.id == "cli-sandbox")
                .and_then(|artifact| artifact.environment.as_deref()),
            Some("CLINE_SANDBOX_DATA_DIR")
        );
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
    fn opencode_roots_use_visible_data_database_and_xdg_overrides() {
        use std::ffi::OsStr;

        let home = tempfile::tempdir().unwrap();
        let configured_db = home.path().join("configured/opencode.db");
        let configured = opencode_roots(
            home.path(),
            Some(OsStr::new("~/configured-opencode")),
            Some(OsStr::new(configured_db.to_str().unwrap())),
            None,
        );
        assert_eq!(configured.0, home.path().join("configured-opencode"));
        assert_eq!(
            configured.1,
            home.path().join("configured-opencode/storage")
        );
        assert_eq!(configured.2, Some(configured_db));

        let xdg = opencode_roots(
            home.path(),
            None,
            None,
            Some(OsStr::new("~/configured-data")),
        );
        assert_eq!(xdg.0, home.path().join("configured-data/opencode"));
        assert_eq!(xdg.1, home.path().join("configured-data/opencode/storage"));
        assert_eq!(xdg.2, None);

        let blank = opencode_roots(
            home.path(),
            Some(OsStr::new("  ")),
            Some(OsStr::new("  ")),
            Some(OsStr::new("  ")),
        );
        assert_eq!(blank.0, home.path().join(".local/share/opencode"));
        assert_eq!(blank.2, None);
    }

    #[test]
    fn hermes_home_override_is_used_and_blank_value_falls_back() {
        use std::ffi::OsStr;

        let home = tempfile::tempdir().unwrap();
        let override_home = home.path().join("configured-hermes");
        let overridden = SourceRoots::from_home_and_pi_env_with_hermes_and_gemini_and_grok(
            home.path(),
            None,
            None,
            Some(OsStr::new(override_home.to_str().unwrap())),
            None,
            None,
        );
        assert_eq!(overridden.hermes_db, override_home.join("state.db"));

        let blank = SourceRoots::from_home_and_pi_env_with_hermes_and_gemini_and_grok(
            home.path(),
            None,
            None,
            Some(OsStr::new("  ")),
            None,
            None,
        );
        assert_eq!(blank.hermes_db, home.path().join(".hermes/state.db"));

        let absent = SourceRoots::from_home_and_pi_env_with_hermes_and_gemini_and_grok(
            home.path(),
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(absent.hermes_db, home.path().join(".hermes/state.db"));
    }

    #[test]
    fn gemini_cli_home_override_is_nested_and_blank_value_falls_back() {
        use std::ffi::OsStr;

        let home = tempfile::tempdir().unwrap();
        let overridden = SourceRoots::from_home_and_pi_env_with_hermes_and_gemini_and_grok(
            home.path(),
            None,
            None,
            None,
            Some(OsStr::new("~/configured-gemini")),
            None,
        );
        assert_eq!(
            overridden.gemini_tmp,
            home.path().join("configured-gemini/.gemini/tmp")
        );
        assert_eq!(
            overridden.gemini_projects_json,
            home.path().join("configured-gemini/.gemini/projects.json")
        );

        let blank = SourceRoots::from_home_and_pi_env_with_hermes_and_gemini_and_grok(
            home.path(),
            None,
            None,
            None,
            Some(OsStr::new("  ")),
            None,
        );
        assert_eq!(blank.gemini_tmp, home.path().join(".gemini/tmp"));
        assert_eq!(
            blank.gemini_projects_json,
            home.path().join(".gemini/projects.json")
        );

        let absent = SourceRoots::from_home_and_pi_env_with_hermes_and_gemini_and_grok(
            home.path(),
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(absent.gemini_tmp, home.path().join(".gemini/tmp"));
        assert_eq!(
            absent.gemini_projects_json,
            home.path().join(".gemini/projects.json")
        );
    }

    #[test]
    fn grok_home_override_is_used_and_blank_value_falls_back() {
        use std::ffi::OsStr;

        let home = tempfile::tempdir().unwrap();
        let overridden = SourceRoots::from_home_and_pi_env_with_hermes_and_gemini_and_grok(
            home.path(),
            None,
            None,
            None,
            None,
            Some(OsStr::new("~/configured-grok")),
        );
        assert_eq!(
            overridden.grok_sessions,
            home.path().join("configured-grok/sessions")
        );

        let blank = SourceRoots::from_home_and_pi_env_with_hermes_and_gemini_and_grok(
            home.path(),
            None,
            None,
            None,
            None,
            Some(OsStr::new("  ")),
        );
        assert_eq!(blank.grok_sessions, home.path().join(".grok/sessions"));

        let absent = SourceRoots::from_home_and_pi_env_with_hermes_and_gemini_and_grok(
            home.path(),
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(absent.grok_sessions, home.path().join(".grok/sessions"));
    }

    #[test]
    fn goose_roots_cover_platform_defaults_legacy_storage_and_absolute_override() {
        use std::ffi::OsStr;

        let home = tempfile::tempdir().unwrap();
        assert_eq!(
            goose_session_roots(home.path(), None, "linux"),
            vec![home.path().join(".local/share/goose/sessions")]
        );
        assert_eq!(
            goose_session_roots(home.path(), None, "macos"),
            vec![
                home.path()
                    .join("Library/Application Support/Block/goose/data/sessions"),
                home.path().join(".local/share/goose/sessions"),
            ]
        );
        assert_eq!(
            goose_session_roots(home.path(), Some(OsStr::new("~/configured-goose")), "macos"),
            vec![home.path().join("configured-goose/data/sessions")]
        );
        assert_eq!(
            goose_session_roots(home.path(), Some(OsStr::new("relative-goose")), "linux"),
            vec![home.path().join(".local/share/goose/sessions")]
        );
    }

    #[test]
    fn cline_cli_root_precedence_ignores_blank_values_and_deduplicates_equivalents() {
        use std::ffi::OsStr;

        let home = tempfile::tempdir().unwrap();
        let explicit = home.path().join("configured-cline");
        let sandbox = home.path().join("sandbox-cline");
        let overridden = cline_roots(
            home.path(),
            "linux",
            Some(explicit.as_os_str()),
            Some(sandbox.as_os_str()),
        );
        assert!(overridden.contains(&explicit));
        assert!(!overridden.contains(&sandbox));

        let blank_data = cline_roots(
            home.path(),
            "linux",
            Some(OsStr::new(" \t")),
            Some(OsStr::new("~/sandbox-cline")),
        );
        assert!(blank_data.contains(&sandbox));

        let defaults = cline_roots(home.path(), "linux", None, None);
        assert!(defaults.contains(&home.path().join(".cline/data")));

        let equivalent = cline_roots(
            home.path(),
            "linux",
            Some(OsStr::new("~/.cline/../.cline/data")),
            None,
        );
        assert_eq!(
            equivalent
                .iter()
                .filter(|path| normalized_path(path)
                    == normalized_path(&home.path().join(".cline/data")))
                .count(),
            1
        );
    }

    #[test]
    fn default_roots_live_under_home() {
        let r = SourceRoots::default_roots();
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        assert!(r.claude.ends_with(".claude/projects"));
        assert!(r.codex.ends_with(".codex/sessions"));
        assert!(r.gemini_tmp.ends_with(".gemini/tmp"));
        assert!(r.gemini_projects_json.ends_with(".gemini/projects.json"));
        assert!(r.hermes_db.ends_with(".hermes/state.db"));
        let grok_home = std::env::var_os("GROK_HOME")
            .and_then(|value| visible_path(&home, &value))
            .unwrap_or_else(|| home.join(".grok"));
        assert_eq!(r.grok_sessions, grok_home.join("sessions"));
        assert!(r
            .antigravity_conversations
            .ends_with(".gemini/antigravity/conversations"));
        assert!(r
            .antigravity_cli_conversations
            .ends_with(".gemini/antigravity-cli/conversations"));
        assert!(r.pi_sessions[0].ends_with(".pi/agent/sessions"));
        assert!(!r.goose_sessions.is_empty());
        assert!(r.cline.iter().any(|path| path.ends_with(".cline/data")));
        assert!(r.cline.iter().any(|path| path
            .ends_with(".vscode-server/data/User/globalStorage/saoudrizwan.claude-dev/tasks")));
    }

    #[test]
    fn run_scan_backfills_gemini_override_queries_cost_and_retains_disappeared_usage() {
        std::env::set_var("TZ", "UTC");
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let gemini_home = base.join("configured-gemini");
        let roots = SourceRoots::from_home_and_pi_env_with_hermes_and_gemini_and_grok(
            base,
            None,
            None,
            None,
            Some(gemini_home.as_os_str()),
            None,
        );
        let session_path = roots.gemini_tmp.join("alpha/chats/session-override.json");
        fs::create_dir_all(session_path.parent().unwrap()).unwrap();
        fs::write(
            &roots.gemini_projects_json,
            r#"{"projects":{"/Users/dev/projects/gemini-demo":"alpha"}}"#,
        )
        .unwrap();
        fs::write(
            &session_path,
            r#"{"sessionId":"gemini-override","messages":[
              {"id":"m1","timestamp":"2026-07-01T10:00:00.000Z","model":"gemini-priced",
               "content":"GEMINI_PRIVATE_PROMPT_MARKER",
               "tokens":{"input":100,"output":20,"cached":30,"thoughts":5,"tool":10,"total":125}}
            ]}"#,
        )
        .unwrap();

        let mut conn = open_db(&base.join("ledger.db")).unwrap();
        pricing::set_override(
            &conn,
            "gemini-priced",
            OverrideRates {
                input: Some(1.0),
                output: Some(2.0),
                cache_read: Some(3.0),
                cache_write: Some(4.0),
            },
        )
        .unwrap();

        let first = run_scan(&mut conn, &roots);
        let gemini = find(&first, "gemini");
        assert_eq!(gemini.events_inserted, 1);
        assert!(gemini.error.is_none());

        let summary = queries::summary(&conn, &Filters::default()).unwrap();
        assert_eq!(summary.input_tokens, 70);
        assert_eq!(summary.output_tokens, 25);
        assert_eq!(summary.cache_read_tokens, 30);
        assert_eq!(summary.total_tokens, 125);
        assert_eq!(summary.requests, 1);
        assert_eq!(summary.cost, Some(210.0));

        let source_rows = queries::breakdown(&conn, "tool", &Filters::default()).unwrap();
        let gemini_row = source_rows
            .iter()
            .find(|row| row.key.as_deref() == Some("gemini"))
            .unwrap();
        assert_eq!(gemini_row.total_tokens, 125);
        assert_eq!(gemini_row.requests, 1);
        assert_eq!(gemini_row.cost, Some(210.0));

        let series = queries::series(&conn, &Filters::default(), "day").unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].source, "gemini");
        assert_eq!(series[0].total_tokens, 125);
        assert_eq!(series[0].requests, 1);
        assert_eq!(series[0].cost, 210.0);

        let second = run_scan(&mut conn, &roots);
        assert_eq!(find(&second, "gemini").events_inserted, 0);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );

        fs::remove_file(&session_path).unwrap();
        let after_disappearance = run_scan(&mut conn, &roots);
        assert_eq!(find(&after_disappearance, "gemini").events_inserted, 0);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1,
            "disappearing Source Artifacts do not delete Ledger history"
        );

        let durable = fs::read(base.join("ledger.db")).unwrap();
        assert!(!durable
            .windows("GEMINI_PRIVATE_PROMPT_MARKER".len())
            .any(|window| window == b"GEMINI_PRIVATE_PROMPT_MARKER"));
    }

    #[test]
    fn run_scan_backfills_grok_override_queries_cost_and_retains_disappeared_usage() {
        std::env::set_var("TZ", "UTC");
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let grok_home = base.join("configured-grok");
        let roots = SourceRoots::from_home_and_pi_env_with_hermes_and_gemini_and_grok(
            base,
            None,
            None,
            None,
            None,
            Some(grok_home.as_os_str()),
        );
        let updates_path = roots
            .grok_sessions
            .join("%2FUsers%2Fdev%2Fprojects%2Fgrok-demo/sess-override/updates.jsonl");
        fs::create_dir_all(updates_path.parent().unwrap()).unwrap();
        fs::write(
            &updates_path,
            concat!(
                r#"{"timestamp":1780287300,"method":"session/update","params":{"sessionId":"sess-override","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"GROK_PRIVATE_PROMPT_MARKER"}}}}"#,
                "\n",
                r#"{"timestamp":1780287301,"method":"session/update","params":{"sessionId":"sess-override","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"GROK_PRIVATE_RESPONSE_MARKER"}},"_meta":{"totalTokens":100}}}"#,
                "\n",
            ),
        )
        .unwrap();
        fs::write(
            updates_path.parent().unwrap().join("summary.json"),
            r#"{"info":{"id":"sess-override","cwd":"/Users/dev/projects/grok-demo"},"current_model_id":"grok-priced","updated_at":"2026-07-01T10:00:00Z"}"#,
        )
        .unwrap();

        let mut conn = open_db(&base.join("ledger.db")).unwrap();
        pricing::set_override(
            &conn,
            "grok-priced",
            OverrideRates {
                input: Some(1.0),
                output: Some(2.0),
                cache_read: Some(3.0),
                cache_write: Some(4.0),
            },
        )
        .unwrap();

        let first = run_scan(&mut conn, &roots);
        let grok = find(&first, "grok");
        assert_eq!(grok.events_inserted, 1);
        assert!(grok.error.is_none());

        let summary = queries::summary(&conn, &Filters::default()).unwrap();
        assert_eq!(summary.input_tokens, 100);
        assert_eq!(summary.output_tokens, 0);
        assert_eq!(summary.total_tokens, 100);
        assert_eq!(summary.requests, 1);
        assert_eq!(summary.cost, Some(100.0));

        let source_rows = queries::breakdown(&conn, "tool", &Filters::default()).unwrap();
        let grok_row = source_rows
            .iter()
            .find(|row| row.key.as_deref() == Some("grok"))
            .unwrap();
        assert_eq!(grok_row.total_tokens, 100);
        assert_eq!(grok_row.requests, 1);
        assert_eq!(grok_row.cost, Some(100.0));

        let series = queries::series(&conn, &Filters::default(), "day").unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].source, "grok");
        assert_eq!(series[0].total_tokens, 100);
        assert_eq!(series[0].requests, 1);
        assert_eq!(series[0].cost, 100.0);

        let second = run_scan(&mut conn, &roots);
        assert_eq!(find(&second, "grok").events_inserted, 0);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );

        fs::remove_dir_all(&grok_home).unwrap();
        let after_disappearance = run_scan(&mut conn, &roots);
        assert_eq!(find(&after_disappearance, "grok").events_inserted, 0);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1,
            "disappearing Source Artifacts do not delete Ledger history"
        );

        drop(conn);
        for db_path in [
            base.join("ledger.db"),
            base.join("ledger.db-wal"),
            base.join("ledger.db-shm"),
        ] {
            if let Ok(durable) = fs::read(db_path) {
                for marker in ["GROK_PRIVATE_PROMPT_MARKER", "GROK_PRIVATE_RESPONSE_MARKER"] {
                    assert!(!durable
                        .windows(marker.len())
                        .any(|window| window == marker.as_bytes()));
                }
            }
        }
    }

    #[test]
    fn unavailable_catalog_source_reports_an_isolated_status_without_scanning() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("claude");
        fs::create_dir_all(root.join("project")).unwrap();
        fs::write(
            root.join("project/session.jsonl"),
            format!("{CLAUDE_LINE}\n"),
        )
        .unwrap();
        let roots = SourceRoots {
            claude: root,
            codex: tmp.path().join("codex"),
            gemini_tmp: tmp.path().join("gemini/tmp"),
            gemini_projects_json: tmp.path().join("gemini/projects.json"),
            hermes_db: tmp.path().join("hermes/state.db"),
            grok_sessions: tmp.path().join("grok"),
            antigravity_conversations: tmp.path().join("antigravity"),
            antigravity_cli_conversations: tmp.path().join("antigravity-cli"),
            goose_sessions: vec![tmp.path().join("goose")],
            pi_sessions: vec![tmp.path().join("pi")],
            opencode_data: tmp.path().join("opencode"),
            opencode_legacy: tmp.path().join("opencode/storage"),
            opencode_db: None,
            cline: vec![tmp.path().join("cline")],
        };
        let mut claude = crate::source_catalog::source("claude").unwrap().clone();
        claude.prerequisite = Some("Claude service".to_string());

        let mut conn = open_db(&tmp.path().join("ledger.db")).unwrap();
        let status = run_scan_sources(&mut conn, &roots, &[claude], std::env::consts::OS);

        assert_eq!(status.sources.len(), 1);
        assert_eq!(status.sources[0].source, "claude");
        assert_eq!(status.sources[0].events_inserted, 0);
        assert_eq!(status.sources[0].lines_skipped, 0);
        assert_eq!(
            status.sources[0].error.as_deref(),
            Some("unavailable: external prerequisite required: Claude service")
        );
        let event_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            event_count, 0,
            "ineligible source must not invoke its scanner"
        );
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
            goose_sessions: vec![base.join("no-goose")],
            pi_sessions: vec![pi_root],
            opencode_data: base.join("no-opencode"),
            opencode_legacy: base.join("no-opencode/storage"),
            opencode_db: None,
            cline: vec![base.join("no-cline")],
        };

        let db_path = base.join("ledger.db");
        let mut conn = open_db(&db_path).unwrap();
        conn.execute(
            "INSERT INTO prices (model, input_per_tok, output_per_tok, cache_read_per_tok, cache_write_5m_per_tok, cache_write_1h_per_tok) \
             VALUES ('pi-response-model', 0.000002, 0.000010, 0.0000005, 0.0000025, 0.000004)",
            [],
        ).unwrap();
        pricing::set_override(
            &conn,
            "pi-fallback-model",
            OverrideRates {
                input: Some(0.000001),
                output: Some(0.000002),
                cache_read: None,
                cache_write: None,
            },
        )
        .unwrap();

        let status = run_scan(&mut conn, &roots);
        assert_eq!(status.sources.len(), 10);
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
        assert!(
            (summary.cost.unwrap() - 0.000805).abs() < 1e-12,
            "Unattributed usage adds no Cost"
        );
        assert_eq!(summary.unattributed_tokens, 3600);
        assert!(summary.has_unpriced);
        assert_eq!(summary.unpriced_models, vec!["pi-error-model".to_string()]);

        let source_rows = queries::breakdown(&conn, "tool", &Filters::default()).unwrap();
        let pi_source = source_rows
            .iter()
            .find(|r| r.key.as_deref() == Some("pi"))
            .unwrap();
        assert_eq!(pi_source.total_tokens, 3839);
        assert_eq!(pi_source.requests, 4);
        assert!((pi_source.cost.unwrap() - 0.000805).abs() < 1e-12);
        assert!(pi_source.has_unpriced);

        // The model breakdown keeps a null-Model row for the Unattributed usage,
        // distinct from the three real pi Models.
        let model_rows = queries::breakdown(&conn, "model", &Filters::default()).unwrap();
        assert_eq!(model_rows.len(), 4);
        assert!(model_rows.iter().all(|r| r.source.as_deref() == Some("pi")));
        assert!(model_rows
            .iter()
            .all(|r| r.key.as_deref() != Some("pi-selected-model")));
        assert_eq!(model_rows.iter().filter(|r| r.key.is_none()).count(), 1);
        let response = model_rows
            .iter()
            .find(|r| r.key.as_deref() == Some("pi-response-model"))
            .unwrap();
        assert_eq!(response.reasoning_tokens, Some(10));

        let projects = queries::breakdown(&conn, "project", &Filters::default()).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(
            projects[0].key.as_deref(),
            Some("/Users/dev/projects/pi-demo")
        );
        assert_eq!(projects[0].total_tokens, 3839);

        let series = queries::series(&conn, &Filters::default(), "day").unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].source, "pi");
        assert_eq!(series[0].total_tokens, 3839);
        assert_eq!(series[0].requests, 4);
        assert_eq!(series[0].cache_write_tokens, 919);
        assert!((series[0].cost - 0.000805).abs() < 1e-12);
        assert_eq!(
            series[0].by_model.len(),
            3,
            "per-model series omits the null Model"
        );

        let pricing_rows = pricing::model_pricing(&conn).unwrap();
        for model in ["pi-response-model", "pi-fallback-model", "pi-error-model"] {
            let row = pricing_rows.iter().find(|r| r.model == model).unwrap();
            assert_eq!(row.tool, "pi");
        }
        assert!(pricing_rows.iter().all(|r| r.model != "pi-selected-model"));

        let second = run_scan(&mut conn, &roots);
        assert_eq!(
            find(&second, "pi").events_inserted,
            0,
            "repeat scan is idempotent"
        );
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM events WHERE source = 'pi'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 4);

        fs::remove_file(session_path).unwrap();
        let after_disappearance = run_scan(&mut conn, &roots);
        assert_eq!(find(&after_disappearance, "pi").events_inserted, 0);
        assert!(find(&after_disappearance, "pi").error.is_none());
        assert!(get_file_state(&conn, &source_file).unwrap().is_none());
        let retained: i64 = conn
            .query_row("SELECT COUNT(*) FROM events WHERE source = 'pi'", [], |r| {
                r.get(0)
            })
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
                !durable_bytes
                    .windows(private.len())
                    .any(|w| w == private.as_bytes()),
                "private fixture content reached the Ledger: {private}",
            );
        }
    }

    fn build_hermes_fixture(
        path: &Path,
        rows: &[(&str, Option<&str>, i64, i64, i64, i64, i64, i64, i64, &str)],
    ) {
        let parent = path.parent().unwrap();
        fs::create_dir_all(parent).unwrap();
        let src = Connection::open(path).unwrap();
        src.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                model TEXT,
                started_at REAL NOT NULL,
                input_tokens INTEGER,
                output_tokens INTEGER,
                cache_read_tokens INTEGER,
                cache_write_tokens INTEGER,
                reasoning_tokens INTEGER,
                api_call_count INTEGER,
                cwd TEXT
            );",
        )
        .unwrap();
        for (
            id,
            model,
            started_at,
            input,
            output,
            cache_read,
            cache_write,
            reasoning,
            calls,
            cwd,
        ) in rows
        {
            src.execute(
                "INSERT INTO sessions
                 (id, model, started_at, input_tokens, output_tokens, cache_read_tokens,
                  cache_write_tokens, reasoning_tokens, api_call_count, cwd)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    id,
                    model,
                    *started_at as f64,
                    input,
                    output,
                    cache_read,
                    cache_write,
                    reasoning,
                    calls,
                    cwd
                ],
            )
            .unwrap();
        }
    }

    #[test]
    fn run_scan_backfills_hermes_profiles_queries_cost_and_retains_disappeared_usage() {
        std::env::set_var("TZ", "UTC");
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let hermes_root = base.join("hermes");
        let primary = hermes_root.join("state.db");
        let profile = hermes_root.join("profiles/coder/state.db");
        let malformed = hermes_root.join("profiles/broken/state.db");
        build_hermes_fixture(
            &primary,
            &[(
                "default",
                Some("hermes-priced"),
                1780287300,
                10,
                5,
                0,
                0,
                0,
                2,
                "",
            )],
        );
        build_hermes_fixture(
            &profile,
            &[("profile", None, 1780287301, 4, 2, 0, 0, 0, 1, "")],
        );
        fs::create_dir_all(malformed.parent().unwrap()).unwrap();
        fs::write(&malformed, b"not a Hermes sqlite database").unwrap();

        let roots = SourceRoots::from_home_and_pi_env_with_hermes_and_gemini_and_grok(
            base,
            None,
            None,
            Some(hermes_root.as_os_str()),
            None,
            None,
        );
        let mut conn = open_db(&base.join("ledger.db")).unwrap();
        pricing::set_override(
            &conn,
            "hermes-priced",
            OverrideRates {
                input: Some(1.0),
                output: Some(1.0),
                cache_read: Some(1.0),
                cache_write: Some(1.0),
            },
        )
        .unwrap();

        let first = run_scan(&mut conn, &roots);
        let hermes = find(&first, "hermes");
        assert_eq!(hermes.events_inserted, 2);
        assert!(hermes
            .error
            .as_deref()
            .is_some_and(|error| error.contains("hermes")));

        let summary = queries::summary(&conn, &Filters::default()).unwrap();
        assert_eq!(summary.total_tokens, 21);
        assert_eq!(summary.requests, 3);
        assert_eq!(summary.unattributed_tokens, 6);
        assert!(!summary.has_unpriced);
        assert_eq!(summary.cost, Some(15.0));

        let source_rows = queries::breakdown(&conn, "tool", &Filters::default()).unwrap();
        let hermes_row = source_rows
            .iter()
            .find(|row| row.key.as_deref() == Some("hermes"))
            .unwrap();
        assert_eq!(hermes_row.total_tokens, 21);
        assert_eq!(hermes_row.requests, 3);
        assert_eq!(hermes_row.cost, Some(15.0));

        let second = run_scan(&mut conn, &roots);
        assert_eq!(find(&second, "hermes").events_inserted, 0);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM events", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            2
        );

        fs::remove_file(&primary).unwrap();
        fs::remove_file(&profile).unwrap();
        let after_disappearance = run_scan(&mut conn, &roots);
        assert_eq!(find(&after_disappearance, "hermes").events_inserted, 0);
        assert!(
            conn.query_row("SELECT COUNT(*) FROM events", [], |row| row
                .get::<_, i64>(0))
                .unwrap()
                == 2
        );

        fs::remove_file(&malformed).unwrap();
        let missing = run_scan(&mut conn, &roots);
        assert!(find(&missing, "hermes").error.is_none());
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
        // reach the Ledger, and the other Sources must still report.
        let pi_root = base.join("pi");
        fs::create_dir_all(&pi_root).unwrap();
        fs::write(pi_root.join("a-broken.jsonl"), [0xff, b'\n']).unwrap();
        fs::write(pi_root.join("b-valid.jsonl"), PI_SESSION).unwrap();

        // Gemini has an existing malformed Artifact; the warning must stay
        // Source-specific while Claude and pi continue scanning.
        let gemini_root = base.join("gemini");
        fs::create_dir_all(gemini_root.join("proj/chats")).unwrap();
        fs::write(
            gemini_root.join("proj/chats/session-broken.json"),
            "{ not json",
        )
        .unwrap();
        let gemini_projects = base.join("gemini-projects.json");
        fs::write(&gemini_projects, r#"{"projects":{}}"#).unwrap();

        // Everything else points at paths that do not exist.
        let roots = SourceRoots {
            claude: claude_root,
            codex: base.join("no-codex"),
            gemini_tmp: gemini_root,
            gemini_projects_json: gemini_projects,
            hermes_db: base.join("no-hermes.db"),
            grok_sessions: base.join("no-grok"),
            antigravity_conversations: base.join("no-antigravity"),
            antigravity_cli_conversations: base.join("no-antigravity-cli"),
            goose_sessions: vec![base.join("no-goose")],
            pi_sessions: vec![pi_root],
            opencode_data: base.join("no-opencode"),
            opencode_legacy: base.join("no-opencode/storage"),
            opencode_db: None,
            cline: vec![base.join("no-cline")],
        };

        let mut conn = open_db(&base.join("ledger.db")).unwrap();
        let status = run_scan(&mut conn, &roots);

        assert_eq!(status.sources.len(), 10);
        assert_eq!(status.sources.last().unwrap().source, "pi");
        assert!(status.scanned_at > 0);

        // Claude still ingests its event even though Gemini reports a warning.
        let claude = find(&status, "claude");
        assert_eq!(claude.events_inserted, 1);
        assert!(claude.error.is_none());

        // Missing directories → zero events, no error.
        let codex = find(&status, "codex");
        assert_eq!(codex.events_inserted, 0);
        assert!(codex.error.is_none());
        let gemini = find(&status, "gemini");
        assert_eq!(gemini.events_inserted, 0);
        assert!(gemini
            .error
            .as_deref()
            .is_some_and(|error| error.contains("gemini") && error.contains("malformed")));

        // Nonexistent Hermes DB → quiet empty Source; other sources unaffected.
        let hermes = find(&status, "hermes");
        assert_eq!(hermes.events_inserted, 0);
        assert!(hermes.error.is_none());

        // Missing directory-shaped roots → zero events, no error.
        let grok = find(&status, "grok");
        assert_eq!(grok.events_inserted, 0);
        assert!(grok.error.is_none());
        let antigravity = find(&status, "antigravity");
        assert_eq!(antigravity.events_inserted, 0);
        assert!(antigravity.error.is_none());
        let pi = find(&status, "pi");
        assert_eq!(
            pi.events_inserted, 4,
            "valid pi file survives broken sibling"
        );
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

    #[test]
    fn run_scan_ingests_opencode_current_and_legacy_sessions_at_session_granularity() {
        std::env::set_var("TZ", "UTC");
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let opencode_root = base.join("opencode");
        let database = opencode_root.join("opencode.db");
        fs::create_dir_all(&opencode_root).unwrap();
        let source = rusqlite::Connection::open(&database).unwrap();
        source
            .execute_batch(
                "CREATE TABLE session (
                    id TEXT PRIMARY KEY,
                    directory TEXT NOT NULL,
                    time_created INTEGER NOT NULL,
                    time_updated INTEGER NOT NULL
                );
                CREATE TABLE message (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    time_created INTEGER NOT NULL,
                    data TEXT NOT NULL
                );
                INSERT INTO session VALUES ('opencode-s1', '/Users/dev/project', 1780000000000, 1780000100000);",
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO message VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    "m1",
                    "opencode-s1",
                    1780000000000i64,
                    r#"{"role":"assistant","modelID":"opencode-model","tokens":{"input":30,"output":8,"cache":{"read":10,"write":2}}}"#,
                ],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO message VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    "m2",
                    "opencode-s1",
                    1780000001000i64,
                    r#"{"role":"assistant","modelID":"opencode-model","tokens":{"input":5,"output":2,"cache":{"read":1,"write":0}}}"#,
                ],
            )
            .unwrap();
        drop(source);

        let legacy = opencode_root.join("storage");
        fs::create_dir_all(legacy.join("session/project")).unwrap();
        fs::create_dir_all(legacy.join("message/legacy-s1")).unwrap();
        fs::write(
            legacy.join("session/project/legacy-s1.json"),
            r#"{"id":"legacy-s1","directory":"/Users/dev/legacy","time":{"updated":1780000200000}}"#,
        )
        .unwrap();
        fs::write(
            legacy.join("message/legacy-s1/msg.json"),
            r#"{"role":"assistant","modelID":"legacy-model","tokens":{"input":7,"output":3,"cache":{"read":2,"write":1}}}"#,
        )
        .unwrap();

        let roots = SourceRoots {
            claude: base.join("no-claude"),
            codex: base.join("no-codex"),
            gemini_tmp: base.join("no-gemini"),
            gemini_projects_json: base.join("no-projects.json"),
            hermes_db: base.join("no-hermes.db"),
            grok_sessions: base.join("no-grok"),
            antigravity_conversations: base.join("no-antigravity"),
            antigravity_cli_conversations: base.join("no-antigravity-cli"),
            goose_sessions: vec![base.join("no-goose")],
            pi_sessions: vec![base.join("no-pi")],
            opencode_data: opencode_root.clone(),
            opencode_legacy: legacy,
            opencode_db: None,
            cline: vec![base.join("no-cline")],
        };
        let mut conn = open_db(&base.join("ledger.db")).unwrap();
        let first = run_scan(&mut conn, &roots);
        let opencode = find(&first, "opencode");
        assert_eq!(opencode.events_inserted, 2);
        assert!(
            opencode.error.is_none(),
            "unexpected error: {:?}",
            opencode.error
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM events WHERE source = 'opencode' AND api_calls = 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            2,
            "one Usage Record per OpenCode Session"
        );
        assert_eq!(
            conn.query_row(
                "SELECT timestamp FROM events WHERE session_id = 'opencode-s1'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1_780_000_100
        );

        let second = run_scan(&mut conn, &roots);
        assert_eq!(find(&second, "opencode").events_inserted, 0);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM events WHERE source = 'opencode'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
    }
}
