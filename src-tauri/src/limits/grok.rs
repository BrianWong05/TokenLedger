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
