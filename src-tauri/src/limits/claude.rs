// Claude Code — OAuth token from the macOS keychain (service
// "Claude Code-credentials") or ~/.claude/.credentials.json elsewhere →
// GET api.anthropic.com/api/oauth/usage. Access tokens are short-lived, so an
// expiring/rejected token is refreshed via the Claude Code OAuth client
// (platform.claude.com) and the rotated credential is written back to the
// store, exactly like Claude Code itself does; without this the card strands
// on stale cache within hours. 429s arm a persisted cooldown; a success
// within CLAUDE_FRESH_SECS is served from disk without calling out
// (refresh-spam cannot burn OAuth usage requests).
use std::path::Path;

use serde_json::Value;

use super::{
    agent, clamp_percent, parse_any_ts, DiskCache, FetchErr, LimitWindow, ToolLimits,
    CLAUDE_FRESH_SECS,
};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const REFRESH_URL: &str = "https://platform.claude.com/v1/oauth/token";
// Claude Code's public OAuth client id — the refresh grant only accepts
// refresh tokens minted for this client.
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const OAUTH_SCOPES: &str =
    "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
// Anthropic bot-filters unfamiliar agents on this endpoint; identify as
// Claude Code like other trackers do.
const USER_AGENT: &str = "claude-code/2.1.69";
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
const DEFAULT_COOLDOWN_SECS: i64 = 300;
const MAX_COOLDOWN_SECS: i64 = 3600;
const EXPIRED_MSG: &str = "Claude token expired — run `claude` once to re-login.";
const RELOGIN_MSG: &str =
    "Claude login can't read usage (missing user:profile scope) — run `claude` and sign in again.";

/// How a credential read ended: found (raw text as stored, and where), cleanly
/// absent (never logged in → "Not connected"), or blocked (item may exist but
/// can't be read — locked keychain, denied access prompt — which must surface
/// as an actionable error, never as "Not connected").
enum CredRead {
    Found { raw: String, from_keychain: bool },
    NotFound,
    Blocked(String),
}

/// security(1) outcome → CredRead. Exit 44 is errSecItemNotFound; any other
/// failure means the read was blocked rather than the item being absent.
fn classify_security_output(code: Option<i32>, stdout: Option<String>) -> CredRead {
    let text = stdout.map(|s| s.trim().to_string()).unwrap_or_default();
    match code {
        Some(0) if !text.is_empty() => CredRead::Found { raw: text, from_keychain: true },
        Some(0) | Some(44) => CredRead::NotFound,
        c => CredRead::Blocked(format!(
            "keychain read failed (security exit {})",
            c.map(|v| v.to_string()).unwrap_or_else(|| "?".into())
        )),
    }
}

fn read_keychain() -> CredRead {
    match std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .output()
    {
        Ok(out) => classify_security_output(out.status.code(), String::from_utf8(out.stdout).ok()),
        Err(e) => CredRead::Blocked(format!("couldn't run security: {e}")),
    }
}

fn read_cred_file(home: &Path) -> Option<String> {
    std::fs::read_to_string(home.join(".claude/.credentials.json"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Keychain first on macOS with the credentials file as fallback (either can
/// hold the login); file only elsewhere.
fn read_credentials_source(home: &Path) -> CredRead {
    if cfg!(target_os = "macos") {
        match read_keychain() {
            found @ CredRead::Found { .. } => found,
            not_found_or_blocked => match read_cred_file(home) {
                Some(raw) => CredRead::Found { raw, from_keychain: false },
                None => not_found_or_blocked,
            },
        }
    } else {
        match read_cred_file(home) {
            Some(raw) => CredRead::Found { raw, from_keychain: false },
            None => CredRead::NotFound,
        }
    }
}

fn oauth_str(payload: &Value, key: &str) -> Option<String> {
    let t = payload.get("claudeAiOauth")?.get(key)?.as_str()?.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn access_token(payload: &Value) -> Option<String> {
    oauth_str(payload, "accessToken")
}

fn expires_at_ms(payload: &Value) -> Option<f64> {
    payload.get("claudeAiOauth")?.get("expiresAt")?.as_f64()
}

/// Refresh when the access token is within 5 minutes of expiry (or past it).
/// No expiry recorded → assume valid; a rejection still triggers a refresh.
pub fn needs_refresh(payload: &Value, now_ts: i64) -> bool {
    expires_at_ms(payload)
        .map(|e| e - (now_ts as f64) * 1000.0 <= 5.0 * 60.0 * 1000.0)
        .unwrap_or(false)
}

fn token_expired(payload: &Value, now_ts: i64) -> bool {
    expires_at_ms(payload).map(|e| e <= (now_ts as f64) * 1000.0).unwrap_or(false)
}

/// A login whose granted scopes lack `user:profile` (e.g. a `claude
/// setup-token` token) can run inference but 403s on the usage endpoint.
/// Absent/empty scopes = older credential; allow and let the call decide.
pub fn missing_profile_scope(payload: &Value) -> bool {
    match payload.get("claudeAiOauth").and_then(|o| o.get("scopes")).and_then(Value::as_array) {
        Some(scopes) if !scopes.is_empty() => {
            !scopes.iter().any(|s| s.as_str() == Some("user:profile"))
        }
        _ => false,
    }
}

/// subscriptionType → title-cased plan label; free/none/unknown filtered.
/// A multiplier in rateLimitTier (e.g. "..._20x") is appended: "Max 20x".
pub fn plan_of(payload: &Value) -> Option<String> {
    let raw = payload.get("claudeAiOauth")?.get("subscriptionType")?.as_str()?.trim();
    let lower = raw.to_lowercase();
    if raw.is_empty() || lower == "free" || lower == "none" || lower == "unknown" {
        return None;
    }
    let mut c = lower.chars();
    let base =
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default();
    let tier = payload
        .get("claudeAiOauth")
        .and_then(|o| o.get("rateLimitTier"))
        .and_then(Value::as_str)
        .and_then(|t| {
            t.split('_').find(|s| {
                s.len() >= 2
                    && s.ends_with('x')
                    && s[..s.len() - 1].bytes().all(|b| b.is_ascii_digit())
            })
        });
    Some(match tier {
        Some(t) => format!("{base} {t}"),
        None => base,
    })
}

/// Merge a refresh-grant response into the credential payload: new access
/// token, rotated refresh token (kept when the response omits it), and
/// expiresAt recomputed from expires_in. False = no usable access token.
pub fn apply_refresh_response(payload: &mut Value, tokens: &Value, now_ts: i64) -> bool {
    let access = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let Some(access) = access else { return false };
    let Some(oauth) = payload.get_mut("claudeAiOauth").and_then(Value::as_object_mut) else {
        return false;
    };
    oauth.insert("accessToken".into(), Value::String(access));
    if let Some(rt) = tokens
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        oauth.insert("refreshToken".into(), Value::String(rt.to_string()));
    }
    if let Some(expires_in) = tokens.get("expires_in").and_then(Value::as_f64) {
        if let Some(n) = serde_json::Number::from_f64((now_ts as f64 + expires_in) * 1000.0) {
            oauth.insert("expiresAt".into(), Value::Number(n));
        }
    }
    true
}

/// Account name of the Claude Code keychain item, so the write-back updates
/// the existing item instead of creating a duplicate under another account.
fn keychain_account() -> String {
    if let Ok(out) = std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE])
        .output()
    {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout).into_owned()
                + &String::from_utf8_lossy(&out.stderr);
            if let Some(i) = text.find("\"acct\"<blob>=\"") {
                let rest = &text[i + 14..];
                if let Some(j) = rest.find('"') {
                    return rest[..j].to_string();
                }
            }
        }
    }
    std::env::var("USER").unwrap_or_default()
}

/// Best-effort write-back of a rotated credential to the store it came from —
/// Claude Code must keep working with the refresh token we just rotated away.
/// Guarded: skipped when the store no longer holds exactly what we read, so a
/// credential rotated mid-flight by `claude` itself is never clobbered.
fn write_credentials(home: &Path, from_keychain: bool, original_raw: &str, payload: &Value) {
    let current = if from_keychain {
        match read_keychain() {
            CredRead::Found { raw, .. } => Some(raw),
            _ => None,
        }
    } else {
        read_cred_file(home)
    };
    if current.as_deref() != Some(original_raw) {
        return;
    }
    let Ok(text) = serde_json::to_string(payload) else { return };
    if from_keychain {
        // ponytail: the blob rides argv for a moment (ps-visible to this
        // user's machine only); Security.framework FFI if that ever matters.
        let _ = std::process::Command::new("/usr/bin/security")
            .args([
                "add-generic-password",
                "-U",
                "-a",
                &keychain_account(),
                "-s",
                KEYCHAIN_SERVICE,
                "-w",
                &text,
            ])
            .output();
    } else {
        let _ = std::fs::write(home.join(".claude/.credentials.json"), text);
    }
}

/// Refresh grant → rotated tokens merged into `payload` and persisted.
/// False = refresh unavailable/failed; caller keeps the old token.
fn try_refresh(
    home: &Path,
    from_keychain: bool,
    original_raw: &str,
    payload: &mut Value,
    now_ts: i64,
) -> bool {
    let Some(refresh_token) = oauth_str(payload, "refreshToken") else { return false };
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": CLIENT_ID,
        "scope": OAUTH_SCOPES,
    });
    let Ok(resp) = agent()
        .post(REFRESH_URL)
        .set("Content-Type", "application/json")
        .set("User-Agent", USER_AGENT)
        .send_json(body)
    else {
        return false;
    };
    let Ok(tokens) = resp.into_json::<Value>() else { return false };
    if !apply_refresh_response(payload, &tokens, now_ts) {
        return false;
    }
    write_credentials(home, from_keychain, original_raw, payload);
    true
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
    windows.extend(window_of(body.get("seven_day_sonnet"), "Sonnet"));
    windows.extend(window_of(body.get("seven_day_opus"), "Opus"));
    // Model-scoped weekly windows (e.g. Fable) arrive only in the generic
    // limits[] array; a scoped entry duplicating a legacy seven_day_<model>
    // window above is dropped so the same window never renders twice.
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
            if windows.iter().any(|w| w.label.eq_ignore_ascii_case(label)) {
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
                    t.stale_reason = Some(rate_limit_msg(Some(retry_at - now_ts)));
                    t.cached_at_ts = Some(c.cached_at_ts);
                    return Some(t);
                }
            }
            return Some(ToolLimits::error_card("claude", rate_limit_msg(Some(retry_at - now_ts))));
        }
    }
    None
}

fn call_usage(token: &str) -> Result<ureq::Response, ureq::Error> {
    agent()
        .get(USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .set("Accept", "application/json")
        .set("User-Agent", USER_AGENT)
        .call()
}

pub fn fetch(home: &Path, now_ts: i64) -> Result<ToolLimits, FetchErr> {
    let (raw, from_keychain) = match read_credentials_source(home) {
        CredRead::Found { raw, from_keychain } => (raw, from_keychain),
        CredRead::NotFound => return Ok(ToolLimits::not_configured("claude")),
        CredRead::Blocked(detail) => {
            return Ok(ToolLimits::error_card(
                "claude",
                format!(
                    "Claude credentials unreadable: {detail}. If macOS shows a keychain \
                     prompt for \"security\", click Always Allow, then refresh."
                ),
            ));
        }
    };
    let Ok(mut payload) = serde_json::from_str::<Value>(&raw) else {
        return Ok(ToolLimits::not_configured("claude"));
    };
    if access_token(&payload).is_none() {
        return Ok(ToolLimits::not_configured("claude"));
    }
    if missing_profile_scope(&payload) {
        return Ok(ToolLimits::error_card("claude", RELOGIN_MSG));
    }
    let plan = plan_of(&payload);

    // Proactive refresh: don't spend a usage call on a token about to die.
    let mut refresh_attempted = false;
    if needs_refresh(&payload, now_ts) {
        refresh_attempted = true;
        if !try_refresh(home, from_keychain, &raw, &mut payload, now_ts)
            && token_expired(&payload, now_ts)
        {
            return Ok(ToolLimits::error_card("claude", EXPIRED_MSG));
        }
    }

    loop {
        let Some(token) = access_token(&payload) else {
            return Ok(ToolLimits::error_card("claude", EXPIRED_MSG));
        };
        match call_usage(&token) {
            Ok(r) => {
                let body: Value = r.into_json().map_err(|e| FetchErr::from(e.to_string()))?;
                return Ok(ToolLimits::live("claude", plan, normalize(&body), now_ts));
            }
            // 401 and 403 both mean this token can't read usage; one refresh
            // + retry, then an actionable card (never silent stale bars).
            Err(ureq::Error::Status(401 | 403, _)) => {
                if !refresh_attempted {
                    refresh_attempted = true;
                    if try_refresh(home, from_keychain, &raw, &mut payload, now_ts) {
                        continue;
                    }
                }
                return Ok(ToolLimits::error_card("claude", EXPIRED_MSG));
            }
            Err(ureq::Error::Status(429, r)) => {
                let retry = r
                    .header("retry-after")
                    .and_then(|v| v.parse::<i64>().ok())
                    .filter(|s| *s > 0)
                    .unwrap_or(DEFAULT_COOLDOWN_SECS)
                    .min(MAX_COOLDOWN_SECS);
                return Err(FetchErr {
                    message: rate_limit_msg(Some(retry)),
                    retry_after_secs: Some(retry),
                });
            }
            Err(ureq::Error::Status(code, _)) => {
                return Err(FetchErr::from(format!("Claude API returned {code}")));
            }
            Err(e) => return Err(FetchErr::from(e.to_string())),
        }
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
    fn normalize_includes_sonnet_window() {
        let mut body = usage_body();
        body.as_object_mut().unwrap().insert(
            "seven_day_sonnet".into(),
            json!({ "utilization": 7, "resets_at": "2026-07-14T00:00:00Z" }),
        );
        let w = normalize(&body);
        let labels: Vec<&str> = w.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "7d", "Sonnet", "Opus", "Fable"]);
    }

    #[test]
    fn plan_title_cases_and_filters() {
        let p = json!({ "claudeAiOauth": { "subscriptionType": "pro" } });
        assert_eq!(plan_of(&p).as_deref(), Some("Pro"));
        let f = json!({ "claudeAiOauth": { "subscriptionType": "free" } });
        assert_eq!(plan_of(&f), None);
    }

    #[test]
    fn plan_appends_rate_limit_tier_multiplier() {
        let p = json!({ "claudeAiOauth": {
            "subscriptionType": "max", "rateLimitTier": "default_claude_max_20x"
        }});
        assert_eq!(plan_of(&p).as_deref(), Some("Max 20x"));
        let no_tier = json!({ "claudeAiOauth": {
            "subscriptionType": "max", "rateLimitTier": "default_claude"
        }});
        assert_eq!(plan_of(&no_tier).as_deref(), Some("Max"));
    }

    #[test]
    fn needs_refresh_only_near_expiry() {
        let now = 1_800_000_000_i64;
        let fresh = json!({ "claudeAiOauth": { "expiresAt": (now as f64 + 3600.0) * 1000.0 } });
        assert!(!needs_refresh(&fresh, now));
        let near = json!({ "claudeAiOauth": { "expiresAt": (now as f64 + 60.0) * 1000.0 } });
        assert!(needs_refresh(&near, now));
        let past = json!({ "claudeAiOauth": { "expiresAt": (now as f64 - 60.0) * 1000.0 } });
        assert!(needs_refresh(&past, now));
        assert!(token_expired(&past, now));
        assert!(!token_expired(&near, now));
        let none = json!({ "claudeAiOauth": {} });
        assert!(!needs_refresh(&none, now));
    }

    #[test]
    fn missing_profile_scope_detection() {
        let no_scopes = json!({ "claudeAiOauth": {} });
        assert!(!missing_profile_scope(&no_scopes));
        let inference_only = json!({ "claudeAiOauth": { "scopes": ["user:inference"] } });
        assert!(missing_profile_scope(&inference_only));
        let full = json!({ "claudeAiOauth": { "scopes": ["user:profile", "user:inference"] } });
        assert!(!missing_profile_scope(&full));
        let empty = json!({ "claudeAiOauth": { "scopes": [] } });
        assert!(!missing_profile_scope(&empty));
    }

    #[test]
    fn classify_keychain_read_outcomes() {
        assert!(matches!(
            classify_security_output(Some(0), Some("{\"claudeAiOauth\":{}}".into())),
            CredRead::Found { from_keychain: true, .. }
        ));
        // 44 = errSecItemNotFound → genuinely not logged in
        assert!(matches!(classify_security_output(Some(44), None), CredRead::NotFound));
        assert!(matches!(classify_security_output(Some(0), Some("  ".into())), CredRead::NotFound));
        // any other failure (locked keychain, denied prompt) is blocked, not absent
        assert!(matches!(classify_security_output(Some(36), None), CredRead::Blocked(_)));
        assert!(matches!(classify_security_output(None, None), CredRead::Blocked(_)));
    }

    #[test]
    fn refresh_response_rotates_tokens_and_expiry() {
        let now = 1_800_000_000_i64;
        let mut payload = json!({ "claudeAiOauth": {
            "accessToken": "old-access", "refreshToken": "old-refresh", "expiresAt": 1.0,
            "subscriptionType": "pro"
        }});
        let tokens = json!({
            "access_token": "new-access", "refresh_token": "new-refresh", "expires_in": 28800
        });
        assert!(apply_refresh_response(&mut payload, &tokens, now));
        let oauth = payload.get("claudeAiOauth").unwrap();
        assert_eq!(oauth.get("accessToken").unwrap(), "new-access");
        assert_eq!(oauth.get("refreshToken").unwrap(), "new-refresh");
        assert_eq!(
            oauth.get("expiresAt").unwrap().as_f64().unwrap(),
            (now as f64 + 28800.0) * 1000.0
        );
        // untouched fields survive the merge
        assert_eq!(oauth.get("subscriptionType").unwrap(), "pro");
    }

    #[test]
    fn refresh_response_keeps_old_refresh_token_when_omitted() {
        let mut payload =
            json!({ "claudeAiOauth": { "accessToken": "a", "refreshToken": "keep-me" } });
        let tokens = json!({ "access_token": "new-access" });
        assert!(apply_refresh_response(&mut payload, &tokens, 0));
        let oauth = payload.get("claudeAiOauth").unwrap();
        assert_eq!(oauth.get("accessToken").unwrap(), "new-access");
        assert_eq!(oauth.get("refreshToken").unwrap(), "keep-me");
    }

    #[test]
    fn refresh_response_without_access_token_is_rejected() {
        let mut payload = json!({ "claudeAiOauth": { "accessToken": "old" } });
        let before = payload.clone();
        assert!(!apply_refresh_response(&mut payload, &json!({ "error": "invalid_grant" }), 0));
        assert_eq!(payload, before);
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
