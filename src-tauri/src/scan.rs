use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::adapters::antigravity::scan_antigravity;
use crate::adapters::claude::scan_claude;
use crate::adapters::cline::scan_cline;
use crate::adapters::codebuddy::scan_codebuddy;
use crate::adapters::codex::scan_codex;
use crate::adapters::copilot::scan_copilot;
use crate::adapters::gemini::scan_gemini;
use crate::adapters::goose::scan_goose;
use crate::adapters::grok::scan_grok;
use crate::adapters::hermes::scan_hermes;
use crate::adapters::kilo::scan_kilo;
use crate::adapters::opencode::scan_opencode;
use crate::adapters::pi::scan_pi;
use crate::adapters::omp::scan_omp;
use crate::adapters::qoder::scan_qoder;
use crate::adapters::workbuddy::scan_workbuddy;
use crate::adapters::zed::scan_zed;
use crate::db::prune_missing_files;
use crate::limits_artifact;
use crate::source_catalog;
use crate::types::{ScanStatus, SourceScanResult, SourceStatus};

/// Where a Scan looks. Artifact paths come from the Source Catalog under
/// `home` (plus env overrides). `limit_exports` is app-owned (ADR-0019), not a
/// home Artifact. Tests plant a home and optionally overlay env values or a
/// path map — not a per-Source field bag.
pub struct SourceRoots {
    pub home: PathBuf,
    /// Where a Companion leaves its Limits Export Artifacts. Owned by the app
    /// rather than found under home; empty means no Companion has been given a
    /// place to write, and a missing directory is not an error.
    pub limit_exports: PathBuf,
    env: HashMap<String, OsString>,
    artifacts: BTreeMap<String, BTreeMap<String, Vec<PathBuf>>>,
    /// Production reads the process environment; a planted home (`at`) does not,
    /// so a developer's `HERMES_HOME` cannot leak into a fixture or validation.
    live_env: bool,
}

impl SourceRoots {
    pub fn default_roots() -> Self {
        let mut roots = Self::at(dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")));
        roots.live_env = true;
        roots
    }

    pub fn at(home: impl Into<PathBuf>) -> Self {
        Self {
            home: home.into(),
            limit_exports: PathBuf::new(),
            env: HashMap::new(),
            artifacts: BTreeMap::new(),
            live_env: false,
        }
    }

    pub fn with_limit_exports(mut self, path: PathBuf) -> Self {
        self.limit_exports = path;
        self
    }

    #[cfg(test)]
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<OsString>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    #[cfg(test)]
    pub fn with_artifact(mut self, source: &str, id: &str, path: PathBuf) -> Self {
        self.artifacts
            .entry(source.to_string())
            .or_default()
            .entry(id.to_string())
            .or_default()
            .push(path);
        self
    }

    /// Overlay `path` onto every catalog Artifact of `source`. Validation and
    /// tests plant one scan root without naming a per-Source Artifact id.
    #[cfg(test)]
    pub fn with_source_path(mut self, source: &str, path: PathBuf) -> Self {
        let ids: Vec<String> = source_catalog::source(source)
            .map(|definition| {
                definition
                    .artifacts
                    .iter()
                    .map(|artifact| artifact.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        for id in ids {
            self = self.with_artifact(source, &id, path.clone());
        }
        self
    }

    fn overlay(&self, source: &str, id: &str) -> Option<&[PathBuf]> {
        self.artifacts
            .get(source)?
            .get(id)
            .map(Vec::as_slice)
            .filter(|paths| !paths.is_empty())
    }

    /// The first planted path for one Artifact. Every resolver that answers with
    /// a single root starts here, so "an overlay wins" is written once.
    fn overlay_first(&self, source: &str, id: &str) -> Option<PathBuf> {
        self.overlay(source, id).map(|paths| paths[0].clone())
    }

    fn overlays_for(&self, source: &str) -> Option<Vec<PathBuf>> {
        let mut out = Vec::new();
        for paths in self.artifacts.get(source).into_iter().flat_map(BTreeMap::values) {
            for path in paths {
                push_unique_root(&mut out, path.clone());
            }
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    fn env_os(&self, name: &str) -> Option<OsString> {
        if let Some(value) = self.env.get(name) {
            return Some(value.clone());
        }
        if self.live_env {
            std::env::var_os(name)
        } else {
            None
        }
    }

    fn catalog_env(&self, source: &str, artifact: &str) -> Option<OsString> {
        let name = source_catalog::artifact(source, artifact)?.environment.as_deref()?;
        self.env_os(name)
    }

    fn artifact_path(&self, source: &str, id: &str) -> PathBuf {
        self.overlay_first(source, id)
            .unwrap_or_else(|| catalog_root(&self.home, source, id))
    }

    pub(crate) fn cline_roots(&self, platform: &str) -> Vec<PathBuf> {
        if let Some(paths) = self.overlays_for("cline") {
            return paths;
        }
        let mut out = Vec::new();
        for artifact in default_artifacts_on_platform("cline", platform) {
            // The chain below picks exactly one of these; scanning them here too
            // would add a second root for the same tasks directory.
            if CLINE_CLI_ROOT_CHAIN.contains(&artifact.id.as_str()) {
                continue;
            }
            if let Some(path) = artifact.path.as_deref() {
                push_unique_root(&mut out, self.home.join(path));
            }
        }

        let cli_root = self
            .catalog_env("cline", "cli-data")
            .and_then(|value| visible_path(&self.home, &value))
            .or_else(|| {
                self.catalog_env("cline", "cli-sandbox")
                    .and_then(|value| visible_path(&self.home, &value))
            })
            .or_else(|| catalog_root_for_platform(&self.home, "cline", "cli-default-data", platform));
        if let Some(path) = cli_root {
            push_unique_root(&mut out, path);
        }
        out
    }

    fn codex_session_roots(&self) -> Vec<PathBuf> {
        if let Some(paths) = self.overlay("codex", "sessions") {
            return paths.to_vec();
        }
        let mut out = vec![catalog_root(&self.home, "codex", "sessions")];
        let suffix = source_catalog::artifact("codex", "home")
            .and_then(|artifact| artifact.suffix.as_deref())
            .unwrap_or_else(|| panic!("source catalog must define codex.home suffix"));
        if let Some(root) = self
            .catalog_env("codex", "home")
            .and_then(|value| visible_path(&self.home, &value))
        {
            push_unique_root(&mut out, root.join(suffix));
        }
        out
    }

    pub(crate) fn pi_session_roots(&self) -> Vec<PathBuf> {
        self.session_roots_with_overrides("pi")
    }

    fn omp_session_roots(&self) -> Vec<PathBuf> {
        self.session_roots_with_overrides("omp")
    }

    fn session_roots_with_overrides(&self, source: &str) -> Vec<PathBuf> {
        if let Some(paths) = self.overlay(source, "sessions") {
            return paths.to_vec();
        }
        let mut out = vec![catalog_root(&self.home, source, "sessions")];
        append_session_override(
            &mut out,
            &self.home,
            source,
            "session-dir",
            self.catalog_env(source, "session-dir").as_deref(),
        );
        append_session_override(
            &mut out,
            &self.home,
            source,
            "agent-dir",
            self.catalog_env(source, "agent-dir").as_deref(),
        );
        out
    }

    pub(crate) fn goose_session_roots(&self, platform: &str) -> Vec<PathBuf> {
        if let Some(paths) = self.overlays_for("goose") {
            return paths;
        }
        if let Some(root) = self
            .catalog_env("goose", "root")
            .and_then(|value| visible_path(&self.home, &value))
            .filter(|path| path.is_absolute())
        {
            return vec![root.join(catalog_suffix("goose", "root"))];
        }
        // Catalog order is scan order: Goose's current per-platform directory is
        // listed before the pre-1.10 `.local/share/goose/sessions` it replaced.
        let mut out = Vec::new();
        for artifact in default_artifacts_on_platform("goose", platform) {
            if let Some(path) = artifact.path.as_deref() {
                push_unique_root(&mut out, self.home.join(path));
            }
        }
        out
    }

    pub(crate) fn opencode_roots(&self) -> (PathBuf, PathBuf, Option<PathBuf>) {
        let data = self
            .overlay_first("opencode", "data")
            .or_else(|| {
                self.catalog_env("opencode", "data")
                    .and_then(|value| visible_path(&self.home, &value))
                    .filter(|path| path.is_absolute())
            })
            .or_else(|| {
                let rel = source_catalog::artifact("opencode", "xdg-data")
                    .and_then(|artifact| artifact.path.clone())?;
                self.catalog_env("opencode", "xdg-data")
                    .and_then(|value| visible_path(&self.home, &value))
                    .filter(|path| path.is_absolute())
                    .map(|path| path.join(rel))
            })
            .unwrap_or_else(|| catalog_root(&self.home, "opencode", "data"));
        let legacy = self
            .overlay_first("opencode", "legacy-storage")
            .unwrap_or_else(|| data.join(catalog_suffix("opencode", "legacy-storage")));
        let database = self
            .overlay_first("opencode", "db")
            .or_else(|| {
                self.catalog_env("opencode", "db")
                    .and_then(|value| visible_path(&self.home, &value))
                    .filter(|path| path.is_absolute())
            });
        (data, legacy, database)
    }

    pub(crate) fn kilo_db_root(&self, platform: &str) -> PathBuf {
        if let Some(paths) = self.overlays_for("kilo") {
            return paths[0].clone();
        }
        let default = default_artifacts_on_platform("kilo", platform)
            .into_iter()
            .next()
            .and_then(|artifact| artifact.path.as_deref())
            .map(|path| self.home.join(path))
            .unwrap_or_else(|| panic!("source catalog must define a kilo database path for {platform}"));
        let Some(value) = self
            .catalog_env("kilo", "db")
            .and_then(|value| visible_path(&self.home, &value))
        else {
            return default;
        };
        if value.is_absolute() {
            return value;
        }
        default
            .parent()
            .map(|parent| parent.join(value))
            .unwrap_or(default)
    }

    /// Qoder's roots of one Artifact `kind`. The IDE ships as two products —
    /// QoderCN and the plain-Qoder edition — which may coexist on one machine,
    /// and the CLI keeps its transcripts in its own directories; all of them
    /// belong to the one Qoder Source.
    fn qoder_roots(&self, platform: &str, kind: &str) -> Vec<PathBuf> {
        let mut over = Vec::new();
        for artifact in catalog_artifacts("qoder").iter().filter(|a| a.kind == kind) {
            for path in self.overlay("qoder", &artifact.id).unwrap_or(&[]) {
                push_unique_root(&mut over, path.clone());
            }
        }
        if !over.is_empty() {
            return over;
        }
        default_artifacts_on_platform("qoder", platform)
            .into_iter()
            .filter(|artifact| artifact.kind == kind)
            .filter_map(|artifact| artifact.path.as_deref().map(|path| self.home.join(path)))
            .collect()
    }

    pub(crate) fn zed_database_roots(&self, platform: &str) -> Vec<PathBuf> {
        if let Some(paths) = self.overlays_for("zed") {
            return paths;
        }
        let mut out = Vec::new();
        let override_data_home = if platform == "linux" {
            self.catalog_env("zed", "database-flatpak")
                .and_then(|value| visible_path(&self.home, &value))
                .filter(|path| path.is_absolute())
                .map(|path| (path, "database-flatpak"))
                .or_else(|| {
                    self.catalog_env("zed", "database-xdg")
                        .and_then(|value| visible_path(&self.home, &value))
                        .filter(|path| path.is_absolute())
                        .map(|path| (path, "database-xdg"))
                })
        } else {
            None
        };

        if let Some((data_home, artifact)) = override_data_home {
            if let Some(path) = source_catalog::artifact("zed", artifact)
                .and_then(|artifact| artifact.path.as_deref())
            {
                push_unique_root(&mut out, data_home.join(path));
            }
        } else {
            for artifact in default_artifacts_on_platform("zed", platform) {
                if let Some(path) = artifact.path.as_deref() {
                    push_unique_root(&mut out, self.home.join(path));
                }
            }
        }
        out
    }

    fn hermes_db(&self) -> PathBuf {
        if let Some(path) = self.overlay_first("hermes", "state") {
            return path;
        }
        let home = self
            .catalog_env("hermes", "state")
            .and_then(|value| visible_path(&self.home, &value))
            .unwrap_or_else(|| {
                catalog_root(&self.home, "hermes", "state")
                    .parent()
                    .expect("Hermes state artifact must have a parent directory")
                    .to_path_buf()
            });
        home.join(source_catalog::artifact_filename("hermes", "state"))
    }

    fn gemini_paths(&self) -> (PathBuf, PathBuf) {
        if let (Some(tmp), Some(projects)) = (
            self.overlay("gemini", "tmp"),
            self.overlay("gemini", "projects"),
        ) {
            return (tmp[0].clone(), projects[0].clone());
        }
        if let Some(tmp) = self.overlay("gemini", "tmp") {
            let tmp = &tmp[0];
            return (
                tmp.clone(),
                tmp.parent()
                    .unwrap_or(tmp)
                    .join(source_catalog::artifact_filename("gemini", "projects")),
            );
        }
        let gemini_dir = catalog_artifact_parent("gemini", "tmp");
        let gemini_home = self
            .catalog_env("gemini", "tmp")
            .and_then(|value| visible_path(&self.home, &value))
            .map(|path| path.join(&gemini_dir))
            .unwrap_or_else(|| self.home.join(&gemini_dir));
        (
            gemini_home.join(source_catalog::artifact_filename("gemini", "tmp")),
            gemini_home.join(source_catalog::artifact_filename("gemini", "projects")),
        )
    }

    fn grok_home(&self) -> PathBuf {
        self.catalog_env("grok", "sessions")
            .and_then(|value| visible_path(&self.home, &value))
            .unwrap_or_else(|| self.home.join(catalog_artifact_parent("grok", "sessions")))
    }

    fn grok_sessions(&self) -> PathBuf {
        if let Some(path) = self.overlay_first("grok", "sessions") {
            return path;
        }
        self.grok_home().join(source_catalog::artifact_filename("grok", "sessions"))
    }

    /// The CLI's own unified log, where every credits snapshot it fetches lands.
    /// A separate artifact from the sessions above, discovered and failing
    /// independently of them (ADR-0015).
    fn grok_logs(&self) -> PathBuf {
        if let Some(path) = self.overlay_first("grok", "logs") {
            return path;
        }
        self.grok_home().join(catalog_artifact_tail("grok", "logs"))
    }

    fn copilot_db(&self) -> PathBuf {
        if let Some(path) = self.overlay_first("copilot", "session-store") {
            return path;
        }
        let home = self
            .catalog_env("copilot", "session-store")
            .and_then(|value| visible_path(&self.home, &value))
            .unwrap_or_else(|| self.home.join(catalog_artifact_parent("copilot", "session-store")));
        home.join(source_catalog::artifact_filename("copilot", "session-store"))
    }
    }

/// Cline's CLI data directory, in precedence order: two environment overrides,
/// then the default path. Exactly one of these is scanned — they all name the
/// same tasks directory — which is why `cline_roots` skips them in its walk of
/// the editor Artifacts.
const CLINE_CLI_ROOT_CHAIN: [&str; 3] = ["cli-data", "cli-sandbox", "cli-default-data"];

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
    if !artifact_on_platform(definition, platform) {
        return None;
    }
    definition.path.as_deref().map(|path| home.join(path))
}

fn catalog_artifacts(source: &str) -> &'static [source_catalog::ArtifactDescriptor] {
    source_catalog::source(source)
        .map(|source| source.artifacts.as_slice())
        .unwrap_or(&[])
}

fn artifact_on_platform(artifact: &source_catalog::ArtifactDescriptor, platform: &str) -> bool {
    artifact
        .platforms
        .iter()
        .any(|supported| supported == "all" || supported == platform)
}

fn catalog_suffix(source: &str, id: &str) -> String {
    source_catalog::artifact(source, id)
        .and_then(|artifact| artifact.suffix.clone())
        .unwrap_or_else(|| panic!("source catalog must define {source}.{id} suffix"))
}

fn default_artifacts_on_platform(
    source: &str,
    platform: &str,
) -> Vec<&'static source_catalog::ArtifactDescriptor> {
    catalog_artifacts(source)
        .iter()
        .filter(|artifact| {
            artifact.path.is_some()
                && artifact.environment.is_none()
                && artifact_on_platform(artifact, platform)
        })
        .collect()
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

















fn catalog_artifact_path(source: &str, artifact: &str) -> &'static str {
    source_catalog::artifact(source, artifact)
        .and_then(|artifact| artifact.path.as_deref())
        .unwrap_or_else(|| panic!("source catalog must define {source}.{artifact} path"))
}

/// The catalog path with its home-relative root removed — precisely the part a
/// `$..._HOME` override replaces. `.grok/logs/unified.jsonl` → `logs/unified.jsonl`,
/// so an artifact nested deeper than one level still resolves under an override.
fn catalog_artifact_tail(source: &str, artifact: &str) -> PathBuf {
    Path::new(catalog_artifact_path(source, artifact)).iter().skip(1).collect()
}

fn catalog_artifact_parent(source: &str, artifact: &str) -> PathBuf {
    Path::new(catalog_artifact_path(source, artifact))
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!("source catalog artifact {source}.{artifact} path must have a parent")
        })
}

fn append_session_override(
    sessions: &mut Vec<PathBuf>,
    home: &Path,
    source: &str,
    artifact_id: &str,
    value: Option<&OsStr>,
) {
    let Some(path) = value.and_then(|value| visible_path(home, value)) else {
        return;
    };
    let artifact = source_catalog::artifact(source, artifact_id)
        .unwrap_or_else(|| panic!("source catalog must define {source}.{artifact_id}"));
    sessions.push(match artifact.suffix.as_deref() {
        Some(suffix) => path.join(suffix),
        None => path,
    });
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
            limit_readings: r.limit_readings,
            artifacts_unreadable: r.artifacts_unreadable,
            unreadable_max_mtime: r.unreadable_max_mtime,
            error: r.error,
        },
        Err(_) => SourceStatus {
            source: source.to_string(),
            events_inserted: 0,
            lines_skipped: 0,
            limit_readings: 0,
            artifacts_unreadable: 0,
            unreadable_max_mtime: None,
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
                "claude" => run_one(&source.key, || {
                    scan_claude(conn, &roots.artifact_path("claude", "projects"))
                }),
                "codex" => run_one(&source.key, || scan_codex(conn, &roots.codex_session_roots())),
                "copilot" => run_one(&source.key, || scan_copilot(conn, &roots.copilot_db())),
                "gemini" => run_one(&source.key, || {
                    let (tmp, projects) = roots.gemini_paths();
                    scan_gemini(conn, &tmp, &projects)
                }),
                "hermes" => run_one(&source.key, || scan_hermes(conn, &roots.hermes_db())),
                "grok" => {
                    run_one(&source.key, || {
                        scan_grok(conn, &roots.grok_sessions(), &roots.grok_logs())
                    })
                }
                // The IDE writes under either `antigravity/` or `antigravity-ide/`
                // depending on its `--app_data_dir`, and the CLI under
                // `antigravity-cli/`. All three share one SQLite schema, and all
                // three are scanned — a dir left out is a dir whose exports
                // nothing would ever read.
                "antigravity" => run_one(&source.key, || {
                    let conversations = roots.artifact_path("antigravity", "conversations");
                    let ide = roots.artifact_path("antigravity", "ide-conversations");
                    let cli = roots.artifact_path("antigravity", "cli-conversations");
                    scan_antigravity(conn, &[conversations.as_path(), ide.as_path(), cli.as_path()])
                }),
                "goose" => run_one(&source.key, || {
                    scan_goose(conn, &roots.goose_session_roots(target_platform))
                }),
                "pi" => run_one(&source.key, || scan_pi(conn, &roots.pi_session_roots())),
                "omp" => run_one(&source.key, || scan_omp(conn, &roots.omp_session_roots())),
                "opencode" => run_one(&source.key, || {
                    let (data, legacy, db) = roots.opencode_roots();
                    scan_opencode(conn, &data, &legacy, db.as_deref())
                }),
                "kilo" => run_one(&source.key, || {
                    scan_kilo(conn, &roots.kilo_db_root(target_platform))
                }),
                "zed" => run_one(&source.key, || {
                    scan_zed(conn, &roots.zed_database_roots(target_platform))
                }),
                "cline" => run_one(&source.key, || scan_cline(conn, &roots.cline_roots(target_platform))),
                "workbuddy" => run_one(&source.key, || {
                    scan_workbuddy(conn, &roots.artifact_path("workbuddy", "projects"))
                }),
                "codebuddy" => run_one(&source.key, || {
                    scan_codebuddy(conn, &roots.artifact_path("codebuddy", "projects"))
                }),
                "qoder" => run_one(&source.key, || {
                    scan_qoder(
                        conn,
                        &roots.qoder_roots(target_platform, "file"),
                        &roots.qoder_roots(target_platform, "directory"),
                    )
                }),
                _ => SourceStatus {
                    source: source.key.clone(),
                    events_inserted: 0,
                    lines_skipped: 0,
                    limit_readings: 0,
                    artifacts_unreadable: 0,
                    unreadable_max_mtime: None,
                    error: Some("unsupported source catalog entry".to_string()),
                },
            },
        };
        sources.push(merge_limit_exports(conn, roots, source, status));
    }

    // Ledger hygiene only: drops scanned_files rows for vanished paths, and
    // the Codex Context rows keyed to them — both rebuildable scan state.
    // Never deletes events (see prune_missing_files contract). Best-effort.
    let _ = prune_missing_files(conn);

    // Persist every Source's Unreadable-Artifact state (ADR-0017) so the ≥
    // floor marker is honest from launch, before a run's first scan.
    // Best-effort, like the pruning above.
    let _ = crate::db::record_unreadable(conn, &sources);

    let scanned_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    ScanStatus {
        sources,
        scanned_at,
        ingest_rev: 0,
    }
}

/// A `live` Source additionally reads the Limit Readings its Companion exported —
/// ordinary Artifacts, read like any other file (ADR-0019). Catalog-driven, so
/// no Source is named here; the Readings carry no tokens, so they change no
/// count on the status, only its error line.
fn merge_limit_exports(
    conn: &mut Connection,
    roots: &SourceRoots,
    source: &source_catalog::SourceDefinition,
    mut status: SourceStatus,
) -> SourceStatus {
    if source.capabilities.limits.as_deref() != Some("live") || roots.limit_exports.as_os_str().is_empty() {
        return status;
    }
    match limits_artifact::ingest(conn, &roots.limit_exports, &source.key) {
        Ok(written) => status.limit_readings += written,
        Err(error) => {
            status.error = Some(match status.error {
                Some(previous) => format!("{previous}; {error}"),
                None => error,
            });
        }
    }
    status
}

fn unavailable_source_status(source: &str, error: String) -> SourceStatus {
    SourceStatus {
        source: source.to_string(),
        events_inserted: 0,
        lines_skipped: 0,
        limit_readings: 0,
        artifacts_unreadable: 0,
        unreadable_max_mtime: None,
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

    #[test]
    fn an_ordinary_scan_reads_a_companions_limits_export() {
        // ADR-0019's route end to end: the Companion leaves an Artifact, and the
        // scan reads it like any other file — no Source is named in the scanner,
        // the catalog's `limits: "live"` is what selects the pass.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let exports = base.join("limits");
        fs::create_dir_all(&exports).unwrap();
        fs::write(
            exports.join("claude.tokenledger-limits.json"),
            r#"{"schema":1,"source":"claude","fetched_at":1786492800,"plan":"Team 5x",
                "windows":[{"key":"five_hour","window_minutes":300,"used_pct":18.0,
                            "resets_at":1786503900}]}"#,
        )
        .unwrap();

        // An empty home, so no Source on this machine contributes anything and
        // the only Readings in play are the export's.
        let roots = SourceRoots::at(base.join("home"))
            .with_limit_exports(exports);
        let mut conn = open_db(&base.join("ledger.db")).unwrap();
        let status = run_scan(&mut conn, &roots);
        assert!(find(&status, "claude").error.is_none());
        // The export's Reading is a relevant change, and it signals as one
        // (#187); a rescan of the unchanged Artifact signals nothing.
        assert_eq!(find(&status, "claude").limit_readings, 1);
        let unchanged = run_scan(&mut conn, &roots);
        assert_eq!(find(&unchanged, "claude").limit_readings, 0);

        let cards = queries::limits(&conn, 1_900_000_000, std::path::Path::new("")).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].source, "claude");
        assert_eq!(cards[0].plan.as_deref(), Some("Team 5x"));
        assert_eq!(cards[0].windows[0].window_key, "five_hour");
        assert_eq!(cards[0].windows[0].used_pct, 18.0);

        // Codex is a `live` Source too, so its corrupt export surfaces on its own
        // status — while a Source with no limits capability has no export pass at
        // all, and a stray file named after it can never be blamed on one.
        for key in ["codex", "gemini"] {
            fs::write(
                base.join("limits").join(format!("{key}.tokenledger-limits.json")),
                "not json",
            )
            .unwrap();
        }
        let again = run_scan(&mut conn, &roots);
        assert!(find(&again, "codex").error.as_deref().is_some_and(|e| e.contains("unreadable")));
        assert!(find(&again, "gemini").error.is_none());
    }
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
                "copilot",
                "gemini",
                "hermes",
                "grok",
                "antigravity",
                "goose",
                "opencode",
                "kilo",
                "zed",
                "cline",
                "pi",
                "omp",
                "workbuddy",
                "codebuddy",
                "qoder"
            ],
        );
        assert!(catalog.sources.iter().all(|source| {
            !source.label.is_empty()
                && !source.aliases.is_empty()
                && source.color.starts_with('#')
                && !source.icon.is_empty()
                && if source.key == "zed" {
                    source.platforms == ["linux", "macos", "windows"]
                } else {
                    source.platforms == ["all"]
                }
                && source.prerequisite.is_none()
                && source.capabilities.model
                && (source.capabilities.project || source.key == "copilot")
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
            ["claude", "codex", "grok", "pi", "omp", "qoder"],
        );
        assert_eq!(
            catalog.sources.iter().flat_map(|source| {
                source.artifacts.iter().filter_map(move |artifact| artifact.path.as_deref()
                    .map(|path| (source.key.as_str(), artifact.id.as_str(), path)))
            }).collect::<Vec<_>>(),
            [
                ("claude", "projects", ".claude/projects"),
                ("codex", "sessions", ".codex/sessions"),
                ("copilot", "session-store", ".copilot/session-store.db"),
                ("gemini", "tmp", ".gemini/tmp"),
                ("gemini", "projects", ".gemini/projects.json"),
                ("hermes", "state", ".hermes/state.db"),
                ("grok", "sessions", ".grok/sessions"),
                ("grok", "logs", ".grok/logs/unified.jsonl"),
                ("antigravity", "conversations", ".gemini/antigravity/conversations"),
                ("antigravity", "ide-conversations", ".gemini/antigravity-ide/conversations"),
                ("antigravity", "cli-conversations", ".gemini/antigravity-cli/conversations"),
                // Catalog order is scan order: the current per-platform directory
                // precedes the pre-1.10 `.local/share` path it replaced.
                ("goose", "sessions-macos", "Library/Application Support/Block/goose/data/sessions"),
                ("goose", "sessions-windows", "AppData/Roaming/Block/goose/data/sessions"),
                ("goose", "sessions", ".local/share/goose/sessions"),
                ("opencode", "data", ".local/share/opencode"),
                ("opencode", "db", ".local/share/opencode/opencode.db"),
                ("opencode", "channel-db", ".local/share/opencode/opencode-<channel>.db"),
                ("opencode", "legacy-storage", ".local/share/opencode/storage"),
                ("opencode", "xdg-data", "opencode"),
                ("kilo", "db-macos", "Library/Application Support/kilo/kilo.db"),
                ("kilo", "db-linux", ".local/share/kilo/kilo.db"),
                ("kilo", "db-windows", "AppData/Local/kilo/kilo.db"),
                ("zed", "database-macos", "Library/Application Support/Zed/threads/threads.db"),
                ("zed", "database-linux", ".local/share/zed/threads/threads.db"),
                ("zed", "database-windows", "AppData/Local/Zed/threads/threads.db"),
                ("zed", "database-xdg", "zed/threads/threads.db"),
                ("zed", "database-flatpak", "zed/threads/threads.db"),
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
                ("omp", "sessions", ".omp/agent/sessions"),
                ("workbuddy", "projects", ".workbuddy/projects"),
                ("codebuddy", "projects", ".codebuddy/projects"),
                ("qoder", "db-cn-macos", "Library/Application Support/QoderCN/SharedClientCache/cache/db/local.db"),
                ("qoder", "db-cn-linux", ".config/QoderCN/SharedClientCache/cache/db/local.db"),
                ("qoder", "db-cn-windows", "AppData/Roaming/QoderCN/SharedClientCache/cache/db/local.db"),
                ("qoder", "db-qoder-macos", "Library/Application Support/Qoder/SharedClientCache/cache/db/local.db"),
                ("qoder", "db-qoder-linux", ".config/Qoder/SharedClientCache/cache/db/local.db"),
                ("qoder", "db-qoder-windows", "AppData/Roaming/Qoder/SharedClientCache/cache/db/local.db"),
                ("qoder", "qoder-projects", ".qoder/projects"),
                ("qoder", "qoder-cli-projects", ".qoder-cli/projects"),
                ("qoder", "qoder-cn-projects", ".qoder-cn/projects"),
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

        let codex = crate::source_catalog::source("codex").unwrap();
        assert_eq!(
            codex
                .artifacts
                .iter()
                .map(|artifact| artifact.id.as_str())
                .collect::<Vec<_>>(),
            ["sessions", "home"]
        );
        assert_eq!(
            codex.artifacts[1].environment.as_deref(),
            Some("CODEX_HOME")
        );
        assert_eq!(codex.artifacts[1].suffix.as_deref(), Some("sessions"));

        let hermes = crate::source_catalog::source("hermes").unwrap();
        assert_eq!(
            hermes.artifacts[0].path.as_deref(),
            Some(".hermes/state.db")
        );
        assert_eq!(
            hermes.artifacts[0].environment.as_deref(),
            Some("HERMES_HOME")
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
            ["sessions-macos", "sessions-windows", "sessions", "root"]
        );
        assert_eq!(
            goose.artifacts[2].path.as_deref(),
            Some(".local/share/goose/sessions"),
            "the pre-1.10 path is scanned last, after the current per-platform one"
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

        let kilo = crate::source_catalog::source("kilo").unwrap();
        assert_eq!(kilo.source, "Kilo CLI");
        assert_eq!(kilo.aliases, ["Kilo Code", "Kilo Code CLI"]);
        assert_eq!(
            kilo.artifacts
                .iter()
                .map(|artifact| artifact.id.as_str())
                .collect::<Vec<_>>(),
            ["db", "db-macos", "db-linux", "db-windows"]
        );
        assert_eq!(kilo.artifacts[0].environment.as_deref(), Some("KILO_DB"));
        assert_eq!(
            kilo.artifacts[1].path.as_deref(),
            Some("Library/Application Support/kilo/kilo.db")
        );

        let zed = crate::source_catalog::source("zed").unwrap();
        assert_eq!(zed.source, "Zed");
        assert_eq!(zed.aliases, ["Zed Editor"]);
        assert_eq!(zed.platforms, ["linux", "macos", "windows"]);
        assert_eq!(zed.artifacts[0].id, "database-macos");
        assert_eq!(zed.artifacts[3].environment.as_deref(), Some("XDG_DATA_HOME"));
        assert_eq!(zed.artifacts[4].environment.as_deref(), Some("FLATPAK_XDG_DATA_HOME"));

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
        let roots = SourceRoots::at(home.path())
            .with_env("PI_CODING_AGENT_SESSION_DIR", OsStr::new("~/custom-sessions"))
            .with_env("PI_CODING_AGENT_DIR", OsStr::new("~/custom-agent"));
        assert_eq!(
            roots.pi_session_roots(),
            vec![
                home.path().join(".pi/agent/sessions"),
                home.path().join("custom-sessions"),
                home.path().join("custom-agent/sessions"),
            ],
        );
    }

    #[test]
    fn codex_roots_include_default_then_visible_home_override() {
        use std::ffi::OsStr;

        let home = tempfile::tempdir().unwrap();
        assert_eq!(
            SourceRoots::at(home.path()).with_env("CODEX_HOME", OsStr::new("~/relocated-codex")).codex_session_roots(),
            vec![
                home.path().join(".codex/sessions"),
                home.path().join("relocated-codex/sessions"),
            ]
        );
        assert_eq!(
            SourceRoots::at(home.path()).with_env("CODEX_HOME", OsStr::new("  ")).codex_session_roots(),
            vec![home.path().join(".codex/sessions")]
        );
        assert_eq!(
            SourceRoots::at(home.path()).with_env("CODEX_HOME", home.path().join(".codex").as_os_str()).codex_session_roots(),
            vec![home.path().join(".codex/sessions")]
        );
    }

    #[test]
    fn opencode_roots_use_visible_data_database_and_xdg_overrides() {
        use std::ffi::OsStr;

        let home = tempfile::tempdir().unwrap();
        let configured_db = home.path().join("configured/opencode.db");
        let configured = SourceRoots::at(home.path())
                .with_env("OPENCODE_DATA_DIR", OsStr::new("~/configured-opencode"))
                .with_env("OPENCODE_DB", OsStr::new(configured_db.to_str().unwrap())).opencode_roots();
        assert_eq!(configured.0, home.path().join("configured-opencode"));
        assert_eq!(
            configured.1,
            home.path().join("configured-opencode/storage")
        );
        assert_eq!(configured.2, Some(configured_db));

        let xdg = SourceRoots::at(home.path()).with_env("XDG_DATA_HOME", OsStr::new("~/configured-data")).opencode_roots();
        assert_eq!(xdg.0, home.path().join("configured-data/opencode"));
        assert_eq!(xdg.1, home.path().join("configured-data/opencode/storage"));
        assert_eq!(xdg.2, None);

        let blank = SourceRoots::at(home.path())
                .with_env("OPENCODE_DATA_DIR", OsStr::new("  "))
                .with_env("OPENCODE_DB", OsStr::new("  "))
                .with_env("XDG_DATA_HOME", OsStr::new("  ")).opencode_roots();
        assert_eq!(blank.0, home.path().join(".local/share/opencode"));
        assert_eq!(blank.2, None);
    }

    #[test]
    fn kilo_root_uses_supported_platform_paths_and_database_override() {
        use std::ffi::OsStr;

        let home = tempfile::tempdir().unwrap();
        let r = SourceRoots::at(home.path());
        assert_eq!(
            r.kilo_db_root("macos"),
            home.path().join("Library/Application Support/kilo/kilo.db")
        );
        assert_eq!(
            r.kilo_db_root("linux"),
            home.path().join(".local/share/kilo/kilo.db")
        );
        assert_eq!(
            r.kilo_db_root("windows"),
            home.path().join("AppData/Local/kilo/kilo.db")
        );

        let configured = home.path().join("configured/kilo.db");
        assert_eq!(
            SourceRoots::at(home.path()).with_env("KILO_DB", OsStr::new(configured.to_str().unwrap())).kilo_db_root("linux",),
            configured
        );
        assert_eq!(
            SourceRoots::at(home.path()).with_env("KILO_DB", OsStr::new("custom.db")).kilo_db_root("linux"),
            home.path().join(".local/share/kilo/custom.db")
        );
    }

    #[test]
    fn zed_roots_use_supported_platform_paths_and_xdg_overrides() {
        use std::ffi::OsStr;

        let home = tempfile::tempdir().unwrap();
        let r = SourceRoots::at(home.path());
        assert_eq!(
            r.zed_database_roots("macos"),
            vec![home
                .path()
                .join("Library/Application Support/Zed/threads/threads.db")]
        );
        assert_eq!(
            r.zed_database_roots("linux"),
            vec![home.path().join(".local/share/zed/threads/threads.db")]
        );
        assert_eq!(
            r.zed_database_roots("windows"),
            vec![home.path().join("AppData/Local/Zed/threads/threads.db")]
        );

        let xdg = home.path().join("configured-data");
        assert_eq!(
            SourceRoots::at(home.path()).with_env("XDG_DATA_HOME", OsStr::new(xdg.to_str().unwrap())).zed_database_roots("linux",),
            vec![xdg.join("zed/threads/threads.db")]
        );

        let flatpak = home.path().join("flatpak-data");
        assert_eq!(
            SourceRoots::at(home.path())
                    .with_env("FLATPAK_XDG_DATA_HOME", OsStr::new(flatpak.to_str().unwrap()))
                    .with_env("XDG_DATA_HOME", OsStr::new(xdg.to_str().unwrap())).zed_database_roots("linux",),
            vec![flatpak.join("zed/threads/threads.db")]
        );
    }

    #[test]
    fn hermes_home_override_is_used_and_blank_value_falls_back() {
        use std::ffi::OsStr;

        let home = tempfile::tempdir().unwrap();
        let override_home = home.path().join("configured-hermes");
        let overridden = SourceRoots::at(home.path())
            .with_env("HERMES_HOME", OsStr::new(override_home.to_str().unwrap()));
        assert_eq!(overridden.hermes_db(), override_home.join("state.db"));

        let blank = SourceRoots::at(home.path()).with_env("HERMES_HOME", OsStr::new("  "));
        assert_eq!(blank.hermes_db(), home.path().join(".hermes/state.db"));

        let absent = SourceRoots::at(home.path());
        assert_eq!(absent.hermes_db(), home.path().join(".hermes/state.db"));
    }

    #[test]
    fn gemini_cli_home_override_is_nested_and_blank_value_falls_back() {
        use std::ffi::OsStr;

        let home = tempfile::tempdir().unwrap();
        let overridden = SourceRoots::at(home.path())
            .with_env("GEMINI_CLI_HOME", OsStr::new("~/configured-gemini"));
        let (tmp, projects) = overridden.gemini_paths();
        assert_eq!(tmp, home.path().join("configured-gemini/.gemini/tmp"));
        assert_eq!(projects, home.path().join("configured-gemini/.gemini/projects.json"));

        let blank = SourceRoots::at(home.path()).with_env("GEMINI_CLI_HOME", OsStr::new("  "));
        let (tmp, projects) = blank.gemini_paths();
        assert_eq!(tmp, home.path().join(".gemini/tmp"));
        assert_eq!(projects, home.path().join(".gemini/projects.json"));

        let absent = SourceRoots::at(home.path());
        let (tmp, projects) = absent.gemini_paths();
        assert_eq!(tmp, home.path().join(".gemini/tmp"));
        assert_eq!(projects, home.path().join(".gemini/projects.json"));
    }

    #[test]
    fn grok_home_override_is_used_and_blank_value_falls_back() {
        use std::ffi::OsStr;

        let home = tempfile::tempdir().unwrap();
        let overridden = SourceRoots::at(home.path())
            .with_env("GROK_HOME", OsStr::new("~/configured-grok"));
        assert_eq!(
            overridden.grok_sessions(),
            home.path().join("configured-grok/sessions")
        );
        // The unified log sits a level deeper, so the override has to replace the
        // home-relative root rather than just the last component.
        assert_eq!(
            overridden.grok_logs(),
            home.path().join("configured-grok/logs/unified.jsonl")
        );

        let blank = SourceRoots::at(home.path()).with_env("GROK_HOME", OsStr::new("  "));
        assert_eq!(blank.grok_sessions(), home.path().join(".grok/sessions"));

        let absent = SourceRoots::at(home.path());
        assert_eq!(absent.grok_sessions(), home.path().join(".grok/sessions"));
        assert_eq!(absent.grok_logs(), home.path().join(".grok/logs/unified.jsonl"));
    }

    #[test]
    fn goose_roots_cover_platform_defaults_legacy_storage_and_absolute_override() {
        use std::ffi::OsStr;

        let home = tempfile::tempdir().unwrap();
        let r = SourceRoots::at(home.path());
        assert_eq!(
            r.goose_session_roots("linux"),
            vec![home.path().join(".local/share/goose/sessions")]
        );
        assert_eq!(
            r.goose_session_roots("macos"),
            vec![
                home.path()
                    .join("Library/Application Support/Block/goose/data/sessions"),
                home.path().join(".local/share/goose/sessions"),
            ]
        );
        assert_eq!(
            SourceRoots::at(home.path()).with_env("GOOSE_PATH_ROOT", OsStr::new("~/configured-goose")).goose_session_roots("macos",),
            vec![home.path().join("configured-goose/data/sessions")]
        );
        assert_eq!(
            SourceRoots::at(home.path()).with_env("GOOSE_PATH_ROOT", OsStr::new("relative-goose")).goose_session_roots("linux",),
            vec![home.path().join(".local/share/goose/sessions")]
        );
    }

    #[test]
    fn cline_cli_root_precedence_ignores_blank_values_and_deduplicates_equivalents() {
        use std::ffi::OsStr;

        let home = tempfile::tempdir().unwrap();
        let explicit = home.path().join("configured-cline");
        let sandbox = home.path().join("sandbox-cline");
        let overridden = SourceRoots::at(home.path())
                .with_env("CLINE_DATA_DIR", explicit.as_os_str())
                .with_env("CLINE_SANDBOX_DATA_DIR", sandbox.as_os_str()).cline_roots("linux",);
        assert!(overridden.contains(&explicit));
        assert!(!overridden.contains(&sandbox));

        let blank_data = SourceRoots::at(home.path())
                .with_env("CLINE_DATA_DIR", OsStr::new(" \t"))
                .with_env("CLINE_SANDBOX_DATA_DIR", OsStr::new("~/sandbox-cline")).cline_roots("linux",);
        assert!(blank_data.contains(&sandbox));

        let defaults = SourceRoots::at(home.path()).cline_roots("linux");
        assert!(defaults.contains(&home.path().join(".cline/data")));

        let equivalent = SourceRoots::at(home.path()).with_env("CLINE_DATA_DIR", OsStr::new("~/.cline/../.cline/data")).cline_roots("linux",);
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
        let os = std::env::consts::OS;
        assert!(r.artifact_path("claude", "projects").ends_with(".claude/projects"));
        assert!(r.codex_session_roots()[0].ends_with(".codex/sessions"));
        let copilot_home = std::env::var_os("COPILOT_HOME")
            .and_then(|value| visible_path(&r.home, &value))
            .unwrap_or_else(|| r.home.join(".copilot"));
        assert_eq!(r.copilot_db(), copilot_home.join("session-store.db"));
        let (gemini_tmp, gemini_projects) = r.gemini_paths();
        assert!(gemini_tmp.ends_with(".gemini/tmp"));
        assert!(gemini_projects.ends_with(".gemini/projects.json"));
        assert!(r.hermes_db().ends_with(".hermes/state.db"));
        let grok_home = std::env::var_os("GROK_HOME")
            .and_then(|value| visible_path(&r.home, &value))
            .unwrap_or_else(|| r.home.join(".grok"));
        assert_eq!(r.grok_sessions(), grok_home.join("sessions"));
        assert!(r.artifact_path("antigravity", "conversations")
            .ends_with(".gemini/antigravity/conversations"));
        assert!(r.artifact_path("antigravity", "cli-conversations")
            .ends_with(".gemini/antigravity-cli/conversations"));
        assert!(r.pi_session_roots()[0].ends_with(".pi/agent/sessions"));
        let zed_suffix = match os {
            "macos" => "Library/Application Support/Zed/threads/threads.db",
            "windows" => "AppData/Local/Zed/threads/threads.db",
            _ => ".local/share/zed/threads/threads.db",
        };
        assert!(r.zed_database_roots(os)[0].ends_with(zed_suffix));
        assert!(!r.goose_session_roots(os).is_empty());
        let cline = r.cline_roots(os);
        assert!(cline.iter().any(|path| path.ends_with(".cline/data")));
        // Every platform carries at least one editor task root; the editor
        // vendor directory is `.vscode-server/...` on Unix and
        // `Code/User/globalStorage/...` on Windows, so assert on the common
        // storage suffix instead of a platform-specific prefix.
        assert!(cline
            .iter()
            .any(|path| path.ends_with("globalStorage/saoudrizwan.claude-dev/tasks")));
    }

    #[test]
    fn run_scan_backfills_gemini_override_queries_cost_and_retains_disappeared_usage() {
        std::env::set_var("TZ", "UTC");
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let gemini_home = base.join("configured-gemini");
        let roots = SourceRoots::at(base).with_env("GEMINI_CLI_HOME", gemini_home.as_os_str());
        let (gemini_tmp, gemini_projects) = roots.gemini_paths();
        let session_path = gemini_tmp.join("alpha/chats/session-override.json");
        fs::create_dir_all(session_path.parent().unwrap()).unwrap();
        fs::write(
            &gemini_projects,
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
        let roots = SourceRoots::at(base).with_env("GROK_HOME", grok_home.as_os_str());
        let updates_path = roots.grok_sessions()
            .join("%2FUsers%2Fdev%2Fprojects%2Fgrok-demo/sess-override/updates.jsonl");
        fs::create_dir_all(updates_path.parent().unwrap()).unwrap();
        fs::write(
            &updates_path,
            concat!(
                r#"{"timestamp":1780287300,"method":"session/update","params":{"sessionId":"sess-override","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"GROK_PRIVATE_PROMPT_MARKER"}}}}"#,
                "\n",
                r#"{"timestamp":1780287301,"method":"session/update","params":{"sessionId":"sess-override","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"GROK_PRIVATE_RESPONSE_MARKER"}},"_meta":{"totalTokens":100}}}"#,
                "\n",
                // The Turn's rollup: a 100-token prompt of which 60 came from
                // cache and 10 were written to it, so every bucket the Override
                // prices carries tokens.
                r#"{"timestamp":1780287302,"method":"_x.ai/session/update","params":{"sessionId":"sess-override","update":{"sessionUpdate":"turn_completed","prompt_id":"p-1","stop_reason":"end_turn","usage":{"inputTokens":100,"outputTokens":20,"totalTokens":120,"cachedReadTokens":60,"cacheCreationTokens":10,"reasoningTokens":5,"modelCalls":3}},"_meta":{"eventId":"e"}}}"#,
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
        assert_eq!(summary.input_tokens, 30); // 100 prompt − 60 read − 10 written
        assert_eq!(summary.output_tokens, 20);
        assert_eq!(summary.total_tokens, 120);
        assert_eq!(summary.requests, 3); // the rollup's modelCalls, not one per Turn
        // Every bucket priced by its own Override rate: 30·1 + 20·2 + 60·3 + 10·4.
        assert_eq!(summary.cost, Some(290.0));

        let source_rows = queries::breakdown(&conn, "tool", &Filters::default()).unwrap();
        let grok_row = source_rows
            .iter()
            .find(|row| row.key.as_deref() == Some("grok"))
            .unwrap();
        assert_eq!(grok_row.total_tokens, 120);
        assert_eq!(grok_row.requests, 3);
        assert_eq!(grok_row.cost, Some(290.0));

        let series = queries::series(&conn, &Filters::default(), "day").unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].source, "grok");
        assert_eq!(series[0].total_tokens, 120);
        assert_eq!(series[0].requests, 3);
        assert_eq!(series[0].cost, 290.0);

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
        let roots = SourceRoots::at(tmp.path())
            .with_artifact("claude", "projects", root)
            .with_limit_exports(tmp.path().join("limits"));
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

        let roots = SourceRoots::at(&base)
            .with_artifact("pi", "sessions", pi_root)
            .with_limit_exports(base.join("limits"));

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
        assert_eq!(status.sources.len(), 17);
        assert_eq!(status.sources.last().unwrap().source, "qoder");
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

        let roots = SourceRoots::at(base).with_env("HERMES_HOME", hermes_root.as_os_str());
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

        let kilo_db = base.join("kilo.db");
        rusqlite::Connection::open(&kilo_db)
            .unwrap()
            .execute_batch("CREATE TABLE unrelated (id TEXT PRIMARY KEY);")
            .unwrap();
        let zed_db = base.join("zed.db");
        rusqlite::Connection::open(&zed_db)
            .unwrap()
            .execute_batch("CREATE TABLE unrelated (id TEXT PRIMARY KEY);")
            .unwrap();

        // Planted fixtures only; other Sources resolve catalog paths under this
        // empty home and stay quiet.
        let roots = SourceRoots::at(&base)
            .with_artifact("claude", "projects", claude_root)
            .with_artifact("gemini", "tmp", gemini_root)
            .with_artifact("gemini", "projects", gemini_projects)
            .with_artifact("pi", "sessions", pi_root)
            .with_artifact("kilo", "db", kilo_db)
            .with_source_path("zed", zed_db)
            .with_limit_exports(base.join("limits"));

        let mut conn = open_db(&base.join("ledger.db")).unwrap();
        let status = run_scan(&mut conn, &roots);

        assert_eq!(status.sources.len(), 17);
        assert_eq!(status.sources.last().unwrap().source, "qoder");
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
        let kilo = find(&status, "kilo");
        assert_eq!(kilo.events_inserted, 0);
        assert!(kilo
            .error
            .as_deref()
            .is_some_and(|error| error.contains("kilo") && error.contains("unsupported")));
        let zed = find(&status, "zed");
        assert_eq!(zed.events_inserted, 0);
        assert!(zed
            .error
            .as_deref()
            .is_some_and(|error| error.contains("zed") && error.contains("unsupported")));

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

        let roots = SourceRoots::at(base)
            .with_artifact("opencode", "data", opencode_root.clone())
            .with_artifact("opencode", "legacy-storage", legacy)
            .with_limit_exports(base.join("limits"));
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

    #[test]
    fn run_scan_ingests_kilo_cli_sessions_at_session_granularity() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let database_path = base.join("kilo/kilo.db");
        fs::create_dir_all(database_path.parent().unwrap()).unwrap();
        let source = rusqlite::Connection::open(&database_path).unwrap();
        source
            .execute_batch(
                "CREATE TABLE session (
                    id TEXT PRIMARY KEY,
                    project_id TEXT,
                    workspace_id TEXT,
                    parent_id TEXT,
                    slug TEXT,
                    directory TEXT,
                    path TEXT,
                    title TEXT,
                    version TEXT,
                    cost REAL,
                    time_created INTEGER,
                    time_updated INTEGER,
                    model TEXT,
                    tokens_input INTEGER,
                    tokens_output INTEGER,
                    tokens_reasoning INTEGER,
                    tokens_cache_read INTEGER,
                    tokens_cache_write INTEGER
                );
                CREATE TABLE message (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    time_created INTEGER NOT NULL,
                    data TEXT NOT NULL
                );",
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO session (
                    id, directory, time_created, time_updated, model,
                    tokens_input, tokens_output, tokens_reasoning,
                    tokens_cache_read, tokens_cache_write
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    "kilo-s1",
                    "/Users/dev/projects/kilo",
                    1_780_000_000_000i64,
                    1_780_000_001_000i64,
                    r#"{"id":"kilo-model"}"#,
                    30,
                    8,
                    2,
                    10,
                    1,
                ],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO session (
                    id, directory, time_created, time_updated, model,
                    tokens_input, tokens_output, tokens_reasoning,
                    tokens_cache_read, tokens_cache_write
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    "kilo-s2",
                    "relative/project",
                    1_780_000_002_000i64,
                    1_780_000_003_000i64,
                    Option::<String>::None,
                    2,
                    1,
                    0,
                    0,
                    0,
                ],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO session (
                    id, directory, time_created, time_updated, model,
                    tokens_input, tokens_output, tokens_reasoning,
                    tokens_cache_read, tokens_cache_write
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    "legacy-zero",
                    "/Users/dev/legacy",
                    1_780_000_004_000i64,
                    1_780_000_005_000i64,
                    Option::<String>::None,
                    0,
                    0,
                    0,
                    0,
                    0,
                ],
            )
            .unwrap();
        drop(source);

        let roots = SourceRoots::at(base)
            .with_artifact("kilo", "db", database_path.clone())
            .with_limit_exports(base.join("limits"));
        let mut ledger = open_db(&base.join("ledger.db")).unwrap();
        let first = run_scan(&mut ledger, &roots);
        let kilo = find(&first, "kilo");
        assert_eq!(kilo.events_inserted, 2);
        assert!(kilo.error.is_none(), "unexpected error: {:?}", kilo.error);
        assert_eq!(
            ledger
                .query_row(
                    "SELECT timestamp, model, project, input_tokens, output_tokens,
                            cache_read_tokens, cache_write_5m_tokens, api_calls
                     FROM events WHERE session_id = 'kilo-s1'",
                    [],
                    |row| Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                    )),
                )
                .unwrap(),
            (
                1_780_000_001,
                Some("kilo-model".to_string()),
                Some("/Users/dev/projects/kilo".to_string()),
                30,
                10,
                10,
                1,
                1,
            )
        );
        assert_eq!(
            ledger
                .query_row(
                    "SELECT model, project FROM events WHERE session_id = 'kilo-s2'",
                    [],
                    |row| Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?
                    )),
                )
                .unwrap(),
            (None, None)
        );

        let second = run_scan(&mut ledger, &roots);
        assert_eq!(find(&second, "kilo").events_inserted, 0);
        assert_eq!(
            ledger
                .query_row(
                    "SELECT COUNT(*) FROM events WHERE source = 'kilo'",
                    [],
                    |row| { row.get::<_, i64>(0) }
                )
                .unwrap(),
            2
        );
    }
}
