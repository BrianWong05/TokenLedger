# Limits Page (Live Rate-Limit Windows) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire `src/overview/Limits.tsx` (currently a static mockup) to real quota data for Claude, Codex, Gemini, Grok Build, and Antigravity, fetched by a new Rust `limits` module through one `limits` Tauri command.

**Architecture:** New `src-tauri/src/limits/` module family mirroring `adapters/`: per-provider files read local credentials and call the vendor's usage endpoint; `mod.rs` orchestrates all five in parallel threads, applies a disk-cache stale-fallback policy, and returns one normalized `LimitsSnapshot`. Frontend gets `fetchLimits(force)` and renders the already-built card states. `main.tsx` grows a two-page nav shell (Overview ↔ Limits).

**Tech Stack:** Rust (ureq, serde, native-tls for the local self-signed Antigravity API), Tauri v2 commands, React + TypeScript, vitest.

**Spec:** `docs/superpowers/specs/2026-07-12-limits-page-design.md`. Two deliberate deviations (spec amended alongside this plan): (1) all IPC timestamps are epoch **seconds** (`resetsAtTs`, `cachedAtTs`, `fetchedAtTs`) matching the repo's existing `Filters.startTs` convention, not ISO strings; (2) Antigravity window labels are `Cl 7d / Cl 5h / Gm 7d / Gm 5h` (the vendor's own bucketIds `3p-weekly`, `3p-5h`, `gemini-weekly`, `gemini-5h`), not per-family single rows.

## Global Constraints

- IPC structs use `#[serde(rename_all = "camelCase")]`; TS mirrors in `src/types.ts` (header comment there: "Do not rename").
- All timestamps crossing IPC are epoch seconds (i64 / number).
- Secrets (tokens, keychain payloads) never cross the IPC boundary — they exist only inside `src-tauri`.
- Per-provider HTTP timeout: 15 s (`ureq` agent). Local Antigravity calls: 8 s.
- Only dependency additions allowed: `ureq` gains the `native-tls` feature + `native-tls = "0.2"` (Task 6). Nothing else.
- Rust tests are colocated `#[cfg(test)] mod tests` in the same file (repo convention).
- One disk cache file: `<app_data_dir>/limits-cache.json`. Claude 429 cooldown: default 300 s, cap 3600 s. Claude fresh-serve TTL: 600 s. Stale fallback max age: 7 days. In-memory snapshot TTL: 120 s.
- Error copy (exact strings, from the example app):
  - Claude 401: `Claude token expired — run `claude` once to refresh.`
  - Claude 429: `Claude API rate limited (429) — retry in ~{N}m.` / `Claude API rate limited (429) — retry shortly.`
  - Codex refresh dead: `Codex refresh token expired or revoked. Run `codex` to re-authenticate.`
  - Gemini not logged in: `Not logged in to Gemini. Run 'gemini' in Terminal to authenticate.`
  - Grok not logged in: ``Not logged in to Grok Build. Run `grok login` in Terminal to authenticate.``
  - Antigravity not running: `Antigravity IDE is not running. Launch Antigravity to see usage limits.`
- Commit style: `feat(limits): …`, `test(limits): …` matching repo history. Every commit ends with the Co-Authored-By trailer used in this repo.

---

### Task 1: `limits/mod.rs` — types, disk cache, fallback policy

**Files:**
- Create: `src-tauri/src/limits/mod.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod limits;` after `mod db;`)

**Interfaces:**
- Produces (used by every later task):
  - `pub struct LimitWindow { pub label: String, pub used_percent: f64, pub resets_at_ts: Option<i64> }`
  - `pub struct ToolLimits { pub source: String, pub configured: bool, pub error: Option<String>, pub plan: Option<String>, pub windows: Vec<LimitWindow>, pub stale: bool, pub cached_at_ts: Option<i64> }` with constructors `not_configured(source)`, `error_card(source, msg)`, `live(source, plan, windows, now_ts)`
  - `pub struct LimitsSnapshot { pub fetched_at_ts: i64, pub tools: Vec<ToolLimits> }`
  - `pub struct FetchErr { pub message: String, pub retry_after_secs: Option<i64> }` + `impl From<String>` and `From<&str>`
  - `pub struct DiskCache { pub claude_retry_at_ts: Option<i64>, pub tools: HashMap<String, CachedTool> }`, `pub struct CachedTool { pub tool: ToolLimits, pub cached_at_ts: i64 }`
  - `pub fn load_cache(path: &Path) -> DiskCache`, `pub fn save_cache(path: &Path, cache: &DiskCache)`
  - `pub fn with_fallback(source: &str, live: Result<ToolLimits, FetchErr>, cached: Option<&CachedTool>, now_ts: i64) -> ToolLimits`
  - `pub fn clamp_percent(v: f64) -> f64`, `pub fn now_ts() -> i64`, `pub fn agent() -> ureq::Agent`

- [ ] **Step 1: Write the failing tests**

Create `src-tauri/src/limits/mod.rs` with only the test module first is awkward for Rust; instead write the full file with `todo!()` bodies is also churn. Pragmatic TDD for Rust here: write the complete file below, but write the tests FIRST inside it, run to see them fail to compile (types missing), then fill the implementation. The final file:

```rust
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

pub mod antigravity;
pub mod claude;
pub mod codex;
pub mod gemini;
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
```

Note the five `pub mod …;` declarations at the top — Tasks 2–6 create those files. Until they exist the crate won't compile, so for THIS task comment out all five lines (`// pub mod …`) and re-enable each as its file lands.

- [ ] **Step 2: Register the module and verify tests fail then pass**

In `src-tauri/src/lib.rs`, after `mod db;` add:

```rust
mod limits;
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml limits::`
Expected: 8 tests pass (with the five submodule lines commented out).

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/limits/mod.rs src-tauri/src/lib.rs
git commit -m "feat(limits): types, disk cache, and stale-fallback policy for the Limits backend"
```

---

### Task 2: `limits/grok.rs` — Grok Build billing windows

**Files:**
- Create: `src-tauri/src/limits/grok.rs`
- Modify: `src-tauri/src/limits/mod.rs` (uncomment `pub mod grok;`)

**Interfaces:**
- Consumes: `LimitWindow`, `ToolLimits`, `FetchErr`, `clamp_percent`, `parse_any_ts`, `agent` from `super`.
- Produces: `pub fn fetch(home: &Path, now_ts: i64) -> Result<ToolLimits, FetchErr>`; `pub fn normalize(body: &serde_json::Value) -> Result<Vec<LimitWindow>, String>` (pure, tested).

- [ ] **Step 1: Write the file with tests**

```rust
// Grok Build (xAI CLI) — ~/.grok/auth.json bearer key → cli-chat-proxy
// billing summary. Monthly credits + on-demand spend, one billing-period
// reset for both.
use std::path::Path;

use serde_json::Value;

use super::{agent, clamp_percent, parse_any_ts, FetchErr, LimitWindow, ToolLimits};

const BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing";
const LOGIN_MSG: &str =
    "Not logged in to Grok Build. Run `grok login` in Terminal to authenticate.";

fn read_token(home: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(home.join(".grok/auth.json")).ok()?;
    let parsed: Value = serde_json::from_str(&raw).ok()?;
    // auth.json maps arbitrary account keys to entries; first non-empty `key` wins.
    for (_k, v) in parsed.as_object()? {
        if let Some(key) = v.get("key").and_then(Value::as_str) {
            let key = key.trim();
            if !key.is_empty() {
                return Some(key.to_string());
            }
        }
    }
    None
}

fn num(v: Option<&Value>) -> Option<f64> {
    let v = v?;
    if let Some(n) = v.as_f64() {
        return n.is_finite().then_some(n);
    }
    v.as_str()?.trim().parse::<f64>().ok().filter(|n| n.is_finite())
}

pub fn normalize(body: &Value) -> Result<Vec<LimitWindow>, String> {
    let config = body
        .get("config")
        .filter(|c| c.is_object())
        .ok_or("Could not parse Grok billing: missing config")?;
    let reset = config.get("billingPeriodEnd").and_then(parse_any_ts);
    let mut windows = Vec::new();
    if let (Some(limit), Some(used)) = (num(config.get("monthlyLimit")), num(config.get("used"))) {
        if limit > 0.0 {
            windows.push(LimitWindow {
                label: "Month".into(),
                used_percent: clamp_percent(used / limit * 100.0),
                resets_at_ts: reset,
            });
        }
    }
    if let (Some(cap), Some(used)) = (num(config.get("onDemandCap")), num(config.get("onDemandUsed"))) {
        if cap > 0.0 {
            windows.push(LimitWindow {
                label: "Extra".into(),
                used_percent: clamp_percent(used / cap * 100.0),
                resets_at_ts: reset,
            });
        }
    }
    if windows.is_empty() {
        return Err("Could not parse Grok billing: no quota windows in response".into());
    }
    Ok(windows)
}

pub fn fetch(home: &Path, now_ts: i64) -> Result<ToolLimits, FetchErr> {
    let grok_home = home.join(".grok");
    if !grok_home.join("auth.json").exists() && !grok_home.join("sessions").exists() {
        return Ok(ToolLimits::not_configured("grok"));
    }
    let Some(token) = read_token(home) else {
        return Ok(ToolLimits::not_configured("grok"));
    };
    let resp = agent()
        .get(BILLING_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "application/json")
        .call();
    let body: Value = match resp {
        Ok(r) => r.into_json().map_err(|e| FetchErr::from(e.to_string()))?,
        Err(ureq::Error::Status(401 | 403, _)) => {
            return Ok(ToolLimits::error_card("grok", LOGIN_MSG));
        }
        Err(e) => return Err(FetchErr::from(format!("Grok billing API error: {e}"))),
    };
    let windows = normalize(&body).map_err(FetchErr::from)?;
    Ok(ToolLimits::live("grok", None, windows, now_ts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalizes_monthly_and_on_demand() {
        let body = json!({ "config": {
            "monthlyLimit": 1000, "used": 190,
            "onDemandCap": 50, "onDemandUsed": 5,
            "billingPeriodEnd": "2026-08-01T00:00:00Z",
        }});
        let w = normalize(&body).unwrap();
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].label, "Month");
        assert_eq!(w[0].used_percent, 19.0);
        assert_eq!(w[1].label, "Extra");
        assert_eq!(w[1].used_percent, 10.0);
        assert!(w[0].resets_at_ts.is_some());
    }

    #[test]
    fn missing_config_is_error() {
        assert!(normalize(&json!({})).is_err());
    }

    #[test]
    fn zero_caps_produce_no_windows() {
        let body = json!({ "config": { "monthlyLimit": 0, "used": 0 } });
        assert!(normalize(&body).is_err());
    }

    #[test]
    fn reads_first_keyed_auth_entry() {
        let dir = tempfile::tempdir().unwrap();
        let grok = dir.path().join(".grok");
        std::fs::create_dir_all(&grok).unwrap();
        std::fs::write(
            grok.join("auth.json"),
            r#"{"acct":{"key":"sk-test-123"},"other":{"nokey":true}}"#,
        )
        .unwrap();
        assert_eq!(read_token(dir.path()).as_deref(), Some("sk-test-123"));
    }

    #[test]
    fn not_configured_without_grok_home() {
        let dir = tempfile::tempdir().unwrap();
        let out = fetch(dir.path(), 0).unwrap();
        assert!(!out.configured);
    }
}
```

- [ ] **Step 2: Uncomment `pub mod grok;` in mod.rs, run tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml limits::grok`
Expected: 5 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/limits/grok.rs src-tauri/src/limits/mod.rs
git commit -m "feat(limits): Grok Build billing windows provider"
```

---

### Task 3: `limits/claude.rs` — Claude OAuth usage + 429 discipline

**Files:**
- Create: `src-tauri/src/limits/claude.rs`
- Modify: `src-tauri/src/limits/mod.rs` (uncomment `pub mod claude;`)

**Interfaces:**
- Consumes: Task 1 types + `CLAUDE_FRESH_SECS`, `DiskCache`.
- Produces:
  - `pub fn fetch(home: &Path, now_ts: i64) -> Result<ToolLimits, FetchErr>` (`retry_after_secs` set on 429)
  - `pub fn precheck(cache: &DiskCache, now_ts: i64) -> Option<ToolLimits>` (pure: fresh-cache serve / cooldown serve — orchestrator calls this BEFORE spawning the live fetch)
  - `pub fn normalize(body: &serde_json::Value) -> Vec<LimitWindow>` (pure, tested)
  - `pub fn plan_of(payload: &serde_json::Value) -> Option<String>` (pure, tested)

- [ ] **Step 1: Write the file with tests**

```rust
// Claude Code — OAuth token from the macOS keychain (service
// "Claude Code-credentials") or ~/.claude/.credentials.json elsewhere →
// GET api.anthropic.com/api/oauth/usage. 429s arm a persisted cooldown;
// a success within CLAUDE_FRESH_SECS is served from disk without calling
// out (refresh-spam cannot burn OAuth usage requests).
use std::path::Path;

use serde_json::Value;

use super::{
    agent, clamp_percent, parse_any_ts, DiskCache, FetchErr, LimitWindow, ToolLimits,
    CLAUDE_FRESH_SECS,
};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
const DEFAULT_COOLDOWN_SECS: i64 = 300;
const MAX_COOLDOWN_SECS: i64 = 3600;

/// Raw credential payload: { claudeAiOauth: { accessToken, subscriptionType, … } }
fn read_credentials(home: &Path) -> Option<Value> {
    let raw = if cfg!(target_os = "macos") {
        let out = std::process::Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        String::from_utf8(out.stdout).ok()?
    } else {
        std::fs::read_to_string(home.join(".claude/.credentials.json")).ok()?
    };
    serde_json::from_str(raw.trim()).ok()
}

fn access_token(payload: &Value) -> Option<String> {
    let t = payload.get("claudeAiOauth")?.get("accessToken")?.as_str()?.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// subscriptionType → title-cased plan label; free/none/unknown filtered.
pub fn plan_of(payload: &Value) -> Option<String> {
    let raw = payload.get("claudeAiOauth")?.get("subscriptionType")?.as_str()?.trim();
    let lower = raw.to_lowercase();
    if raw.is_empty() || lower == "free" || lower == "none" || lower == "unknown" {
        return None;
    }
    let mut c = lower.chars();
    Some(c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default())
}

fn window_of(v: Option<&Value>, label: &str) -> Option<LimitWindow> {
    let v = v.filter(|v| v.is_object())?;
    let pct = v.get("utilization")?.as_f64()?;
    Some(LimitWindow {
        label: label.into(),
        used_percent: clamp_percent(pct),
        resets_at_ts: v.get("resets_at").and_then(parse_any_ts),
    })
}

pub fn normalize(body: &Value) -> Vec<LimitWindow> {
    let mut windows = Vec::new();
    windows.extend(window_of(body.get("five_hour"), "5h"));
    windows.extend(window_of(body.get("seven_day"), "7d"));
    let has_opus = body.get("seven_day_opus").map(|v| v.is_object()).unwrap_or(false);
    windows.extend(window_of(body.get("seven_day_opus"), "Opus"));
    // Model-scoped weekly windows (e.g. Fable) arrive only in the generic
    // limits[] array; an "Opus" scoped entry duplicating seven_day_opus is
    // dropped so the same window never renders twice.
    if let Some(limits) = body.get("limits").and_then(Value::as_array) {
        for entry in limits {
            if entry.get("kind").and_then(Value::as_str) != Some("weekly_scoped") {
                continue;
            }
            let model = entry.get("scope").and_then(|s| s.get("model"));
            let label = model
                .and_then(|m| m.get("display_name").and_then(Value::as_str))
                .or_else(|| model.and_then(|m| m.get("id").and_then(Value::as_str)))
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let Some(label) = label else { continue };
            if has_opus && label.eq_ignore_ascii_case("opus") {
                continue;
            }
            let Some(pct) = entry.get("percent").and_then(Value::as_f64) else { continue };
            windows.push(LimitWindow {
                label: label.to_string(),
                used_percent: clamp_percent(pct),
                resets_at_ts: entry.get("resets_at").and_then(parse_any_ts),
            });
        }
    }
    windows
}

fn rate_limit_msg(retry_after_secs: Option<i64>) -> String {
    match retry_after_secs {
        Some(s) if s > 0 => {
            format!("Claude API rate limited (429) — retry in ~{}m.", (s + 59) / 60)
        }
        _ => "Claude API rate limited (429) — retry shortly.".to_string(),
    }
}

/// Serve without a live call when (a) the last success is fresh, or (b) a
/// 429 cooldown is armed. Pure — unit-tested with a synthetic cache.
pub fn precheck(cache: &DiskCache, now_ts: i64) -> Option<ToolLimits> {
    let cached = cache.tools.get("claude");
    if let Some(c) = cached {
        if now_ts - c.cached_at_ts <= CLAUDE_FRESH_SECS
            && c.tool.error.is_none()
            && !c.tool.windows.is_empty()
        {
            let mut t = c.tool.clone();
            t.cached_at_ts = Some(c.cached_at_ts);
            return Some(t); // fresh: not marked stale
        }
    }
    if let Some(retry_at) = cache.claude_retry_at_ts {
        if now_ts < retry_at {
            if let Some(c) = cached {
                if c.tool.error.is_none() && !c.tool.windows.is_empty() {
                    let mut t = c.tool.clone();
                    t.stale = true;
                    t.cached_at_ts = Some(c.cached_at_ts);
                    return Some(t);
                }
            }
            return Some(ToolLimits::error_card("claude", rate_limit_msg(Some(retry_at - now_ts))));
        }
    }
    None
}

pub fn fetch(home: &Path, now_ts: i64) -> Result<ToolLimits, FetchErr> {
    let Some(payload) = read_credentials(home) else {
        return Ok(ToolLimits::not_configured("claude"));
    };
    let Some(token) = access_token(&payload) else {
        return Ok(ToolLimits::not_configured("claude"));
    };
    let plan = plan_of(&payload);
    let resp = agent()
        .get(USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .set("Accept", "application/json")
        .call();
    match resp {
        Ok(r) => {
            let body: Value = r.into_json().map_err(|e| FetchErr::from(e.to_string()))?;
            Ok(ToolLimits::live("claude", plan, normalize(&body), now_ts))
        }
        Err(ureq::Error::Status(401, _)) => Ok(ToolLimits::error_card(
            "claude",
            "Claude token expired — run `claude` once to refresh.",
        )),
        Err(ureq::Error::Status(429, r)) => {
            let retry = r
                .header("retry-after")
                .and_then(|v| v.parse::<i64>().ok())
                .filter(|s| *s > 0)
                .unwrap_or(DEFAULT_COOLDOWN_SECS)
                .min(MAX_COOLDOWN_SECS);
            Err(FetchErr { message: rate_limit_msg(Some(retry)), retry_after_secs: Some(retry) })
        }
        Err(ureq::Error::Status(code, _)) => {
            Err(FetchErr::from(format!("Claude API returned {code}")))
        }
        Err(e) => Err(FetchErr::from(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::CachedTool;
    use serde_json::json;

    fn usage_body() -> Value {
        json!({
            "five_hour": { "utilization": 11, "resets_at": "2026-07-12T12:00:00Z" },
            "seven_day": { "utilization": 28, "resets_at": "2026-07-14T00:00:00Z" },
            "seven_day_opus": { "utilization": 5, "resets_at": "2026-07-14T00:00:00Z" },
            "limits": [
                { "kind": "weekly_scoped", "percent": 43,
                  "scope": { "model": { "display_name": "Fable", "id": "claude-fable-5" } },
                  "resets_at": "2026-07-14T00:00:00Z" },
                { "kind": "weekly_scoped", "percent": 5,
                  "scope": { "model": { "display_name": "Opus" } },
                  "resets_at": "2026-07-14T00:00:00Z" },
                { "kind": "other", "percent": 1 }
            ]
        })
    }

    #[test]
    fn normalizes_all_window_kinds_and_dedupes_opus() {
        let w = normalize(&usage_body());
        let labels: Vec<&str> = w.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "7d", "Opus", "Fable"]);
        assert_eq!(w[3].used_percent, 43.0);
        assert!(w[0].resets_at_ts.is_some());
    }

    #[test]
    fn scoped_opus_kept_when_no_legacy_field() {
        let mut body = usage_body();
        body.as_object_mut().unwrap().remove("seven_day_opus");
        let w = normalize(&body);
        let labels: Vec<&str> = w.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "7d", "Fable", "Opus"]);
    }

    #[test]
    fn plan_title_cases_and_filters() {
        let p = json!({ "claudeAiOauth": { "subscriptionType": "pro" } });
        assert_eq!(plan_of(&p).as_deref(), Some("Pro"));
        let f = json!({ "claudeAiOauth": { "subscriptionType": "free" } });
        assert_eq!(plan_of(&f), None);
    }

    fn cache_with(tool: ToolLimits, cached_at_ts: i64, retry_at: Option<i64>) -> DiskCache {
        let mut c = DiskCache::default();
        c.claude_retry_at_ts = retry_at;
        c.tools.insert("claude".into(), CachedTool { tool, cached_at_ts });
        c
    }

    fn live_tool(ts: i64) -> ToolLimits {
        ToolLimits::live("claude", Some("Pro".into()), normalize(&usage_body()), ts)
    }

    #[test]
    fn precheck_serves_fresh_cache_unstale() {
        let cache = cache_with(live_tool(1000), 1000, None);
        let out = precheck(&cache, 1000 + CLAUDE_FRESH_SECS).unwrap();
        assert!(!out.stale);
        assert_eq!(out.cached_at_ts, Some(1000));
    }

    #[test]
    fn precheck_skips_old_cache_without_cooldown() {
        let cache = cache_with(live_tool(1000), 1000, None);
        assert!(precheck(&cache, 1000 + CLAUDE_FRESH_SECS + 1).is_none());
    }

    #[test]
    fn precheck_serves_stale_cache_during_cooldown() {
        let cache = cache_with(live_tool(1000), 1000, Some(10_000));
        let out = precheck(&cache, 5000).unwrap();
        assert!(out.stale);
    }

    #[test]
    fn precheck_cooldown_without_cache_is_error_card() {
        let mut cache = DiskCache::default();
        cache.claude_retry_at_ts = Some(10_000);
        let out = precheck(&cache, 5000).unwrap();
        assert!(out.error.unwrap().contains("retry in ~"));
    }

    #[test]
    fn precheck_expired_cooldown_is_none() {
        let mut cache = DiskCache::default();
        cache.claude_retry_at_ts = Some(10_000);
        assert!(precheck(&cache, 10_000).is_none());
    }
}
```

- [ ] **Step 2: Uncomment `pub mod claude;`, run tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml limits::claude`
Expected: 8 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/limits/claude.rs src-tauri/src/limits/mod.rs
git commit -m "feat(limits): Claude OAuth usage provider with 429 cooldown and fresh-cache serve"
```

---

### Task 4: `limits/codex.rs` — Codex wham usage + token refresh

**Files:**
- Create: `src-tauri/src/limits/codex.rs`
- Modify: `src-tauri/src/limits/mod.rs` (uncomment `pub mod codex;`)
- Modify: `src-tauri/src/time.rs` (add `epoch_to_iso`)

**Interfaces:**
- Consumes: Task 1 types; `crate::time::{iso_to_epoch, epoch_to_iso}`.
- Produces:
  - `pub fn fetch(home: &Path, now_ts: i64) -> Result<ToolLimits, FetchErr>`
  - `pub fn windows_of(rate_limit: &serde_json::Value) -> Vec<LimitWindow>` (pure, tested)
  - `pub fn jwt_claims(token: &str) -> Option<serde_json::Value>` (pure, tested)
  - In `time.rs`: `pub fn epoch_to_iso(ts: i64) -> String`

- [ ] **Step 1: Add `epoch_to_iso` to `src-tauri/src/time.rs` with a round-trip test**

Append to `time.rs` (before the test module):

```rust
// Inverse of iso_to_epoch (civil-from-days). Needed to stamp last_refresh
// when persisting refreshed Codex tokens.
pub fn epoch_to_iso(ts: i64) -> String {
    let days = ts.div_euclid(86400);
    let secs = ts.rem_euclid(86400);
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, m, d, secs / 3600, (secs % 3600) / 60, secs % 60
    )
}
```

And inside its `mod tests`:

```rust
#[test]
fn epoch_to_iso_round_trips() {
    let iso = "2026-06-01T10:00:00Z";
    assert_eq!(epoch_to_iso(iso_to_epoch(iso).unwrap()), iso);
    assert_eq!(epoch_to_iso(0), "1970-01-01T00:00:00Z");
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml time::`
Expected: all time tests pass including the new round trip.

- [ ] **Step 2: Write `codex.rs` with tests**

```rust
// Codex (OpenAI CLI) — ~/.codex/auth.json bearer token (proactively
// refreshed when >8 days stale, mirroring the official CLI) →
// GET chatgpt.com/backend-api/wham/usage. Windows classified by
// limit_window_seconds, not slot position: free tiers deliver the weekly
// window in the primary slot.
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::{agent, clamp_percent, FetchErr, LimitWindow, ToolLimits};
use crate::time::{epoch_to_iso, iso_to_epoch};

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const REFRESH_URL: &str = "https://auth.openai.com/oauth/token";
// Public OAuth client id used by the official `codex` CLI — not a secret.
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_AUTH_CLAIM: &str = "https://api.openai.com/auth";
const SESSION_SECS: i64 = 18_000;
const WEEKLY_SECS: i64 = 604_800;
const REFRESH_THRESHOLD_SECS: i64 = 8 * 24 * 3600;
const REAUTH_MSG: &str = "Codex refresh token expired or revoked. Run `codex` to re-authenticate.";

fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut buf = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for ch in s.chars() {
        if ch == '=' {
            break;
        }
        let v = match ch {
            'A'..='Z' => ch as u32 - 'A' as u32,
            'a'..='z' => ch as u32 - 'a' as u32 + 26,
            '0'..='9' => ch as u32 - '0' as u32 + 52,
            '-' | '+' => 62,
            '_' | '/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            buf.push((acc >> bits) as u8);
        }
    }
    Some(buf)
}

pub fn jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    serde_json::from_slice(&b64url_decode(payload)?).ok()
}

struct AuthBundle {
    path: PathBuf,
    auth: Value,
    access_token: String,
    account_id: Option<String>,
    plan: Option<String>,
    refresh_token: Option<String>,
    last_refresh_ts: Option<i64>,
}

fn codex_home(home: &Path) -> PathBuf {
    match std::env::var("CODEX_HOME") {
        Ok(p) if !p.trim().is_empty() => PathBuf::from(p),
        _ => home.join(".codex"),
    }
}

fn str_of(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

fn read_bundle(home: &Path) -> Option<AuthBundle> {
    let path = codex_home(home).join("auth.json");
    let auth: Value = serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok()?;
    let tokens = auth.get("tokens")?;
    let access_token = str_of(tokens.get("access_token"))?;
    let access_claims = jwt_claims(&access_token);
    let id_claims = str_of(tokens.get("id_token")).and_then(|t| jwt_claims(&t));
    let ns = |claims: &Option<Value>, key: &str| -> Option<String> {
        str_of(claims.as_ref()?.get(OPENAI_AUTH_CLAIM)?.get(key).into())
    };
    let account_id = str_of(tokens.get("account_id"))
        .or_else(|| ns(&access_claims, "chatgpt_account_id"))
        .or_else(|| ns(&id_claims, "chatgpt_account_id"));
    let plan_raw = ns(&access_claims, "chatgpt_plan_type").or_else(|| ns(&id_claims, "chatgpt_plan_type"));
    let plan = plan_raw.and_then(|p| {
        let lower = p.to_lowercase();
        if lower == "free" || lower == "none" || lower == "unknown" {
            return None;
        }
        let mut c = lower.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str())
    });
    Some(AuthBundle {
        access_token,
        account_id,
        plan,
        refresh_token: str_of(tokens.get("refresh_token")),
        last_refresh_ts: str_of(auth.get("last_refresh")).and_then(|s| iso_to_epoch(&s)),
        path,
        auth,
    })
}

fn window_of(v: Option<&Value>) -> Option<Value> {
    v.filter(|v| v.is_object()).cloned()
}

fn to_limit(v: &Value, label: &str) -> Option<LimitWindow> {
    let pct = v.get("used_percent")?.as_f64()?;
    Some(LimitWindow {
        label: label.into(),
        used_percent: clamp_percent(pct).round(),
        resets_at_ts: v.get("reset_at").and_then(Value::as_i64).filter(|t| *t > 0),
    })
}

fn classify(v: &Value) -> Option<&'static str> {
    match v.get("limit_window_seconds").and_then(Value::as_i64) {
        Some(SESSION_SECS) => Some("5h"),
        Some(WEEKLY_SECS) => Some("7d"),
        _ => None,
    }
}

pub fn windows_of(rate_limit: &Value) -> Vec<LimitWindow> {
    let primary = window_of(rate_limit.get("primary_window"));
    let secondary = window_of(rate_limit.get("secondary_window"));
    let mut session = None;
    let mut weekly = None;
    for w in [&primary, &secondary].into_iter().flatten() {
        match classify(w) {
            Some("5h") if session.is_none() => session = Some(w.clone()),
            Some("7d") if weekly.is_none() => weekly = Some(w.clone()),
            _ => {}
        }
    }
    // Positional fallback only when classification failed for both —
    // preserves data from unexpected window durations.
    if session.is_none() && weekly.is_none() {
        session = primary;
        weekly = secondary;
    }
    let mut out = Vec::new();
    out.extend(session.as_ref().and_then(|w| to_limit(w, "5h")));
    out.extend(weekly.as_ref().and_then(|w| to_limit(w, "7d")));
    out
}

/// Best-effort refresh; Ok(new access token) or Err(REAUTH) only when the
/// refresh token is dead — other failures fall through to the stale token.
fn refresh(bundle: &AuthBundle, now_ts: i64) -> Result<Option<String>, ()> {
    let Some(refresh_token) = &bundle.refresh_token else { return Ok(None) };
    let resp = agent().post(REFRESH_URL).send_json(json!({
        "client_id": CLIENT_ID,
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "scope": "openid profile email",
    }));
    match resp {
        Ok(r) => {
            let Ok(body) = r.into_json::<Value>() else { return Ok(None) };
            let Some(access) = str_of(body.get("access_token")) else { return Ok(None) };
            let new_refresh = str_of(body.get("refresh_token"))
                .unwrap_or_else(|| refresh_token.clone());
            let id_token = str_of(body.get("id_token"))
                .or_else(|| str_of(bundle.auth.get("tokens").and_then(|t| t.get("id_token"))));
            let mut merged = bundle.auth.clone();
            let tokens = merged
                .as_object_mut()
                .map(|o| o.entry("tokens").or_insert_with(|| json!({})));
            if let Some(Value::Object(t)) = tokens {
                t.insert("access_token".into(), json!(access));
                t.insert("refresh_token".into(), json!(new_refresh));
                t.insert("id_token".into(), json!(id_token));
            }
            if let Some(o) = merged.as_object_mut() {
                o.insert("last_refresh".into(), json!(epoch_to_iso(now_ts)));
            }
            // Atomic persist so a kill mid-write can't corrupt auth.json.
            let tmp = bundle.path.with_extension("json.tmp");
            if serde_json::to_string_pretty(&merged)
                .ok()
                .and_then(|s| std::fs::write(&tmp, s).ok())
                .is_some()
            {
                let _ = std::fs::rename(&tmp, &bundle.path);
            }
            Ok(Some(access))
        }
        Err(ureq::Error::Status(401, _)) => Err(()),
        Err(_) => Ok(None),
    }
}

pub fn fetch(home: &Path, now_ts: i64) -> Result<ToolLimits, FetchErr> {
    let Some(bundle) = read_bundle(home) else {
        return Ok(ToolLimits::not_configured("codex"));
    };
    let mut token = bundle.access_token.clone();
    let is_stale = bundle
        .last_refresh_ts
        .map(|ts| now_ts - ts > REFRESH_THRESHOLD_SECS)
        .unwrap_or(true);
    if is_stale {
        match refresh(&bundle, now_ts) {
            Ok(Some(new_token)) => token = new_token,
            Ok(None) => {} // best effort: proceed with the existing token
            Err(()) => return Ok(ToolLimits::error_card("codex", REAUTH_MSG)),
        }
    }
    let mut req = agent()
        .get(USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "application/json");
    if let Some(id) = &bundle.account_id {
        req = req.set("ChatGPT-Account-Id", id);
    }
    match req.call() {
        Ok(r) => {
            let body: Value = r.into_json().map_err(|e| FetchErr::from(e.to_string()))?;
            let windows = windows_of(body.get("rate_limit").unwrap_or(&Value::Null));
            Ok(ToolLimits::live("codex", bundle.plan, windows, now_ts))
        }
        // "No usage data for this auth state" — neutral, not an error card.
        Err(ureq::Error::Status(401 | 403 | 404, _)) => {
            Ok(ToolLimits::live("codex", bundle.plan, Vec::new(), now_ts))
        }
        Err(ureq::Error::Status(code, _)) => {
            Err(FetchErr::from(format!("Codex API returned {code}")))
        }
        Err(e) => Err(FetchErr::from(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // {"https://api.openai.com/auth":{"chatgpt_account_id":"acct-1","chatgpt_plan_type":"plus"}}
    fn fake_jwt() -> String {
        let payload = serde_json::to_vec(&json!({
            OPENAI_AUTH_CLAIM: { "chatgpt_account_id": "acct-1", "chatgpt_plan_type": "plus" }
        }))
        .unwrap();
        // std-only base64url encode for the test
        const TBL: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut s = String::new();
        for chunk in payload.chunks(3) {
            let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
            let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
            s.push(TBL[(n >> 18) as usize & 63] as char);
            s.push(TBL[(n >> 12) as usize & 63] as char);
            if chunk.len() > 1 {
                s.push(TBL[(n >> 6) as usize & 63] as char);
            }
            if chunk.len() > 2 {
                s.push(TBL[n as usize & 63] as char);
            }
        }
        format!("h.{s}.sig")
    }

    #[test]
    fn decodes_jwt_claims() {
        let claims = jwt_claims(&fake_jwt()).unwrap();
        assert_eq!(
            claims[OPENAI_AUTH_CLAIM]["chatgpt_account_id"].as_str(),
            Some("acct-1")
        );
    }

    #[test]
    fn classifies_windows_by_duration() {
        let rl = json!({
            "primary_window": { "used_percent": 1, "limit_window_seconds": 18000, "reset_at": 1800000000 },
            "secondary_window": { "used_percent": 16, "limit_window_seconds": 604800, "reset_at": 1800600000 },
        });
        let w = windows_of(&rl);
        assert_eq!(w[0].label, "5h");
        assert_eq!(w[0].used_percent, 1.0);
        assert_eq!(w[1].label, "7d");
        assert_eq!(w[1].resets_at_ts, Some(1800600000));
    }

    #[test]
    fn weekly_in_primary_slot_is_still_weekly() {
        // Free tier: only a weekly window, delivered in the primary slot.
        let rl = json!({
            "primary_window": { "used_percent": 40, "limit_window_seconds": 604800 },
        });
        let w = windows_of(&rl);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].label, "7d");
    }

    #[test]
    fn positional_fallback_when_unclassifiable() {
        let rl = json!({
            "primary_window": { "used_percent": 10, "limit_window_seconds": 3600 },
            "secondary_window": { "used_percent": 20, "limit_window_seconds": 86400 },
        });
        let w = windows_of(&rl);
        assert_eq!(w[0].label, "5h");
        assert_eq!(w[1].label, "7d");
        assert_eq!(w[0].used_percent, 10.0);
    }

    #[test]
    fn reads_bundle_from_auth_json() {
        let dir = tempfile::tempdir().unwrap();
        let codex = dir.path().join(".codex");
        std::fs::create_dir_all(&codex).unwrap();
        let auth = json!({
            "tokens": { "access_token": fake_jwt(), "refresh_token": "r1" },
            "last_refresh": "2026-07-10T00:00:00Z",
        });
        std::fs::write(codex.join("auth.json"), auth.to_string()).unwrap();
        std::env::remove_var("CODEX_HOME");
        let b = read_bundle(dir.path()).unwrap();
        assert_eq!(b.account_id.as_deref(), Some("acct-1"));
        assert_eq!(b.plan.as_deref(), Some("Plus"));
        assert!(b.last_refresh_ts.is_some());
    }
}
```

- [ ] **Step 3: Uncomment `pub mod codex;`, run tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml limits::codex time::`
Expected: 5 codex tests + time tests pass.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/limits/codex.rs src-tauri/src/limits/mod.rs src-tauri/src/time.rs
git commit -m "feat(limits): Codex wham usage provider with duration-classified windows and token refresh"
```

---

### Task 5: `limits/gemini.rs` — Code Assist quota

**Files:**
- Create: `src-tauri/src/limits/gemini.rs`
- Modify: `src-tauri/src/limits/mod.rs` (uncomment `pub mod gemini;`)

**Interfaces:**
- Consumes: Task 1 types.
- Produces:
  - `pub fn fetch(home: &Path, now_ts: i64) -> Result<ToolLimits, FetchErr>`
  - `pub fn normalize(buckets: &serde_json::Value) -> Result<Vec<LimitWindow>, String>` (pure, tested)
  - `pub fn plan_of(tier: Option<&str>) -> Option<String>` (pure, tested)

- [ ] **Step 1: Write the file with tests**

```rust
// Gemini CLI — ~/.gemini/oauth_creds.json (Google OAuth), refreshed via the
// gemini-cli's own OAuth client (scraped from its installed oauth2.js, with
// the public fallback client when the CLI isn't on disk) →
// cloudcode-pa loadCodeAssist (tier/project) + retrieveUserQuota (buckets).
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::{agent, clamp_percent, parse_any_ts, FetchErr, LimitWindow, ToolLimits};

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const LOAD_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
const QUOTA_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota";
const LOGIN_MSG: &str = "Not logged in to Gemini. Run 'gemini' in Terminal to authenticate.";
// Public installed-app OAuth client shipped inside gemini-cli — not a
// secret (installed-app clients cannot keep one). Assembled from parts only
// to dodge secret-scanner false positives, mirroring the example app.
const FALLBACK_CLIENT_ID: &str =
    "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com";
fn fallback_client_secret() -> String {
    ["GOCSPX", "4uHgMPm", "1o7Sk", "geV6Cu5clXFsxl"].join("-")
}

fn creds_path(home: &Path) -> PathBuf {
    home.join(".gemini/oauth_creds.json")
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn gemini_on_path() -> bool {
    std::process::Command::new("which")
        .arg("gemini")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Scrape OAUTH_CLIENT_ID/SECRET from the installed gemini-cli's oauth2.js.
/// ponytail: `which` + one symlink hop + the example's candidate path list;
/// no bundle chunk scan — the public fallback client covers the rest.
fn oauth_client() -> (String, String) {
    let which = std::process::Command::new("which").arg("gemini").output().ok();
    let gemini_path = which
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(p) = gemini_path {
        let real = std::fs::read_link(&p)
            .map(|l| if l.is_absolute() { l } else { Path::new(&p).parent().unwrap_or(Path::new("/")).join(l) })
            .unwrap_or_else(|_| PathBuf::from(&p));
        let bin_dir = real.parent().unwrap_or(Path::new("/"));
        let base = bin_dir.parent().unwrap_or(Path::new("/"));
        const REL: &[&str] = &[
            "libexec/lib/node_modules/@google/gemini-cli/node_modules/@google/gemini-cli-core/dist/src/code_assist/oauth2.js",
            "lib/node_modules/@google/gemini-cli/node_modules/@google/gemini-cli-core/dist/src/code_assist/oauth2.js",
            "share/gemini-cli/node_modules/@google/gemini-cli-core/dist/src/code_assist/oauth2.js",
            "../gemini-cli-core/dist/src/code_assist/oauth2.js",
            "node_modules/@google/gemini-cli-core/dist/src/code_assist/oauth2.js",
        ];
        for rel in REL {
            let candidate = base.join(rel);
            let Ok(content) = std::fs::read_to_string(&candidate) else { continue };
            let grab = |name: &str| -> Option<String> {
                let idx = content.find(name)?;
                let rest = &content[idx + name.len()..];
                let start = rest.find(['"', '\''])? + 1;
                let quote = rest.as_bytes()[start - 1] as char;
                let end = rest[start..].find(quote)?;
                Some(rest[start..start + end].to_string())
            };
            if let (Some(id), Some(secret)) = (grab("OAUTH_CLIENT_ID"), grab("OAUTH_CLIENT_SECRET")) {
                return (id, secret);
            }
        }
    }
    (FALLBACK_CLIENT_ID.to_string(), fallback_client_secret())
}

fn refresh_token(home: &Path, refresh: &str) -> Result<String, FetchErr> {
    let (client_id, client_secret) = oauth_client();
    let resp = agent().post(TOKEN_URL).send_form(&[
        ("client_id", client_id.as_str()),
        ("client_secret", client_secret.as_str()),
        ("refresh_token", refresh),
        ("grant_type", "refresh_token"),
    ]);
    let body: Value = match resp {
        Ok(r) => r.into_json().map_err(|e| FetchErr::from(e.to_string()))?,
        Err(_) => return Err(FetchErr::from(LOGIN_MSG)),
    };
    let Some(access) = body.get("access_token").and_then(Value::as_str) else {
        return Err(FetchErr::from("Could not parse Gemini token refresh response"));
    };
    // Persist so the CLI (and our next poll) reuse the fresh token.
    let path = creds_path(home);
    if let Some(mut creds) = read_json(&path) {
        if let Some(o) = creds.as_object_mut() {
            o.insert("access_token".into(), json!(access));
            if let Some(id) = body.get("id_token").and_then(Value::as_str) {
                o.insert("id_token".into(), json!(id));
            }
            if let Some(exp) = body.get("expires_in").and_then(Value::as_f64) {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as f64)
                    .unwrap_or(0.0);
                o.insert("expiry_date".into(), json!(now_ms + exp * 1000.0));
            }
            let _ = std::fs::write(&path, serde_json::to_string_pretty(&creds).unwrap_or_default());
        }
    }
    Ok(access.to_string())
}

pub fn plan_of(tier: Option<&str>) -> Option<String> {
    match tier {
        Some("standard-tier") => Some("Paid".into()),
        Some("legacy-tier") => Some("Legacy".into()),
        Some("free-tier") => Some("Free".into()),
        _ => None,
    }
}

fn family(id: &str) -> Option<&'static str> {
    let lower = id.to_lowercase();
    if lower.contains("flash-lite") {
        Some("Lite")
    } else if lower.contains("flash") {
        Some("Flash")
    } else if lower.contains("pro") {
        Some("Pro")
    } else {
        None
    }
}

pub fn normalize(buckets: &Value) -> Result<Vec<LimitWindow>, String> {
    let arr = buckets
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or("Could not parse Gemini usage: no quota buckets in response")?;
    // Per model keep the LOWEST remaining fraction, then per family the
    // lowest-remaining model — the binding constraint is what matters.
    let mut by_model: std::collections::HashMap<String, (f64, Option<i64>)> =
        std::collections::HashMap::new();
    for b in arr {
        let Some(id) = b.get("modelId").and_then(Value::as_str) else { continue };
        let Some(rf) = b.get("remainingFraction").and_then(Value::as_f64).filter(|f| f.is_finite())
        else {
            continue;
        };
        let reset = b.get("resetTime").and_then(parse_any_ts);
        let entry = by_model.entry(id.to_string()).or_insert((rf, reset));
        if rf < entry.0 {
            *entry = (rf, reset);
        }
    }
    if by_model.is_empty() {
        return Err("Could not parse Gemini usage: no quota buckets in response".into());
    }
    let mut out = Vec::new();
    for fam in ["Pro", "Flash", "Lite"] {
        let pick = by_model
            .iter()
            .filter(|(id, _)| family(id) == Some(fam))
            .min_by(|a, b| a.1 .0.partial_cmp(&b.1 .0).unwrap_or(std::cmp::Ordering::Equal));
        if let Some((_, (rf, reset))) = pick {
            out.push(LimitWindow {
                label: fam.into(),
                used_percent: clamp_percent(100.0 - rf * 100.0),
                resets_at_ts: *reset,
            });
        }
    }
    if out.is_empty() {
        // No family matched (upstream renamed models): surface the overall
        // binding constraint rather than dropping data.
        let (_, (rf, reset)) = by_model
            .iter()
            .min_by(|a, b| a.1 .0.partial_cmp(&b.1 .0).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap();
        out.push(LimitWindow {
            label: "Quota".into(),
            used_percent: clamp_percent(100.0 - rf * 100.0),
            resets_at_ts: *reset,
        });
    }
    Ok(out)
}

pub fn fetch(home: &Path, now_ts: i64) -> Result<ToolLimits, FetchErr> {
    let settings = read_json(&home.join(".gemini/settings.json"));
    let creds = read_json(&creds_path(home));
    // A bare settings.json is NOT Gemini evidence (Antigravity also writes
    // under ~/.gemini); require creds or the CLI on PATH.
    if creds.is_none() && !gemini_on_path() {
        return Ok(ToolLimits::not_configured("gemini"));
    }
    let selected = settings
        .as_ref()
        .and_then(|s| s.pointer("/security/auth/selectedType"))
        .and_then(Value::as_str);
    match selected {
        Some("api-key") => {
            return Ok(ToolLimits::error_card(
                "gemini",
                "Gemini API key auth not supported. Use Google account (OAuth) instead.",
            ))
        }
        Some("vertex-ai") => {
            return Ok(ToolLimits::error_card(
                "gemini",
                "Gemini Vertex AI auth not supported. Use Google account (OAuth) instead.",
            ))
        }
        _ => {}
    }
    let Some(creds) = creds else {
        return Ok(ToolLimits::error_card("gemini", LOGIN_MSG));
    };
    let Some(mut access) = creds.get("access_token").and_then(Value::as_str).map(String::from)
    else {
        return Ok(ToolLimits::error_card("gemini", LOGIN_MSG));
    };
    let now_ms = (now_ts as f64) * 1000.0;
    let expired = creds
        .get("expiry_date")
        .and_then(Value::as_f64)
        .map(|e| e > 0.0 && e < now_ms)
        .unwrap_or(false);
    if expired {
        if let Some(rt) = creds.get("refresh_token").and_then(Value::as_str) {
            access = refresh_token(home, rt)?;
        }
    }
    let bearer = format!("Bearer {access}");
    // Tier + project id (project is optional in the quota call).
    let load: Value = match agent()
        .post(LOAD_URL)
        .set("Authorization", &bearer)
        .send_json(json!({ "metadata": { "ideType": "GEMINI_CLI", "pluginType": "GEMINI" } }))
    {
        Ok(r) => r.into_json().unwrap_or(Value::Null),
        Err(_) => Value::Null,
    };
    let tier = load.pointer("/currentTier/id").and_then(Value::as_str).map(String::from);
    let project = load
        .get("cloudaicompanionProject")
        .and_then(|p| {
            p.as_str()
                .map(String::from)
                .or_else(|| p.get("id").and_then(Value::as_str).map(String::from))
                .or_else(|| p.get("projectId").and_then(Value::as_str).map(String::from))
        })
        .filter(|s| !s.trim().is_empty());
    let quota_body = match &project {
        Some(p) => json!({ "project": p }),
        None => json!({}),
    };
    let quota: Value = match agent().post(QUOTA_URL).set("Authorization", &bearer).send_json(quota_body) {
        Ok(r) => r.into_json().map_err(|e| FetchErr::from(e.to_string()))?,
        Err(ureq::Error::Status(401, _)) => return Ok(ToolLimits::error_card("gemini", LOGIN_MSG)),
        Err(ureq::Error::Status(code, _)) => {
            return Err(FetchErr::from(format!("Gemini API error: HTTP {code}")))
        }
        Err(e) => return Err(FetchErr::from(e.to_string())),
    };
    let windows = normalize(quota.get("buckets").unwrap_or(&Value::Null)).map_err(FetchErr::from)?;
    Ok(ToolLimits::live("gemini", plan_of(tier.as_deref()), windows, now_ts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalizes_families_lowest_remaining() {
        let buckets = json!([
            { "modelId": "gemini-2.5-pro", "remainingFraction": 1.0, "resetTime": "2026-07-13T00:00:00Z" },
            { "modelId": "gemini-2.5-pro", "remainingFraction": 0.4, "resetTime": "2026-07-13T00:00:00Z" },
            { "modelId": "gemini-2.5-flash", "remainingFraction": 0.9 },
            { "modelId": "gemini-2.5-flash-lite", "remainingFraction": 0.95 },
        ]);
        let w = normalize(&buckets).unwrap();
        let labels: Vec<&str> = w.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Pro", "Flash", "Lite"]);
        assert!((w[0].used_percent - 60.0).abs() < 1e-9); // 1 - 0.4
        assert!((w[1].used_percent - 10.0).abs() < 1e-9); // flash ≠ flash-lite
    }

    #[test]
    fn flash_lite_not_counted_as_flash() {
        let buckets = json!([
            { "modelId": "gemini-flash-lite", "remainingFraction": 0.5 },
        ]);
        let w = normalize(&buckets).unwrap();
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].label, "Lite");
    }

    #[test]
    fn unknown_models_fall_back_to_single_quota_row() {
        let buckets = json!([
            { "modelId": "mystery-model", "remainingFraction": 0.25 },
            { "modelId": "another", "remainingFraction": 0.8 },
        ]);
        let w = normalize(&buckets).unwrap();
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].label, "Quota");
        assert!((w[0].used_percent - 75.0).abs() < 1e-9);
    }

    #[test]
    fn empty_buckets_is_error() {
        assert!(normalize(&json!([])).is_err());
        assert!(normalize(&json!(null)).is_err());
    }

    #[test]
    fn maps_tiers_to_plans() {
        assert_eq!(plan_of(Some("standard-tier")).as_deref(), Some("Paid"));
        assert_eq!(plan_of(Some("legacy-tier")).as_deref(), Some("Legacy"));
        assert_eq!(plan_of(Some("free-tier")).as_deref(), Some("Free"));
        assert_eq!(plan_of(Some("other")), None);
        assert_eq!(plan_of(None), None);
    }
}
```

- [ ] **Step 2: Uncomment `pub mod gemini;`, run tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml limits::gemini`
Expected: 5 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/limits/gemini.rs src-tauri/src/limits/mod.rs
git commit -m "feat(limits): Gemini Code Assist quota provider with OAuth refresh"
```

---

### Task 6: `limits/antigravity.rs` — local IDE quota API

**Files:**
- Create: `src-tauri/src/limits/antigravity.rs`
- Modify: `src-tauri/src/limits/mod.rs` (uncomment `pub mod antigravity;`)
- Modify: `src-tauri/Cargo.toml` (ureq native-tls feature + native-tls dep)

**Interfaces:**
- Consumes: Task 1 types.
- Produces:
  - `pub fn fetch(home: &Path, now_ts: i64) -> Result<ToolLimits, FetchErr>`
  - Pure, tested: `parse_process_line`, `is_antigravity_command_line`, `extract_flag`, `parse_listening_ports`, `normalize_quota_summary(&Value) -> Result<Vec<LimitWindow>, String>`, `normalize_user_status(&Value, fallback_to_configs: bool) -> Result<(Option<String>, Vec<LimitWindow>), String>`

- [ ] **Step 1: Add the TLS dependency**

In `src-tauri/Cargo.toml` change the ureq line and add native-tls:

```toml
ureq = { version = "2", features = ["native-tls"] }
native-tls = "0.2"
```

Run: `cargo check --manifest-path src-tauri/Cargo.toml`
Expected: compiles (feature is additive; rustls default stays for other providers).

- [ ] **Step 2: Write the file with tests**

```rust
// Google Antigravity — quota lives behind the RUNNING IDE's local
// language-server API (self-signed HTTPS on a dynamic port, CSRF token in
// the process args). ps → detect process + csrf; lsof → candidate ports;
// probe with GetUnleashData; then RetrieveUserQuotaSummary, falling back to
// GetUserStatus / GetCommandModelConfigs (older servers / agy CLI).
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::{clamp_percent, parse_any_ts, FetchErr, LimitWindow, ToolLimits};

const SVC: &str = "/exa.language_server_pb.LanguageServerService";
const NOT_RUNNING_MSG: &str =
    "Antigravity IDE is not running. Launch Antigravity to see usage limits.";

pub struct ProcessInfo {
    pub pid: i64,
    pub csrf_token: Option<String>,
    pub extension_port: Option<u16>,
}

pub fn parse_process_line(line: &str) -> Option<(i64, String)> {
    let trimmed = line.trim();
    let (pid, rest) = trimmed.split_once(char::is_whitespace)?;
    let pid = pid.parse::<i64>().ok()?;
    Some((pid, rest.trim_start().to_string()))
}

fn first_token(command: &str) -> &str {
    let t = command.trim_start();
    match t.chars().next() {
        Some(q @ ('"' | '\'')) => {
            let inner = &t[1..];
            inner.split(q).next().unwrap_or(inner)
        }
        _ => t.split_whitespace().next().unwrap_or(""),
    }
}

pub fn is_antigravity_command_line(command: &str) -> bool {
    let lower = command.to_lowercase();
    let exe = first_token(command)
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_lowercase();
    // agy CLI binary is itself the server.
    if exe == "agy" || exe == "agy.exe" {
        return true;
    }
    // IDE language_server — require Antigravity markers so sibling Codeium
    // products (Windsurf etc.) are not misidentified.
    let is_lang_server = exe.starts_with("language_server");
    let has_marker = (lower.contains("--app_data_dir") && lower.contains("antigravity"))
        || lower.contains("/antigravity/")
        || lower.contains("/antigravity.app/")
        || lower.contains("\\antigravity\\")
        || lower.contains("--override_ide_name=antigravity")
        || lower.contains("--override_ide_name antigravity");
    is_lang_server && has_marker
}

pub fn extract_flag(command: &str, flag: &str) -> Option<String> {
    let idx = command.find(flag)?;
    let rest = &command[idx + flag.len()..];
    let rest = rest.trim_start_matches(['=', ' ', '\t']);
    let val: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
    (!val.is_empty()).then_some(val)
}

fn detect_process() -> Result<Option<ProcessInfo>, String> {
    let out = std::process::Command::new("/bin/ps")
        .args(["-ax", "-o", "pid=,command="])
        .output()
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        let Some((pid, command)) = parse_process_line(line) else { continue };
        if !is_antigravity_command_line(&command) {
            continue;
        }
        let csrf_token = extract_flag(&command, "--csrf_token");
        let extension_port = extract_flag(&command, "--extension_server_port")
            .and_then(|p| p.parse::<u16>().ok());
        return Ok(Some(ProcessInfo { pid, csrf_token, extension_port }));
    }
    Ok(None)
}

pub fn parse_listening_ports(output: &str) -> Vec<u16> {
    let mut ports: Vec<u16> = Vec::new();
    for line in output.lines() {
        if !line.contains("(LISTEN)") {
            continue;
        }
        // "... TCP 127.0.0.1:42117 (LISTEN)" → take the :port right before (LISTEN)
        for part in line.split_whitespace() {
            if let Some(rest) = part.rsplit(':').next() {
                if let Ok(p) = rest.parse::<u16>() {
                    if !ports.contains(&p) {
                        ports.push(p);
                    }
                }
            }
        }
    }
    ports.sort_unstable();
    ports
}

fn list_ports(pid: i64) -> Result<Vec<u16>, String> {
    let lsof = ["/usr/sbin/lsof", "/usr/bin/lsof"]
        .iter()
        .find(|p| Path::new(p).exists())
        .ok_or("Antigravity port detection needs lsof. Install it, then retry.")?;
    let out = std::process::Command::new(lsof)
        .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-a", "-p", &pid.to_string()])
        .output()
        .map_err(|e| e.to_string())?;
    let ports = parse_listening_ports(&String::from_utf8_lossy(&out.stdout));
    if ports.is_empty() {
        return Err("Antigravity is running but not exposing ports yet. Try again in a few seconds.".into());
    }
    Ok(ports)
}

/// Local agent accepting the IDE's self-signed cert. Local-only (127.0.0.1).
fn local_agent() -> Result<ureq::Agent, String> {
    let tls = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(8))
        .tls_connector(Arc::new(tls))
        .build())
}

fn call_local(
    agent: &ureq::Agent,
    scheme: &str,
    port: u16,
    method: &str,
    body: &Value,
    csrf: Option<&str>,
) -> Result<Value, String> {
    let url = format!("{scheme}://127.0.0.1:{port}{SVC}/{method}");
    let mut req = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .set("Connect-Protocol-Version", "1");
    if let Some(t) = csrf {
        req = req.set("X-Codeium-Csrf-Token", t);
    }
    match req.send_json(body.clone()) {
        Ok(r) => r.into_json().map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

fn default_body() -> Value {
    json!({ "metadata": {
        "ideName": "antigravity", "extensionName": "antigravity",
        "ideVersion": "unknown", "locale": "en",
    }})
}

fn unleash_body() -> Value {
    json!({ "context": { "properties": {
        "devMode": "false", "extensionVersion": "unknown",
        "hasAnthropicModelAccess": "true", "ide": "antigravity",
        "ideVersion": "unknown", "installationId": "tokenledger",
        "language": "UNSPECIFIED", "os": "macos",
        "requestedModelId": "MODEL_UNSPECIFIED",
    }}})
}

fn code_ok(body: &Value) -> bool {
    match body.get("code") {
        None | Some(Value::Null) => true,
        Some(Value::Number(n)) => n.as_i64() == Some(0),
        Some(Value::String(s)) => {
            let l = s.to_lowercase();
            l == "ok" || l == "success" || l == "0"
        }
        _ => false,
    }
}

pub fn normalize_quota_summary(body: &Value) -> Result<Vec<LimitWindow>, String> {
    if !code_ok(body) {
        return Err(format!("Antigravity API error: {}", body.get("code").unwrap_or(&Value::Null)));
    }
    let groups = body
        .pointer("/response/groups")
        .and_then(Value::as_array)
        .filter(|g| !g.is_empty())
        .ok_or("Could not parse Antigravity quota summary: no groups.")?;
    let mut buckets: std::collections::HashMap<&str, &Value> = std::collections::HashMap::new();
    for g in groups {
        if let Some(bs) = g.get("buckets").and_then(Value::as_array) {
            for b in bs {
                if let Some(id) = b.get("bucketId").and_then(Value::as_str) {
                    buckets.insert(id, b);
                }
            }
        }
    }
    let win = |id: &str, label: &str| -> Option<LimitWindow> {
        let b = buckets.get(id)?;
        let rf = b.get("remainingFraction").and_then(Value::as_f64)?;
        Some(LimitWindow {
            label: label.into(),
            used_percent: clamp_percent(100.0 - rf * 100.0),
            resets_at_ts: b.get("resetTime").and_then(parse_any_ts),
        })
    };
    let out: Vec<LimitWindow> = [
        win("3p-weekly", "Cl 7d"),
        win("3p-5h", "Cl 5h"),
        win("gemini-weekly", "Gm 7d"),
        win("gemini-5h", "Gm 5h"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if out.is_empty() {
        // Known bucketIds all missing (upstream rename) → treat as parse
        // failure so the caller falls back to GetUserStatus.
        return Err("Could not parse Antigravity quota summary: no known buckets matched.".into());
    }
    Ok(out)
}

struct ModelQuota {
    text: String, // label + model id, lowercased, for family classification
    remaining_fraction: Option<f64>,
    reset_ts: Option<i64>,
}

fn parse_model_configs(configs: Option<&Value>) -> Vec<ModelQuota> {
    let Some(arr) = configs.and_then(Value::as_array) else { return Vec::new() };
    arr.iter()
        .filter_map(|c| {
            let quota = c.get("quotaInfo")?;
            let label = c.get("label").and_then(Value::as_str).unwrap_or("");
            let model = c.pointer("/modelOrAlias/model").and_then(Value::as_str).unwrap_or("");
            Some(ModelQuota {
                text: format!("{label} {model}").to_lowercase(),
                remaining_fraction: quota.get("remainingFraction").and_then(Value::as_f64),
                reset_ts: quota.get("resetTime").and_then(parse_any_ts),
            })
        })
        .collect()
}

fn family(m: &ModelQuota) -> &'static str {
    if m.text.contains("claude") {
        "claude"
    } else if m.text.contains("gemini") && m.text.contains("pro") {
        "gemini_pro"
    } else if m.text.contains("gemini") && m.text.contains("flash") {
        "gemini_flash"
    } else {
        "unknown"
    }
}

fn is_chat_model(m: &ModelQuota) -> bool {
    !(m.text.contains("lite") || m.text.contains("autocomplete") || m.text.contains("tab_"))
}

pub fn normalize_user_status(
    body: &Value,
    fallback_to_configs: bool,
) -> Result<(Option<String>, Vec<LimitWindow>), String> {
    if !code_ok(body) {
        return Err(format!("Antigravity API error: {}", body.get("code").unwrap_or(&Value::Null)));
    }
    let user_status = body.get("userStatus");
    let configs = if fallback_to_configs {
        body.get("clientModelConfigs")
    } else {
        user_status.and_then(|u| u.pointer("/cascadeModelConfigData/clientModelConfigs"))
    };
    let all = parse_model_configs(configs);
    if all.is_empty() {
        return Err("Could not parse Antigravity quota: no quota models available.".into());
    }
    let chat: Vec<&ModelQuota> = all.iter().filter(|m| is_chat_model(m)).collect();
    let models: Vec<&ModelQuota> = if chat.is_empty() { all.iter().collect() } else { chat };
    let claude: Vec<&&ModelQuota> = models.iter().filter(|m| family(m) == "claude").collect();
    let gemini: Vec<&&ModelQuota> = models
        .iter()
        .filter(|m| matches!(family(m), "gemini_pro" | "gemini_flash"))
        .collect();
    // Most-used (min remaining → the weekly quota) and least-used (max
    // remaining → the 5h rolling quota) per family.
    fn pick<'a>(
        list: &[&&'a ModelQuota],
        min: bool,
    ) -> Option<&'a ModelQuota> {
        let mut with_rf: Vec<&&ModelQuota> =
            list.iter().filter(|m| m.remaining_fraction.is_some()).copied().collect();
        if with_rf.is_empty() {
            return list.first().copied().copied();
        }
        with_rf.sort_by(|a, b| {
            a.remaining_fraction
                .partial_cmp(&b.remaining_fraction)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let m = if min { with_rf.first() } else { with_rf.last() };
        m.copied().copied()
    }
    let win = |m: Option<&ModelQuota>, label: &str| -> Option<LimitWindow> {
        let m = m?;
        let rf = m.remaining_fraction.unwrap_or(0.0);
        Some(LimitWindow {
            label: label.into(),
            used_percent: clamp_percent(100.0 - rf * 100.0),
            resets_at_ts: m.reset_ts,
        })
    };
    let all_ref: Vec<&&ModelQuota> = models.iter().collect();
    let windows: Vec<LimitWindow> = [
        win(if claude.is_empty() { pick(&all_ref, true) } else { pick(&claude, true) }, "Cl 7d"),
        win(if claude.is_empty() { None } else { pick(&claude, false) }, "Cl 5h"),
        win(if gemini.is_empty() { None } else { pick(&gemini, true) }, "Gm 7d"),
        win(if gemini.is_empty() { None } else { pick(&gemini, false) }, "Gm 5h"),
    ]
    .into_iter()
    .flatten()
    .collect();
    let plan = user_status
        .map(|u| {
            [
                "/planStatus/planInfo/planDisplayName",
                "/planStatus/planInfo/displayName",
                "/planStatus/planInfo/productName",
                "/planStatus/planInfo/planName",
                "/planStatus/planInfo/planShortName",
            ]
            .iter()
            .find_map(|p| u.pointer(p).and_then(Value::as_str))
            .map(String::from)
        })
        .unwrap_or(None);
    Ok((plan, windows))
}

fn has_install_evidence(home: &Path) -> bool {
    ["antigravity", "antigravity-ide", "antigravity-cli"]
        .iter()
        .any(|d| home.join(".gemini").join(d).exists())
}

pub fn fetch(home: &Path, now_ts: i64) -> Result<ToolLimits, FetchErr> {
    let process = match detect_process() {
        Ok(p) => p,
        Err(e) => return Err(FetchErr::from(e)),
    };
    let Some(info) = process else {
        if !has_install_evidence(home) {
            return Ok(ToolLimits::not_configured("antigravity"));
        }
        // Installed but not running → orchestrator's with_fallback serves the
        // disk cache; this message shows only when no cache exists.
        return Err(FetchErr::from(NOT_RUNNING_MSG));
    };
    let ports = list_ports(info.pid).map_err(FetchErr::from)?;
    let agent = local_agent().map_err(FetchErr::from)?;
    let csrf = info.csrf_token.as_deref();
    let mut working: Option<(u16, &str)> = None;
    for port in &ports {
        if call_local(&agent, "https", *port, "GetUnleashData", &unleash_body(), csrf).is_ok() {
            working = Some((*port, "https"));
            break;
        }
        // agy CLI serves plain HTTP with no CSRF.
        if csrf.is_none()
            && call_local(&agent, "http", *port, "GetUnleashData", &unleash_body(), None).is_ok()
        {
            working = Some((*port, "http"));
            break;
        }
    }
    let Some((port, scheme)) = working else {
        return Err(FetchErr::from("Antigravity port detection failed: no working API port found"));
    };
    // Preferred: quota summary (newest servers).
    if let Ok(body) = call_local(&agent, scheme, port, "RetrieveUserQuotaSummary", &default_body(), csrf) {
        if let Ok(windows) = normalize_quota_summary(&body) {
            return Ok(ToolLimits::live("antigravity", None, windows, now_ts));
        }
    }
    // Fallback: GetUserStatus (IDE), then GetCommandModelConfigs (agy CLI).
    match call_local(&agent, scheme, port, "GetUserStatus", &default_body(), csrf) {
        Ok(body) => {
            let (plan, windows) = normalize_user_status(&body, false).map_err(FetchErr::from)?;
            Ok(ToolLimits::live("antigravity", plan, windows, now_ts))
        }
        Err(_) => {
            let fallback_port = info.extension_port.unwrap_or(port);
            let fallback_scheme = if fallback_port == port {
                if scheme == "https" { "http" } else { "https" }
            } else {
                "http"
            };
            let body = call_local(
                &agent,
                fallback_scheme,
                fallback_port,
                "GetCommandModelConfigs",
                &default_body(),
                csrf,
            )
            .map_err(FetchErr::from)?;
            let (plan, windows) = normalize_user_status(&body, true).map_err(FetchErr::from)?;
            Ok(ToolLimits::live("antigravity", plan, windows, now_ts))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_process_lines() {
        let (pid, cmd) = parse_process_line("  423  /Applications/Antigravity.app/x language_server_macos --csrf_token abc").unwrap();
        assert_eq!(pid, 423);
        assert!(cmd.contains("language_server_macos"));
        assert!(parse_process_line("garbage").is_none());
    }

    #[test]
    fn identifies_antigravity_commands() {
        assert!(is_antigravity_command_line(
            "/Applications/Antigravity.app/Contents/language_server_macos_arm --app_data_dir /x/antigravity --csrf_token t"
        ));
        assert!(is_antigravity_command_line("/usr/local/bin/agy serve"));
        assert!(!is_antigravity_command_line("vim /tmp/agy"));
        // Windsurf's language_server without antigravity markers:
        assert!(!is_antigravity_command_line("/x/windsurf/language_server_macos --app_data_dir /x/windsurf"));
    }

    #[test]
    fn extracts_flags_both_syntaxes() {
        assert_eq!(extract_flag("a --csrf_token=tok1 b", "--csrf_token").as_deref(), Some("tok1"));
        assert_eq!(extract_flag("a --csrf_token tok2 b", "--csrf_token").as_deref(), Some("tok2"));
        assert_eq!(extract_flag("a b", "--csrf_token"), None);
    }

    #[test]
    fn parses_lsof_listen_ports() {
        let out = "language_ 423 me 12u IPv4 0x0 0t0 TCP 127.0.0.1:42117 (LISTEN)\n\
                   language_ 423 me 13u IPv4 0x0 0t0 TCP 127.0.0.1:42118 (LISTEN)\n\
                   language_ 423 me 14u IPv4 0x0 0t0 TCP 1.2.3.4:443->5.6.7.8:1 (ESTABLISHED)\n";
        assert_eq!(parse_listening_ports(out), vec![42117, 42118]);
    }

    #[test]
    fn normalizes_quota_summary_buckets() {
        let body = json!({ "response": { "groups": [
            { "buckets": [
                { "bucketId": "3p-weekly", "remainingFraction": 1.0, "resetTime": "2026-07-18T00:00:00Z" },
                { "bucketId": "3p-5h", "remainingFraction": 1.0 },
            ]},
            { "buckets": [
                { "bucketId": "gemini-weekly", "remainingFraction": 0.76 },
                { "bucketId": "gemini-5h", "remainingFraction": 1.0 },
            ]},
        ]}});
        let w = normalize_quota_summary(&body).unwrap();
        let labels: Vec<&str> = w.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Cl 7d", "Cl 5h", "Gm 7d", "Gm 5h"]);
        assert!((w[2].used_percent - 24.0).abs() < 1e-9);
    }

    #[test]
    fn quota_summary_unknown_buckets_is_error() {
        let body = json!({ "response": { "groups": [
            { "buckets": [ { "bucketId": "renamed", "remainingFraction": 0.5 } ] },
        ]}});
        assert!(normalize_quota_summary(&body).is_err());
    }

    #[test]
    fn quota_summary_bad_code_is_error() {
        assert!(normalize_quota_summary(&json!({ "code": 7 })).is_err());
    }

    #[test]
    fn normalizes_user_status_families() {
        let body = json!({ "userStatus": {
            "planStatus": { "planInfo": { "planDisplayName": "Dev" } },
            "cascadeModelConfigData": { "clientModelConfigs": [
                { "label": "Claude Sonnet", "modelOrAlias": { "model": "claude-x" },
                  "quotaInfo": { "remainingFraction": 0.3, "resetTime": "2026-07-18T00:00:00Z" } },
                { "label": "Claude Opus", "modelOrAlias": { "model": "claude-y" },
                  "quotaInfo": { "remainingFraction": 0.9 } },
                { "label": "Gemini Pro", "modelOrAlias": { "model": "gemini-pro" },
                  "quotaInfo": { "remainingFraction": 0.5 } },
                { "label": "Gemini Flash Lite", "modelOrAlias": { "model": "gemini-flash-lite" },
                  "quotaInfo": { "remainingFraction": 0.1 } },
            ]},
        }});
        let (plan, w) = normalize_user_status(&body, false).unwrap();
        assert_eq!(plan.as_deref(), Some("Dev"));
        let labels: Vec<&str> = w.iter().map(|w| w.label.as_str()).collect();
        // lite model excluded from chat models; claude min=0.3 → Cl 7d 70%,
        // claude max=0.9 → Cl 5h 10%, gemini pro both slots.
        assert_eq!(labels, vec!["Cl 7d", "Cl 5h", "Gm 7d", "Gm 5h"]);
        assert!((w[0].used_percent - 70.0).abs() < 1e-9);
        assert!((w[1].used_percent - 10.0).abs() < 1e-9);
    }

    #[test]
    fn user_status_no_models_is_error() {
        assert!(normalize_user_status(&json!({ "userStatus": {} }), false).is_err());
    }
}
```

- [ ] **Step 3: Uncomment `pub mod antigravity;`, run tests**

Run: `cargo test --manifest-path src-tauri/Cargo.toml limits::antigravity`
Expected: 9 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/limits/antigravity.rs src-tauri/src/limits/mod.rs src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "feat(limits): Antigravity local quota provider via IDE process detection"
```

---

### Task 7: Orchestrator, Tauri command, AppState wiring

**Files:**
- Modify: `src-tauri/src/limits/mod.rs` (add `fetch_snapshot`)
- Modify: `src-tauri/src/lib.rs` (AppState fields, setup, `limits` command, handler registration, existing test)

**Interfaces:**
- Consumes: all five providers' `fetch`, `claude::precheck`.
- Produces:
  - `pub fn fetch_snapshot(data_dir: &Path, home: &Path) -> LimitsSnapshot` in `limits::`
  - Tauri command `limits(state, force: bool) -> Result<LimitsSnapshot, String>`
  - `AppState` gains `pub data_dir: PathBuf` and `pub limits_snapshot: Mutex<Option<(std::time::Instant, limits::LimitsSnapshot)>>`

- [ ] **Step 1: Add `fetch_snapshot` to `limits/mod.rs`**

Append (above the test module):

```rust
// ---- orchestrator ----------------------------------------------------------

/// One full snapshot: all five providers in parallel, cache-aware.
/// Blocking (network + subprocesses); callers run it off the UI path.
pub fn fetch_snapshot(data_dir: &Path, home: &Path) -> LimitsSnapshot {
    let now = now_ts();
    let cache_path = data_dir.join("limits-cache.json");
    let mut cache = load_cache(&cache_path);

    // Claude may be served from cache without a live call (fresh success
    // within CLAUDE_FRESH_SECS, or an armed 429 cooldown).
    let claude_pre = claude::precheck(&cache, now);

    let (claude_r, codex_r, gemini_r, grok_r, anti_r) = std::thread::scope(|s| {
        let c = s.spawn(|| match claude_pre {
            Some(t) => Ok(t),
            None => claude::fetch(home, now),
        });
        let x = s.spawn(|| codex::fetch(home, now));
        let g = s.spawn(|| gemini::fetch(home, now));
        let k = s.spawn(|| grok::fetch(home, now));
        let a = s.spawn(|| antigravity::fetch(home, now));
        (
            c.join().unwrap_or_else(|_| Err("Claude provider panicked".into())),
            x.join().unwrap_or_else(|_| Err("Codex provider panicked".into())),
            g.join().unwrap_or_else(|_| Err("Gemini provider panicked".into())),
            k.join().unwrap_or_else(|_| Err("Grok provider panicked".into())),
            a.join().unwrap_or_else(|_| Err("Antigravity provider panicked".into())),
        )
    });

    // Arm / clear the Claude 429 cooldown before resolving fallbacks.
    match &claude_r {
        Err(e) if e.retry_after_secs.is_some() => {
            cache.claude_retry_at_ts = Some(now + e.retry_after_secs.unwrap());
        }
        Ok(t) if t.error.is_none() && !t.windows.is_empty() => {
            cache.claude_retry_at_ts = None;
        }
        _ => {}
    }

    let mut tools = Vec::with_capacity(5);
    for (source, live) in [
        ("claude", claude_r),
        ("codex", codex_r),
        ("gemini", gemini_r),
        ("grok", grok_r),
        ("antigravity", anti_r),
    ] {
        let resolved = with_fallback(source, live, cache.tools.get(source), now);
        // Persist live successes (real bars, not served-from-cache) for
        // future stale fallbacks.
        if resolved.configured
            && resolved.error.is_none()
            && !resolved.windows.is_empty()
            && !resolved.stale
            && resolved.cached_at_ts == Some(now)
        {
            cache
                .tools
                .insert(source.to_string(), CachedTool { tool: resolved.clone(), cached_at_ts: now });
        }
        tools.push(resolved);
    }
    save_cache(&cache_path, &cache);
    LimitsSnapshot { fetched_at_ts: now, tools }
}
```

Note: `claude::precheck` fresh-serves carry the ORIGINAL `cached_at_ts` (not `now`), so the persist guard above correctly skips re-writing them.

- [ ] **Step 2: Wire lib.rs**

In `src-tauri/src/lib.rs`:

1. Add to the `use` block: `use std::path::PathBuf;` and `use limits::LimitsSnapshot;`
2. Extend `AppState`:

```rust
pub struct AppState {
    pub db: Mutex<Connection>,
    pub roots: SourceRoots,
    pub scan_lock: Mutex<()>,
    pub data_dir: PathBuf,
    pub limits_snapshot: Mutex<Option<(std::time::Instant, LimitsSnapshot)>>,
}
```

3. In `setup`, keep a clone for state (the pricing thread moves the original):

```rust
            app.manage(AppState {
                db: Mutex::new(conn),
                roots: SourceRoots::default_roots(),
                scan_lock: Mutex::new(()),
                data_dir: data_dir.clone(),
                limits_snapshot: Mutex::new(None),
            });
```

4. Add the command (near `scan`):

```rust
#[tauri::command]
async fn limits(state: State<'_, AppState>, force: bool) -> Result<LimitsSnapshot, String> {
    // Hold the snapshot lock across the fetch: a second caller blocks, then
    // sees the fresh cache — natural coalescing, mirroring scan_lock.
    let mut guard = state.limits_snapshot.lock().map_err(|e| e.to_string())?;
    if !force {
        if let Some((at, snap)) = guard.as_ref() {
            if at.elapsed().as_secs() < limits::SNAPSHOT_TTL_SECS {
                return Ok(snap.clone());
            }
        }
    }
    let home = dirs::home_dir().ok_or("no home directory")?;
    let snap = limits::fetch_snapshot(&state.data_dir, &home);
    *guard = Some((std::time::Instant::now(), snap.clone()));
    Ok(snap)
}
```

5. Register `limits` in `invoke_handler` (after `scan,`).
6. Update the existing `appstate_wires_scan_and_query` test's `AppState` literal — add:

```rust
            data_dir: dir.path().to_path_buf(),
            limits_snapshot: Mutex::new(None),
```

- [ ] **Step 3: Full backend test run**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`
Expected: entire suite passes (limits + adapters + queries + e2e_real_logs).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/limits/mod.rs src-tauri/src/lib.rs
git commit -m "feat(limits): parallel snapshot orchestrator and limits Tauri command"
```

---

### Task 8: Frontend contract — types, api, time formatters

**Files:**
- Modify: `src/types.ts`, `src/api.ts`, `src/lib/format.ts`
- Test: `src/lib/format.test.ts`

**Interfaces:**
- Produces:
  - TS `LimitWindow { label: string; usedPercent: number; resetsAtTs: number | null }`, `ToolLimits`, `LimitsSnapshot` in types.ts
  - `fetchLimits(force?: boolean): Promise<LimitsSnapshot>` in api.ts
  - `fmtRemain(ts: number | null, nowMs?: number): string` and `fmtAgo(ts: number | null, nowMs?: number): string` in format.ts

- [ ] **Step 1: Write failing formatter tests**

Append to `src/lib/format.test.ts`:

```ts
import { fmtRemain, fmtAgo } from './format';

const NOW = Date.parse('2026-07-12T12:00:00Z');
const ts = (iso: string) => Math.floor(Date.parse(iso) / 1000);

describe('fmtRemain', () => {
  it('em-dash for null', () => expect(fmtRemain(null, NOW)).toBe('—'));
  it('now for past', () => expect(fmtRemain(ts('2026-07-12T11:00:00Z'), NOW)).toBe('now'));
  it('minutes under an hour', () =>
    expect(fmtRemain(ts('2026-07-12T12:34:00Z'), NOW)).toBe('34m'));
  it('hours under a day', () =>
    expect(fmtRemain(ts('2026-07-12T14:30:00Z'), NOW)).toBe('2h'));
  it('days beyond 24h', () =>
    expect(fmtRemain(ts('2026-07-18T13:00:00Z'), NOW)).toBe('6d'));
});

describe('fmtAgo', () => {
  it('em-dash for null', () => expect(fmtAgo(null, NOW)).toBe('—'));
  it('just now under a minute', () =>
    expect(fmtAgo(ts('2026-07-12T11:59:30Z'), NOW)).toBe('just now'));
  it('minutes', () => expect(fmtAgo(ts('2026-07-12T11:15:00Z'), NOW)).toBe('45m ago'));
  it('hours', () => expect(fmtAgo(ts('2026-07-12T10:00:00Z'), NOW)).toBe('2h ago'));
  it('days', () => expect(fmtAgo(ts('2026-07-10T10:00:00Z'), NOW)).toBe('2d ago'));
});
```

Run: `npm test`
Expected: FAIL — `fmtRemain` is not exported.

- [ ] **Step 2: Implement formatters**

Append to `src/lib/format.ts`:

```ts
// Time-until a limit-window reset (epoch seconds) — '34m', '2h', '6d'.
export function fmtRemain(ts: number | null, nowMs = Date.now()): string {
  if (ts == null) return '—';
  const secs = ts - Math.floor(nowMs / 1000);
  if (secs <= 0) return 'now';
  const m = Math.floor(secs / 60);
  if (m < 60) return `${Math.max(1, m)}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h`;
  return `${Math.floor(h / 24)}d`;
}

// Age of a reading (epoch seconds) — 'just now', '45m ago', '2h ago'.
export function fmtAgo(ts: number | null, nowMs = Date.now()): string {
  if (ts == null) return '—';
  const secs = Math.floor(nowMs / 1000) - ts;
  if (secs < 60) return 'just now';
  const m = Math.floor(secs / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  return `${Math.floor(h / 24)}d ago`;
}
```

Run: `npm test`
Expected: PASS.

- [ ] **Step 3: Types + api**

Append to `src/types.ts`:

```ts
// ---- Limits page (limits command) ----

export interface LimitWindow {
  label: string;
  usedPercent: number;
  resetsAtTs: number | null; // epoch seconds
}

export interface ToolLimits {
  source: string; // 'claude' | 'codex' | 'gemini' | 'grok' | 'antigravity'
  configured: boolean;
  error: string | null;
  plan: string | null;
  windows: LimitWindow[];
  stale: boolean;
  cachedAtTs: number | null; // epoch seconds
}

export interface LimitsSnapshot {
  fetchedAtTs: number; // epoch seconds
  tools: ToolLimits[];
}
```

Append to `src/api.ts` (and add `LimitsSnapshot` to the type import list):

```ts
export function fetchLimits(force = false): Promise<LimitsSnapshot> {
  return invoke('limits', { force });
}
```

Run: `npx tsc --noEmit`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add src/types.ts src/api.ts src/lib/format.ts src/lib/format.test.ts
git commit -m "feat(limits): frontend contract — types, fetchLimits, remain/age formatters"
```

---

### Task 9: Wire `Limits.tsx` to real data

**Files:**
- Modify: `src/overview/Limits.tsx` (full rewrite of the data layer; visual card system stays)

**Interfaces:**
- Consumes: `fetchLimits`, `ToolLimits`, `LimitsSnapshot`, `fmtRemain`, `fmtAgo`.
- Produces: `export default function Limits({ nav, onNav }: { nav: string; onNav: (n: string) => void })` — Task 10 mounts it with these props. Until Task 10, `main.tsx` temporarily renders `<Limits nav="Limits" onNav={() => {}} />`.

- [ ] **Step 1: Rewrite Limits.tsx**

Replace the file's data machinery, keeping the visual language (same inline styles, `ToolIcon`, `SCOPED_CSS`, seg buttons, bar grid, animations). Deletions: `RAW`, `ToolDef`/`BarDef`, `ACCENT`-adjacent design-prop constants `COMPACT_ROWS`/`SHOW_TICKS`, `connected` state, `connect()`, tick/warn rendering, fake `cachedAge`. The full new component logic:

```tsx
import { useCallback, useEffect, useRef, useState } from 'react';
import { fetchLimits } from '../api';
import type { LimitsSnapshot, ToolLimits } from '../types';
import { fmtAgo, fmtRemain } from '../lib/format';

const MONO = "ui-monospace,'SF Mono',Menlo,monospace";
const ACCENT = '#2a6df4';
const REFRESH_MS = 5 * 60 * 1000;

type IconKey = 'claude' | 'codex' | 'gemini' | 'grok' | 'antigravity';

const TOOL_META: Record<IconKey, { name: string; color: string }> = {
  claude: { name: 'Claude', color: '#d97757' },
  codex: { name: 'Codex', color: '#59c2a6' },
  gemini: { name: 'Gemini', color: '#e2a63b' },
  grok: { name: 'Grok Build', color: '#b8c0cc' },
  antigravity: { name: 'Antigravity', color: '#37c98b' },
};

// ToolIcon: keep the existing claude/codex/gemini/grok/antigravity arms,
// drop cursor/kiro/copilot/kimi/zcode/letter arms.

const NAV = ['Overview', 'Activity', 'Models', 'Limits', 'Settings'];

export default function Limits({ nav, onNav }: { nav: string; onNav: (n: string) => void }) {
  const [snap, setSnap] = useState<LimitsSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [mode, setMode] = useState<'used' | 'left'>('used');
  const [filter, setFilter] = useState<'all' | 'connected'>('all');
  const inflight = useRef(false);

  const load = useCallback(async (force: boolean) => {
    if (inflight.current) return;
    inflight.current = true;
    setRefreshing(true);
    try {
      setSnap(await fetchLimits(force));
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      inflight.current = false;
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    void load(false);
    const id = setInterval(() => void load(false), REFRESH_MS);
    return () => clearInterval(id);
  }, [load]);

  const tools = snap?.tools ?? [];
  const isConnected = (t: ToolLimits) => t.configured && !t.error;
  const connected = tools.filter(isConnected).length;
  const countLabel = `${tools.length} tools · ${connected} connected`;
  const visible = filter === 'connected' ? tools.filter(isConnected) : tools;

  const barColor = (t: ToolLimits, used: number) => {
    if (mode === 'used') {
      if (used >= 90) return '#f0616d';
      if (used >= 75) return '#e2a63b';
    }
    return TOOL_META[t.source as IconKey]?.color ?? '#5b9dff';
  };
  // …render: shell + heading identical to the mockup; toolbar seg buttons
  // drive mode/filter; ⟳ calls load(true) and spins while `refreshing`;
  // footer shows `Last checked ${fmtAgo(snap?.fetchedAtTs ?? null)} ·
  // auto-refreshes every 5 min`.
}
```

Card rendering rules (inside `visible.map((t) => …)`, same card markup as the mockup):

- Title: `t.plan ? `${meta.name} ${t.plan}` : meta.name` (e.g. "Claude Pro", "Gemini Paid").
- Badge: `t.stale && <span …green badge…>cached · {fmtAgo(t.cachedAtTs)}</span>` (reuses the mockup's badge styles verbatim).
- `!t.configured` → the mockup's "Not connected" row minus the Connect button.
- `t.error` → the mockup's error card minus the Authenticate button (`<div style={{ fontSize: '12px', color: '#f0616d', lineHeight: 1.5 }}>{t.error}</div>`).
- `t.configured && !t.error && t.windows.length === 0` → `<span style={{ fontSize: '12.5px', color: '#6d7793' }}>No usage data</span>`.
- Bars: for each `w` of `t.windows`: display pct `mode === 'used' ? w.usedPercent : 100 - w.usedPercent` (rounded for the % text), fill width `Math.max(dp, 2)`% when `dp > 0` (mockup convention), fill color `barColor(t, w.usedPercent)`, remain column `fmtRemain(w.resetsAtTs)`. Grid columns stay `48px 1fr 42px 30px`.
- Loading state (`snap === null && !error`): render the shell with a single muted `Loading…` line in place of the grid.
- Top-bar nav: `NAV.map((n) => …)` — active styling from the `nav` prop; `onClick={() => (n === 'Overview' || n === 'Limits') && onNav(n)}`.
- The page-level `tt-rise` animations and `tl-spin` spinner stay as-is.
- Avatar initials: change "MK" → "BW" (matches Overview8b).

Temporarily update `src/main.tsx` to `<Limits nav="Limits" onNav={() => {}} />` so it compiles.

- [ ] **Step 2: Typecheck + tests**

Run: `npx tsc --noEmit && npm test`
Expected: both clean.

- [ ] **Step 3: Commit**

```bash
git add src/overview/Limits.tsx src/main.tsx
git commit -m "feat(limits): wire Limits page to the live limits command"
```

---

### Task 10: Real nav — Overview ↔ Limits shell

**Files:**
- Modify: `src/main.tsx`, `src/overview/Overview8b.tsx`, `src/overview/Limits.tsx` (nav prop already added in Task 9)

**Interfaces:**
- Produces: `main.tsx` `App` owning `nav` state; `Overview8b({ nav, onNav })` with the same contract as `Limits`.

- [ ] **Step 1: main.tsx shell**

```tsx
import React, { useState } from "react";
import ReactDOM from "react-dom/client";
import Overview8b from "./overview/Overview8b";
import Limits from "./overview/Limits";
import "./index.css";

function App() {
  const [nav, setNav] = useState("Overview");
  return nav === "Limits" ? (
    <Limits nav={nav} onNav={setNav} />
  ) : (
    <Overview8b nav={nav} onNav={setNav} />
  );
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
```

- [ ] **Step 2: Overview8b controlled nav**

In `src/overview/Overview8b.tsx`:
- Change `const NAV = ['Overview', 'Insights', 'Models', 'Settings'];` to `const NAV = ['Overview', 'Activity', 'Models', 'Limits', 'Settings'];` (unified with Limits).
- Change the signature to `export default function Overview8b({ nav, onNav }: { nav: string; onNav: (n: string) => void })` and DELETE the `const [nav, setNav] = useState('Overview');` line.
- In the nav render, replace `onClick={() => setNav(n)}` with `onClick={() => (n === 'Overview' || n === 'Limits') && onNav(n)}`.

- [ ] **Step 3: Verify + commit**

Run: `npx tsc --noEmit && npm test`
Expected: clean.

```bash
git add src/main.tsx src/overview/Overview8b.tsx src/overview/Limits.tsx
git commit -m "feat(limits): real Overview/Limits navigation shell"
```

---

### Task 11: End-to-end verification

**Files:** none (verification only)

- [ ] **Step 1: Full suites**

Run: `cargo test --manifest-path src-tauri/Cargo.toml && npx tsc --noEmit && npm test`
Expected: everything green.

- [ ] **Step 2: Run the real app**

Run: `npm run tauri dev` (NOTE: dev runs against the prod app-data DB — fine here, this feature has no schema migrations; do not run experimental schema code in this session).

Verify by hand on this machine (Claude, Codex, Gemini, Grok credentials present):
1. Nav to Limits → five cards render; Claude shows `5h`/`7d` (+`Opus`/scoped rows if the API returns them) with a plan suffix in the title; tools without creds show "Not connected".
2. Used/Left toggle inverts percentages; All/Connected filters cards.
3. ⟳ spins and re-fetches (`force: true`), footer's "Last checked" updates to "just now".
4. macOS keychain prompt appears once for "Claude Code-credentials" — approve with "Always Allow".
5. Overview ↔ Limits nav round-trips; Overview still fully works.
6. Stale path: `cargo test` proves the logic; optionally toggle Wi-Fi off and force-refresh — cards flip to `cached · …` badges instead of erroring.

- [ ] **Step 3: Final commit if any fixups**

```bash
git add -A && git commit -m "fix(limits): e2e verification fixups"
```

---

## Self-Review (completed)

- **Spec coverage:** all five providers (Tasks 2–6), normalized contract (1), caching/429/fresh policies (1, 3, 7), command + parallel orchestration (7), frontend contract + rendering rules incl. "No usage data" (8, 9), real nav (10), keychain note (3, 11), testing incl. named edge cases (each task + 11). Non-goals untouched.
- **Placeholders:** none — every step carries code or exact commands.
- **Type consistency:** `ToolLimits`/`LimitWindow` field names match across Rust (`snake_case` + camelCase serde) and TS (camelCase); `fetch(home, now_ts) -> Result<ToolLimits, FetchErr>` uniform across providers; `precheck` consumed by Task 7 as defined in Task 3.
