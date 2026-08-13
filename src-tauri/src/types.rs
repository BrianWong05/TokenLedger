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

/// One observation of one Limit — a rolling window with a ceiling that fills and
/// resets, imposed by a Source's vendor (CONTEXT.md).
/// A Limit Reading holds no tokens and never enters the Ledger; it is stored
/// verbatim, with `used_pct` carrying the vendor's own figure unconverted.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitReading {
    pub source: String,
    /// Opaque: Claude's own response key (`five_hour`, `seven_day_opus`),
    /// Codex's `w{canonical minutes}`. Never parsed for structure — it may one
    /// day carry a pool prefix.
    pub window_key: String,
    pub window_minutes: Option<i64>,
    pub used_pct: f64,
    pub resets_at: i64,
    pub observed_at: i64,
    /// 'logs' (read from an Artifact the scan already walks) | 'live' (fetched
    /// by a Companion, ADR-0019).
    pub via: String,
    /// The vendor's own plan value, raw — `rateLimitTier`, `plan_type`,
    /// `subscriptionTier`. It is the plan identity the evidence contract requires
    /// as well as the card's pill, so it must never be localized, prettified, or
    /// normalized on the way in; presentation belongs to the frontend. A Source
    /// that ever needs a label distinct from its identity brings a column of its
    /// own rather than reshaping this one.
    pub plan: Option<String>,
    pub provenance: ReadingProvenance,
}

/// What a Reading proves about itself beyond its own figures, so that two
/// Readings may be compared as evidence (spec: evidence participation contract).
/// Every field is unknown until a Source proves it, and unknown is never a
/// wildcard: a Reading missing any of them cannot bound a Limit Evidence
/// Interval. `source`, `plan` and `resets_at` on the Reading itself already
/// stand for Source, plan identity and reset epoch, so nothing here repeats them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReadingProvenance {
    /// Stable opaque account/subscription identity, shared with the Usage
    /// capture context. Never an email, token, or any reversible credential.
    pub account_id: Option<String>,
    /// The vendor meter in force, as the adapter identifies it — one plan may
    /// bill under several regimes, so a plan label alone is not this.
    pub metering_regime: Option<String>,
    /// Raw vendor Limit identity, or an adapter-defined canonical one with a
    /// documented one-to-one mapping. A duration, slot, or display label is not.
    pub limit_id: Option<String>,
    pub model_scope: Option<ModelScope>,
    /// Stable source order for Readings sharing one `observed_at`; without it
    /// same-second Readings cannot be ordered, and so bound nothing.
    pub source_order: Option<i64>,
    /// The earliest instant local capture of this Source and account is durably
    /// proven to cover, unbroken, through this Reading. An interval is complete
    /// only when this reaches back past its earlier anchor, so raising it is how
    /// a later unreadable-Artifact discovery withdraws coverage: the newest pass
    /// to prove a value replaces the stored one, in either direction, while a
    /// pass that proves nothing leaves it alone.
    pub covered_from: Option<i64>,
    /// A known or detected fact that activity outside local capture moved this
    /// Limit through the stretch ending here. Unknown is no such fact, not proof
    /// of absence — the mere possibility of invisible activity rejects nothing.
    ///
    /// A bounded signature (ADR-0011): an adapter-defined constant naming the
    /// kind of activity, whose length its own vocabulary fixes. Never a vendor
    /// payload, message, or anything whose size the user's data decides.
    pub external_activity: Option<String>,
}

/// Which Models a Limit meters: the whole Source, or an exact set of raw logged
/// Model identities. A display name, response-key tail, or slug is never a Model
/// mapping, so a Source that cannot name raw Models has no scope at all.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelScope {
    All,
    Models(Vec<String>),
}

impl ModelScope {
    /// The stored form: `all`, or a sorted, deduplicated JSON array — canonical
    /// so that two Readings scope the same Models exactly when their stored
    /// strings are equal. An empty set is no scope at all, and stores as unknown
    /// rather than as an empty one.
    pub fn stored(&self) -> Option<String> {
        match self {
            ModelScope::All => Some("all".to_string()),
            ModelScope::Models(models) => {
                let mut canonical: Vec<&str> = models.iter().map(String::as_str).collect();
                canonical.sort_unstable();
                canonical.dedup();
                (!canonical.is_empty())
                    .then(|| serde_json::to_string(&canonical).unwrap_or_default())
            }
        }
    }

    /// The inverse of `stored`. Anything else — junk, an empty set — is unknown
    /// scope rather than a wildcard.
    pub fn parse(stored: &str) -> Option<Self> {
        if stored == "all" {
            return Some(ModelScope::All);
        }
        let models: Vec<String> = serde_json::from_str(stored).ok()?;
        (!models.is_empty()).then_some(ModelScope::Models(models))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FileState {
    pub size: i64,
    pub mtime: i64,
    pub byte_offset: i64,
}

/// Metadata (never token figures) for a Source Session whose transcript the
/// Source has pruned (ADR-0016). Recorded so the Session still leaves history
/// in the Ledger; only cwd/model/timestamps/title, never usage.
#[derive(Debug, Clone)]
pub struct SourceSessionMeta {
    pub session_id: String,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub title: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Default, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SourceScanResult {
    pub events_inserted: u64,
    pub lines_skipped: u64,
    /// Unreadable Artifacts seen this scan (ADR-0017): present but permanently
    /// unparseable without violating ADR-0013, so counted — never warned — and
    /// the count marks token totals as floors.
    pub artifacts_unreadable: u64,
    /// Latest mtime among them, epoch seconds. Content is never newer than its
    /// file, so a window is definitely complete iff this precedes its start.
    pub unreadable_max_mtime: Option<i64>,
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
    #[ts(type = "number")]
    pub artifacts_unreadable: u64,
    #[ts(type = "number | null")]
    pub unreadable_max_mtime: Option<i64>,
    pub error: Option<String>,
}

/// One Source's persisted Unreadable-Artifact state (ADR-0017), written by
/// every scan and read back for the ≥ floor marker — from the DB rather than
/// scan memory, so the Menu Bar Extra is honest from launch, before this
/// run's first scan lands. Field names mirror SourceStatus so the frontend
/// rule (src/lib/tokenCompleteness.ts) reads both shapes.
#[derive(Debug, Serialize, Clone, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct SourceUnreadable {
    pub source: String,
    #[ts(type = "number")]
    pub artifacts_unreadable: u64,
    #[ts(type = "number | null")]
    pub unreadable_max_mtime: Option<i64>,
}

#[derive(Debug, Serialize, Clone, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct ScanStatus {
    pub sources: Vec<SourceStatus>,
    #[ts(type = "number")]
    pub scanned_at: i64,
}
