use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::scan::SourceRoots;
use crate::source_catalog;
use crate::{db, queries, scan, types};

const SOURCE_ENV: &str = "TOKENLEDGER_VALIDATION_SOURCE";
const ARTIFACT_ENV: &str = "TOKENLEDGER_VALIDATION_ARTIFACT";

#[derive(Debug, PartialEq, Eq)]
enum SelectionError {
    MissingSource,
    MissingArtifact,
    UnknownSource,
}

struct Selection {
    source: String,
    artifact: PathBuf,
}

#[derive(Default, PartialEq, Eq)]
struct ValidationCounts {
    records: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    total_tokens: i64,
    requests: i64,
    unattributed_tokens: i64,
}

struct ValidationReport {
    source: String,
    counts: ValidationCounts,
    schema_fingerprint: Option<String>,
    result: &'static str,
}

impl ValidationReport {
    fn to_json(&self) -> String {
        serde_json::json!({
            "source": &self.source,
            "counts": {
                "records": self.counts.records,
                "input_tokens": self.counts.input_tokens,
                "output_tokens": self.counts.output_tokens,
                "cache_read_tokens": self.counts.cache_read_tokens,
                "cache_write_tokens": self.counts.cache_write_tokens,
                "total_tokens": self.counts.total_tokens,
                "requests": self.counts.requests,
                "unattributed_tokens": self.counts.unattributed_tokens,
            },
            "schema_fingerprint": &self.schema_fingerprint,
            "result": self.result,
        })
        .to_string()
    }
}

impl Selection {
    fn from_env() -> Result<Self, SelectionError> {
        let source = std::env::var(SOURCE_ENV).map_err(|_| SelectionError::MissingSource)?;
        let artifact = std::env::var_os(ARTIFACT_ENV).ok_or(SelectionError::MissingArtifact)?;
        Self::from_values(&source, Path::new(&artifact))
    }

    fn from_values(source: &str, artifact: &Path) -> Result<Self, SelectionError> {
        let source = source.trim().to_ascii_lowercase();
        if source.is_empty() {
            return Err(SelectionError::MissingSource);
        }
        if source_catalog::source(&source).is_none() {
            return Err(SelectionError::UnknownSource);
        }
        if artifact.as_os_str().is_empty() {
            return Err(SelectionError::MissingArtifact);
        }
        Ok(Self {
            source,
            artifact: artifact.to_path_buf(),
        })
    }
}

fn roots_for(source: &str, artifact: &Path, missing: &Path) -> SourceRoots {
    let missing = missing.to_path_buf();
    let mut roots = SourceRoots {
        claude: missing.clone(),
        codex: missing.clone(),
        gemini_tmp: missing.clone(),
        gemini_projects_json: missing.clone(),
        hermes_db: missing.clone(),
        grok_sessions: missing.clone(),
        antigravity_conversations: missing.clone(),
        antigravity_cli_conversations: missing.clone(),
        goose_sessions: vec![missing.clone()],
        pi_sessions: vec![missing.clone()],
    };

    match source {
        "claude" => roots.claude = artifact.to_path_buf(),
        "codex" => roots.codex = artifact.to_path_buf(),
        "gemini" => {
            roots.gemini_tmp = artifact.to_path_buf();
            roots.gemini_projects_json = sibling_projects_json(artifact, &missing);
        }
        "hermes" => roots.hermes_db = artifact.to_path_buf(),
        "grok" => roots.grok_sessions = artifact.to_path_buf(),
        "antigravity" => roots.antigravity_conversations = artifact.to_path_buf(),
        "goose" => roots.goose_sessions = vec![artifact.to_path_buf()],
        "pi" => roots.pi_sessions = vec![artifact.to_path_buf()],
        _ => {}
    }

    roots
}

fn sibling_projects_json(artifact: &Path, missing: &Path) -> PathBuf {
    let Some(parent) = artifact.parent() else {
        return missing.to_path_buf();
    };
    let candidate = parent.join("projects.json");
    if candidate.is_file() {
        candidate
    } else {
        missing.to_path_buf()
    }
}

fn validation_counts(conn: &Connection, source: &str) -> rusqlite::Result<ValidationCounts> {
    let filters = queries::Filters {
        tools: vec![source.to_string()],
        ..Default::default()
    };
    let summary = queries::summary(conn, &filters)?;
    let records = conn.query_row(
        "SELECT COUNT(*) FROM events WHERE source = ?1",
        [source],
        |row| row.get(0),
    )?;
    Ok(ValidationCounts {
        records,
        input_tokens: summary.input_tokens,
        output_tokens: summary.output_tokens,
        cache_read_tokens: summary.cache_read_tokens,
        cache_write_tokens: summary.cache_write_tokens,
        total_tokens: summary.total_tokens,
        requests: summary.requests,
        unattributed_tokens: summary.unattributed_tokens,
    })
}

fn artifact_files(root: &Path) -> Result<Vec<PathBuf>, ()> {
    fn visit(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), ()> {
        let metadata = fs::symlink_metadata(path).map_err(|_| ())?;
        if metadata.file_type().is_file() {
            files.push(path.to_path_buf());
            return Ok(());
        }
        if !metadata.file_type().is_dir() {
            return Ok(());
        }
        for entry in fs::read_dir(path).map_err(|_| ())? {
            visit(&entry.map_err(|_| ())?.path(), files)?;
        }
        Ok(())
    }

    let mut files = Vec::new();
    visit(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_json_shape(value: &Value, path: &mut Vec<String>, shapes: &mut BTreeSet<String>) {
    let location = if path.is_empty() {
        "$".to_string()
    } else {
        format!("$.{}", path.join("."))
    };
    let kind = match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    };
    shapes.insert(format!("{location}:{kind}"));

    match value {
        Value::Array(items) => {
            path.push("[]".to_string());
            for item in items {
                collect_json_shape(item, path, shapes);
            }
            path.pop();
        }
        Value::Object(fields) => {
            for (key, field) in fields {
                path.push(key.clone());
                collect_json_shape(field, path, shapes);
                path.pop();
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn collect_jsonl_schema(path: &Path, shapes: &mut BTreeSet<String>) -> Result<bool, ()> {
    let file = fs::File::open(path).map_err(|_| ())?;
    let mut parsed = false;
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|_| ())?;
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        collect_json_shape(&value, &mut Vec::new(), shapes);
        parsed = true;
    }
    Ok(parsed)
}

fn collect_json_schema(path: &Path, shapes: &mut BTreeSet<String>) -> Result<bool, ()> {
    let content = fs::read_to_string(path).map_err(|_| ())?;
    let value = serde_json::from_str::<Value>(&content).map_err(|_| ())?;
    collect_json_shape(&value, &mut Vec::new(), shapes);
    Ok(true)
}

fn collect_sqlite_schema(path: &Path, shapes: &mut BTreeSet<String>) -> Result<bool, ()> {
    let conn =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|_| ())?;
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| ())?;
    shapes.insert(format!("sqlite:user_version:{version}"));
    let mut statement = conn
        .prepare(
            "SELECT type, name, tbl_name, sql
             FROM sqlite_master
             WHERE sql IS NOT NULL
             ORDER BY type, name, tbl_name",
        )
        .map_err(|_| ())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|_| ())?;
    for row in rows {
        let (kind, name, table, sql) = row.map_err(|_| ())?;
        shapes.insert(format!("sqlite:{kind}:{name}:{table}:{sql}"));
    }
    Ok(true)
}

fn artifact_schema_fingerprint(source: &str, artifact: &Path) -> Result<String, ()> {
    let files = artifact_files(artifact)?;
    let mut shapes = BTreeSet::new();
    let mut considered = false;
    for path in files {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase());
        if (source == "hermes" || source == "goose" || source == "antigravity")
            && extension.as_deref() == Some("db")
        {
            considered = true;
            collect_sqlite_schema(&path, &mut shapes)?;
        } else if extension.as_deref() == Some("jsonl") {
            considered = true;
            let _ = collect_jsonl_schema(&path, &mut shapes)?;
        } else if extension.as_deref() == Some("json") {
            considered = true;
            let _ = collect_json_schema(&path, &mut shapes)?;
        }
    }
    if !considered || shapes.is_empty() {
        return Err(());
    }

    let mut hash = Sha256::new();
    hash.update(b"tokenledger-source-artifact-schema-v1\0");
    hash.update(source.as_bytes());
    hash.update([0]);
    for shape in shapes {
        hash.update(shape.as_bytes());
        hash.update([0]);
    }
    Ok(format!("sha256:{:x}", hash.finalize()))
}

const PRIVACY_MARKERS: &[&str] = &[
    "PRIVATE_PROMPT_SHOULD_NOT_PERSIST",
    "PRIVATE_RESPONSE_SHOULD_NOT_PERSIST",
    "PRIVATE_REASONING_SHOULD_NOT_PERSIST",
    "PRIVATE_IMAGE_SHOULD_NOT_PERSIST",
    "PRIVATE_TOOL_ARG_SHOULD_NOT_PERSIST",
    "PRIVATE_TOOL_RESULT_SHOULD_NOT_PERSIST",
    "PRIVATE_ERROR_SHOULD_NOT_PERSIST",
    "GOOSE_PRIVATE_PROMPT_MARKER",
    "GOOSE_PRIVATE_RESPONSE_MARKER",
];

fn ledger_has_no_privacy_markers(db_path: &Path) -> bool {
    let mut bytes = Vec::new();
    for suffix in ["", "-wal", "-shm"] {
        if let Ok(mut file_bytes) = std::fs::read(format!("{}{}", db_path.display(), suffix)) {
            bytes.append(&mut file_bytes);
        }
    }
    PRIVACY_MARKERS.iter().all(|marker| {
        !bytes
            .windows(marker.len())
            .any(|window| window == marker.as_bytes())
    })
}

fn source_status<'a>(
    status: &'a types::ScanStatus,
    source: &str,
) -> Option<&'a types::SourceStatus> {
    status.sources.iter().find(|item| item.source == source)
}

fn validate(selection: &Selection) -> ValidationReport {
    let source = selection.source.clone();
    let failed = |counts, schema_fingerprint| ValidationReport {
        source: source.clone(),
        counts,
        schema_fingerprint,
        result: "fail",
    };

    let fingerprint = artifact_schema_fingerprint(&source, &selection.artifact).ok();
    let Ok(ledger_dir) = tempfile::tempdir() else {
        return failed(ValidationCounts::default(), fingerprint);
    };
    let missing = ledger_dir.path().join("unselected-source");
    let roots = roots_for(&source, &selection.artifact, &missing);
    let db_path = ledger_dir.path().join("validation-ledger.db");
    let Ok(mut conn) = db::open_db(&db_path) else {
        return failed(ValidationCounts::default(), fingerprint);
    };

    let first_scan = scan::run_scan(&mut conn, &roots);
    let first_counts = validation_counts(&conn, &source).unwrap_or_default();
    let other_source_records: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE source <> ?1",
            [&source],
            |row| row.get(0),
        )
        .unwrap_or(-1);

    let second_scan = scan::run_scan(&mut conn, &roots);
    let second_counts = validation_counts(&conn, &source).unwrap_or_default();
    let first_status = source_status(&first_scan, &source);
    let second_status = source_status(&second_scan, &source);
    let other_sources_untouched = first_scan
        .sources
        .iter()
        .chain(second_scan.sources.iter())
        .filter(|item| item.source != source)
        .all(|item| item.events_inserted == 0);
    let validation_invariants_hold = first_counts == second_counts
        && first_counts.total_tokens > 0
        && first_status.is_some_and(|item| item.events_inserted > 0 && item.error.is_none())
        && second_status.is_some_and(|item| item.events_inserted == 0 && item.error.is_none())
        && other_sources_untouched
        && other_source_records == 0;

    drop(conn);
    let privacy_safe = ledger_has_no_privacy_markers(&db_path);
    if validation_invariants_hold && fingerprint.is_some() && privacy_safe {
        ValidationReport {
            source,
            counts: first_counts,
            schema_fingerprint: fingerprint,
            result: "pass",
        }
    } else {
        failed(first_counts, fingerprint)
    }
}

#[test]
#[ignore]
fn private_source_artifact_validation() {
    let selection = Selection::from_env().unwrap_or_else(|_| {
        panic!("set TOKENLEDGER_VALIDATION_SOURCE and TOKENLEDGER_VALIDATION_ARTIFACT")
    });
    let report = validate(&selection);
    println!("{}", report.to_json());
    assert_eq!(
        report.result, "pass",
        "private Source Artifact validation failed"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_artifact_is_used_as_the_single_source_root() {
        let artifact = PathBuf::from("/private/source-artifact");
        let missing = Path::new("/private/missing");

        let roots = roots_for("pi", &artifact, missing);

        assert_eq!(roots.pi_sessions, vec![artifact]);
        assert_eq!(roots.claude, missing);
        assert_eq!(roots.codex, missing);
    }

    #[test]
    fn selected_goose_artifact_is_used_as_a_session_root() {
        let artifact = PathBuf::from("/private/goose-sessions");
        let missing = Path::new("/private/missing");

        let roots = roots_for("goose", &artifact, missing);

        assert_eq!(roots.goose_sessions, vec![artifact]);
        assert_eq!(roots.pi_sessions, vec![missing]);
    }

    #[test]
    fn selection_normalizes_the_source_key_without_exposing_the_artifact() {
        let selection = Selection::from_values(" PI ", Path::new("/private/secret"))
            .expect("known Source should be accepted");

        assert_eq!(selection.source, "pi");
        assert_eq!(selection.artifact, PathBuf::from("/private/secret"));
    }

    #[test]
    fn report_json_contains_only_aggregate_evidence() {
        let report = ValidationReport {
            source: "pi".to_string(),
            counts: ValidationCounts {
                records: 4,
                input_tokens: 10,
                output_tokens: 20,
                cache_read_tokens: 30,
                cache_write_tokens: 40,
                total_tokens: 100,
                requests: 4,
                unattributed_tokens: 0,
            },
            schema_fingerprint: Some("sha256:abc".to_string()),
            result: "pass",
        };

        let json = report.to_json();
        assert!(json.contains("\"schema_fingerprint\":\"sha256:abc\""));
        assert!(json.contains("\"total_tokens\":100"));
        assert!(!json.contains("/private/secret"));
        assert!(!json.contains("PRIVATE_PROMPT"));
    }

    #[test]
    fn artifact_schema_fingerprint_omits_json_values() {
        let directory = tempfile::tempdir().unwrap();
        let artifact = directory.path().join("session.jsonl");
        fs::write(&artifact, r#"{"id":"first","usage":{"input":1}}"#).unwrap();
        let first = artifact_schema_fingerprint("pi", &artifact).unwrap();

        fs::write(&artifact, r#"{"id":"second","usage":{"input":999999}}"#).unwrap();
        let second = artifact_schema_fingerprint("pi", &artifact).unwrap();

        assert_eq!(first, second);
        assert!(!first.contains("first"));
        assert!(!first.contains("second"));
    }

    #[test]
    fn validation_accepts_a_selected_fixture_file() {
        let artifact = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/adapters/fixtures/pi/basic-session.jsonl");
        let selection = Selection::from_values("pi", &artifact).unwrap();

        let report = validate(&selection);

        assert_eq!(report.result, "pass");
    }
}
