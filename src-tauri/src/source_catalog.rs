// This shared catalog carries frontend-owned display and capability facts in
// addition to the roots consumed by Rust discovery.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub sources: Vec<SourceDefinition>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SourceDefinition {
    pub key: String,
    pub label: String,
    pub source: String,
    pub aliases: Vec<String>,
    pub color: String,
    pub icon: String,
    pub artifacts: Vec<ArtifactDescriptor>,
    pub capabilities: Capabilities,
    pub platforms: Vec<String>,
    pub prerequisite: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ArtifactDescriptor {
    pub id: String,
    pub kind: String,
    pub path: Option<String>,
    pub environment: Option<String>,
    pub suffix: Option<String>,
    pub platforms: Vec<String>,
    pub prerequisite: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub model: bool,
    pub project: bool,
    pub session: bool,
    pub token_categories: bool,
    pub context: bool,
    /// How this Source's Limits are acquired: `"logs"` (captured passively from
    /// an Artifact the scan already walks) or `"live"` (fetched by a Companion,
    /// ADR-0019, and so gated behind the opt-in). Absent means the Source has no
    /// vendor window to show and gets no card at all. Vendor URLs stay in the
    /// Companion — never in this data file.
    #[serde(default)]
    pub limits: Option<String>,
}

pub fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../src/source-catalog.json"
        )))
        .expect("source catalog must be valid JSON")
    })
}

pub fn source(key: &str) -> Option<&'static SourceDefinition> {
    catalog().sources.iter().find(|source| source.key == key)
}

pub fn artifact(source_key: &str, id: &str) -> Option<&'static ArtifactDescriptor> {
    source(source_key).and_then(|source| source.artifacts.iter().find(|artifact| artifact.id == id))
}

pub fn artifact_filename(source_key: &str, id: &str) -> PathBuf {
    let path = artifact(source_key, id)
        .and_then(|artifact| artifact.path.as_deref())
        .unwrap_or_else(|| panic!("source catalog must define {source_key}.{id} path"));
    Path::new(path)
        .file_name()
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("source catalog artifact {source_key}.{id} path must name a file"))
}

/// Returns an explanation when a Source cannot run on the target platform or
/// needs an external prerequisite. The caller converts this into an isolated
/// scan status; catalog facts never select a different scanner.
pub fn availability(source: &SourceDefinition, target_platform: &str) -> Result<(), String> {
    if !source
        .platforms
        .iter()
        .any(|platform| platform == "all" || platform == target_platform)
    {
        return Err(format!(
            "unavailable on {target_platform}: supported platforms are {}",
            source.platforms.join(", ")
        ));
    }

    if let Some(prerequisite) = &source.prerequisite {
        return Err(format!(
            "unavailable: external prerequisite required: {prerequisite}"
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_requires_a_matching_platform_and_no_prerequisite() {
        let mut source = source("claude").unwrap().clone();
        assert_eq!(availability(&source, "test-platform"), Ok(()));

        source.platforms = vec!["other-platform".to_string()];
        assert_eq!(
            availability(&source, "test-platform"),
            Err("unavailable on test-platform: supported platforms are other-platform".to_string())
        );

        source.platforms = vec!["all".to_string()];
        source.prerequisite = Some("source service".to_string());
        assert_eq!(
            availability(&source, "test-platform"),
            Err("unavailable: external prerequisite required: source service".to_string())
        );
    }
}
