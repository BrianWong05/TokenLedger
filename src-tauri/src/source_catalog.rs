// This shared catalog carries frontend-owned display and capability facts in
// addition to the roots consumed by Rust discovery.
#![allow(dead_code)]

use std::sync::OnceLock;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub sources: Vec<SourceDefinition>,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
pub struct ArtifactDescriptor {
    pub id: String,
    pub kind: String,
    pub path: Option<String>,
    pub environment: Option<String>,
    pub suffix: Option<String>,
    pub platforms: Vec<String>,
    pub prerequisite: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub model: bool,
    pub project: bool,
    pub session: bool,
    pub token_categories: bool,
    pub context: bool,
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
