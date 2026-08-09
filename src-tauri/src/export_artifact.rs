// The Export Artifact format (ADR-0018) — the one contract between the export
// companion, which writes these files, and the Antigravity adapter, which reads
// them. Both sides use these types, so a field can never be spelled one way by
// the writer and another by the reader.
//
// One Artifact per decrypted Session, named `<session>.tokenledger.json` and
// written beside the `.pb` it stands in for.

use std::path::Path;

use serde::{Deserialize, Serialize};

pub const SUFFIX: &str = ".tokenledger.json";

/// Bump when the shape changes. An Artifact declaring a schema the reader does
/// not know is a malformed instance of a supported shape (ADR-0015): it warns,
/// and — crucially — does not stand in for its `.pb`, so the "≥" marker stays.
pub const SCHEMA: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationExport {
    pub schema: u32,
    pub conversation_id: String,
    /// Antigravity records a placeholder enum per generation, so the readable
    /// model name is a Session-level fact when it is recorded at all.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub generations: Vec<GenerationExport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationExport {
    #[serde(default)]
    pub response_id: Option<String>,
    pub ts: i64,
    #[serde(default)]
    pub model_enum: Option<u64>,
    #[serde(default)]
    pub input: i64,
    /// Total output. `thinking` is a subset of it, never an addend.
    #[serde(default)]
    pub output: i64,
    #[serde(default)]
    pub cache_read: i64,
    #[serde(default)]
    pub cache_write: i64,
    #[serde(default)]
    pub thinking: i64,
}

/// The Artifact that stands in for `<session>.pb`.
pub fn file_name(session: &str) -> String {
    format!("{session}{SUFFIX}")
}

/// `<session>.tokenledger.json` → `<session>`; any other name → None.
pub fn session_id(path: &Path) -> Option<String> {
    path.file_name()?.to_str()?.strip_suffix(SUFFIX).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn the_name_a_writer_produces_is_the_name_a_reader_recognises() {
        let named = PathBuf::from("/tmp").join(file_name("abc-123"));
        assert_eq!(named.file_name().unwrap(), "abc-123.tokenledger.json");
        assert_eq!(session_id(&named).as_deref(), Some("abc-123"));
    }

    #[test]
    fn other_files_in_the_conversations_dir_are_not_exports() {
        assert_eq!(session_id(Path::new("/tmp/abc.pb")), None);
        assert_eq!(session_id(Path::new("/tmp/abc.db")), None);
        assert_eq!(session_id(Path::new("/tmp/abc.json")), None);
        // The staging name the companion renames from must never be picked up.
        assert_eq!(session_id(Path::new("/tmp/abc.tokenledger.json.part")), None);
    }

    #[test]
    fn a_written_export_round_trips_through_the_reader() {
        let written = ConversationExport {
            schema: SCHEMA,
            conversation_id: "s".into(),
            model: Some("gemini-3-pro-high".into()),
            project: None,
            generations: vec![GenerationExport {
                response_id: Some("r".into()),
                ts: 1_780_300_000,
                model_enum: Some(1008),
                input: 5,
                output: 3,
                cache_read: 2,
                cache_write: 0,
                thinking: 1,
            }],
        };
        let json = serde_json::to_string(&written).unwrap();
        let read: ConversationExport = serde_json::from_str(&json).unwrap();
        assert_eq!(read.conversation_id, "s");
        assert_eq!(read.generations[0].output, 3);
        assert_eq!(read.generations[0].thinking, 1);
        assert_eq!(read.model.as_deref(), Some("gemini-3-pro-high"));
    }
}
