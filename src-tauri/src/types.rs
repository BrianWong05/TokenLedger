use serde::Serialize;

#[derive(Debug, Clone, PartialEq)]
pub struct UsageEvent {
    pub dedup_key: String,
    pub source: String,
    pub timestamp: i64,
    pub model: String,
    pub project: Option<String>,
    pub api_calls: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_5m_tokens: i64,
    pub cache_write_1h_tokens: i64,
    pub source_file: String,
}

#[derive(Debug, Clone, Copy)]
pub struct FileState {
    pub size: i64,
    pub mtime: i64,
    pub byte_offset: i64,
}

#[derive(Debug, Default, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SourceScanResult {
    pub events_inserted: u64,
    pub lines_skipped: u64,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SourceStatus {
    pub source: String,
    pub events_inserted: u64,
    pub lines_skipped: u64,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatus {
    pub sources: Vec<SourceStatus>,
    pub scanned_at: i64,
}
