use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq)]
pub struct UsageEvent {
    pub dedup_key: String,
    pub source: String,
    pub timestamp: i64,
    pub model: Option<String>, // None = Unattributed Usage, never a sentinel Model
    pub project: Option<String>,
    pub api_calls: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_5m_tokens: i64,
    pub cache_write_1h_tokens: i64,
    pub source_file: String,
    pub session_id: Option<String>,
    pub reasoning_tokens: Option<i64>,
    pub ctx: CtxTokens,
}

/// Attributed share of an event's billed context (input + cache_read +
/// cache_write). NULL = the source cannot attribute that category.
/// messages/system/reasoning partition billed exactly; toolcalls/agents/
/// mcp/skills are overlapping subsets of messages.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CtxTokens {
    pub messages: Option<i64>,
    pub system: Option<i64>,
    pub reasoning: Option<i64>,
    pub toolcalls: Option<i64>,
    pub agents: Option<i64>,
    pub mcp: Option<i64>,
    pub skills: Option<i64>,
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

#[derive(Debug, Serialize, Clone, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct SourceStatus {
    pub source: String,
    #[ts(type = "number")]
    pub events_inserted: u64,
    #[ts(type = "number")]
    pub lines_skipped: u64,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Clone, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct ScanStatus {
    pub sources: Vec<SourceStatus>,
    #[ts(type = "number")]
    pub scanned_at: i64,
}
