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
