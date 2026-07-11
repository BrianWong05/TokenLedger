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
