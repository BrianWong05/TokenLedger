// TokenLedger — live rate-limit windows ("Limits" page backend).
//
// Each provider submodule reads the tool's LOCAL credentials and calls the
// vendor's own usage endpoint, normalizing to LimitWindow rows. Everything
// secret stays in this process; the webview only sees percentages, labels,
// and error strings. Mechanism follows mm7894215/TokenTracker
// (src/lib/usage-limits.js), ported to Rust.
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

// pub mod antigravity;
// pub mod claude;
// pub mod codex;
// pub mod gemini;
pub mod grok;

/// Live fetch failure. `retry_after_secs` is set only by Claude's 429 path
/// so the orchestrator can arm the cooldown.
#[derive(Debug)]
pub struct FetchErr {
    pub message: String,
    pub retry_after_secs: Option<i64>,
}

impl From<String> for FetchErr {
    fn from(message: String) -> Self {
        FetchErr { message, retry_after_secs: None }
    }
}

impl From<&str> for FetchErr {
    fn from(message: &str) -> Self {
        FetchErr { message: message.to_string(), retry_after_secs: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitWindow {
    pub label: String,
    pub used_percent: f64,
    pub resets_at_ts: Option<i64>, // epoch seconds
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolLimits {
    pub source: String, // "claude" | "codex" | "gemini" | "grok" | "antigravity"
    pub configured: bool,
    pub error: Option<String>,
    pub plan: Option<String>,
    pub windows: Vec<LimitWindow>,
    pub stale: bool,
    pub cached_at_ts: Option<i64>, // epoch seconds
}

impl ToolLimits {
    pub fn not_configured(source: &str) -> Self {
        ToolLimits {
            source: source.to_string(),
            configured: false,
            error: None,
            plan: None,
            windows: Vec::new(),
            stale: false,
            cached_at_ts: None,
        }
    }

    pub fn error_card(source: &str, msg: impl Into<String>) -> Self {
        ToolLimits {
            source: source.to_string(),
            configured: true,
            error: Some(msg.into()),
            plan: None,
            windows: Vec::new(),
            stale: false,
            cached_at_ts: None,
        }
    }

    pub fn live(source: &str, plan: Option<String>, windows: Vec<LimitWindow>, now_ts: i64) -> Self {
        ToolLimits {
            source: source.to_string(),
            configured: true,
            error: None,
            plan,
            windows,
            stale: false,
            cached_at_ts: Some(now_ts),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitsSnapshot {
    pub fetched_at_ts: i64,
    pub tools: Vec<ToolLimits>,
}

// ---- disk cache -----------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedTool {
    pub tool: ToolLimits,
    pub cached_at_ts: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DiskCache {
    pub claude_retry_at_ts: Option<i64>,
    pub tools: HashMap<String, CachedTool>,
}

pub fn load_cache(path: &Path) -> DiskCache {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Atomic write (tmp + rename) so a kill mid-write can't corrupt the cache.
pub fn save_cache(path: &Path, cache: &DiskCache) {
    let Ok(json) = serde_json::to_string(cache) else { return };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

// ---- fallback policy ------------------------------------------------------

pub const STALE_MAX_AGE_SECS: i64 = 7 * 24 * 3600;
pub const CLAUDE_FRESH_SECS: i64 = 600;
pub const SNAPSHOT_TTL_SECS: u64 = 120;

/// A cached entry is servable only if it was a real success (has bars).
fn cache_usable(c: &CachedTool, now_ts: i64) -> bool {
    now_ts - c.cached_at_ts <= STALE_MAX_AGE_SECS
        && c.tool.configured
        && c.tool.error.is_none()
        && !c.tool.windows.is_empty()
}

/// Live result wins; a live failure serves the last good read (marked stale)
/// so one dead vendor never blanks its card; no cache → error card.
pub fn with_fallback(
    source: &str,
    live: Result<ToolLimits, FetchErr>,
    cached: Option<&CachedTool>,
    now_ts: i64,
) -> ToolLimits {
    match live {
        Ok(t) => t,
        Err(e) => {
            if let Some(c) = cached {
                if cache_usable(c, now_ts) {
                    let mut t = c.tool.clone();
                    t.stale = true;
                    t.cached_at_ts = Some(c.cached_at_ts);
                    return t;
                }
            }
            ToolLimits::error_card(source, e.message)
        }
    }
}

// ---- shared helpers -------------------------------------------------------

pub fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn clamp_percent(v: f64) -> f64 {
    if !v.is_finite() {
        return 0.0;
    }
    v.clamp(0.0, 100.0)
}

pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .build()
}

/// Vendor reset stamps come as ISO strings, epoch-second numbers, or
/// epoch-second numeric strings (Antigravity). → epoch seconds.
pub fn parse_any_ts(v: &serde_json::Value) -> Option<i64> {
    match v {
        serde_json::Value::Number(n) => {
            let f = n.as_f64()?;
            if f > 0.0 { Some(f as i64) } else { None }
        }
        serde_json::Value::String(s) => {
            if let Ok(n) = s.parse::<f64>() {
                return if n > 0.0 { Some(n as i64) } else { None };
            }
            crate::time::iso_to_epoch(s)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn win() -> Vec<LimitWindow> {
        vec![LimitWindow { label: "5h".into(), used_percent: 11.0, resets_at_ts: Some(1_800_000_000) }]
    }

    #[test]
    fn clamps_percent() {
        assert_eq!(clamp_percent(-3.0), 0.0);
        assert_eq!(clamp_percent(140.0), 100.0);
        assert_eq!(clamp_percent(f64::NAN), 0.0);
        assert_eq!(clamp_percent(42.5), 42.5);
    }

    #[test]
    fn cache_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("limits-cache.json");
        let mut cache = DiskCache::default();
        cache.claude_retry_at_ts = Some(123);
        cache.tools.insert(
            "grok".into(),
            CachedTool { tool: ToolLimits::live("grok", None, win(), 1000), cached_at_ts: 1000 },
        );
        save_cache(&path, &cache);
        let loaded = load_cache(&path);
        assert_eq!(loaded.claude_retry_at_ts, Some(123));
        assert_eq!(loaded.tools["grok"].tool.windows[0].label, "5h");
    }

    #[test]
    fn load_cache_defaults_on_missing_or_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let missing = load_cache(&dir.path().join("nope.json"));
        assert!(missing.tools.is_empty());
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "{not json").unwrap();
        assert!(load_cache(&bad).tools.is_empty());
    }

    #[test]
    fn fallback_prefers_live() {
        let live = ToolLimits::live("grok", None, win(), 2000);
        let out = with_fallback("grok", Ok(live.clone()), None, 2000);
        assert_eq!(out, live);
    }

    #[test]
    fn fallback_serves_fresh_cache_as_stale() {
        let cached = CachedTool { tool: ToolLimits::live("grok", None, win(), 1000), cached_at_ts: 1000 };
        let out = with_fallback("grok", Err("boom".into()), Some(&cached), 2000);
        assert!(out.stale);
        assert_eq!(out.cached_at_ts, Some(1000));
        assert_eq!(out.windows.len(), 1);
    }

    #[test]
    fn fallback_rejects_expired_cache() {
        let cached = CachedTool { tool: ToolLimits::live("grok", None, win(), 0), cached_at_ts: 0 };
        let now = STALE_MAX_AGE_SECS + 1;
        let out = with_fallback("grok", Err("boom".into()), Some(&cached), now);
        assert_eq!(out.error.as_deref(), Some("boom"));
        assert!(out.windows.is_empty());
    }

    #[test]
    fn fallback_rejects_windowless_cache() {
        let cached = CachedTool {
            tool: ToolLimits::error_card("grok", "old error"),
            cached_at_ts: 1000,
        };
        let out = with_fallback("grok", Err("boom".into()), Some(&cached), 1001);
        assert_eq!(out.error.as_deref(), Some("boom"));
    }

    #[test]
    fn parses_any_ts() {
        assert_eq!(parse_any_ts(&json!(1780308000)), Some(1780308000));
        assert_eq!(parse_any_ts(&json!("1780308000")), Some(1780308000));
        assert_eq!(parse_any_ts(&json!("2026-06-01T10:00:00Z")), Some(1780308000));
        assert_eq!(parse_any_ts(&json!(null)), None);
        assert_eq!(parse_any_ts(&json!(0)), None);
    }
}
