//! copilot-limits — read-only Companion for GitHub's account Limits.
//!
//! It follows Copilot CLI's login precedence, never writes or refreshes a
//! credential, and emits only the finite Premium Requests window (ADR-0022).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use tokenledger_lib::limits_artifact::{
    self, LimitsExport, WindowEvidence, WindowExport, NOT_SIGNED_IN,
};
use tokenledger_lib::time::iso_to_epoch;

const LIMITS_URL: &str = "https://api.github.com/copilot_internal/user";

fn main() {
    match run() {
        Ok(report) => println!("{report}"),
        Err(error) => {
            eprintln!("copilot-limits: {error}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<String, String> {
    let body = fetch(&credential()?)?;
    if std::env::args()
        .skip(1)
        .any(|argument| argument == "--shape")
    {
        return Ok(limits_artifact::shape(&body));
    }
    let fetched_at = now();
    let export = limits_export(&body, fetched_at)?;
    if let Some(directory) = std::env::var_os("TOKENLEDGER_LIMITS_DIR") {
        limits_artifact::write(&PathBuf::from(directory), &export)
            .map_err(|error| format!("could not write the export: {error}"))?;
    }
    serde_json::to_string(&export).map_err(|error| error.to_string())
}

fn limits_export(body: &Value, fetched_at: i64) -> Result<LimitsExport, String> {
    let plan = body
        .get("copilot_plan")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|plan| !plan.is_empty())
        .ok_or_else(|| "the vendor's answer carried no valid plan".to_string())?;
    Ok(LimitsExport {
        schema: limits_artifact::SCHEMA,
        source: "copilot".to_string(),
        fetched_at,
        plan: Some(plan.to_string()),
        // Copilot is not in the estimate map; its Readings stay display-only.
        metering_regime: None,
        // No proven account identity on this path yet: absent is honest, never
        // "the same account as last time".
        account_id: None,
        usage_resets_available: None,
        windows: vec![premium_requests(body)?],
    })
}

fn credential() -> Result<String, String> {
    for name in ["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(token) = std::env::var(name) {
            if !token.trim().is_empty() {
                return Ok(token);
            }
        }
    }
    if let Some(token) = copilot_cli_credential() {
        return Ok(token);
    }
    for executable in gh_candidates() {
        let Ok(output) = Command::new(executable)
            .args(["auth", "token", "--hostname", "github.com"])
            .output()
        else {
            continue;
        };
        let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if output.status.success() && !token.is_empty() {
            return Ok(token);
        }
    }
    Err(format!(
        "{NOT_SIGNED_IN}: no Copilot or GitHub sign-in found — run `copilot login` or `gh auth login`"
    ))
}

fn copilot_cli_credential() -> Option<String> {
    let config = copilot_config_path()?;
    let account = config_account(&config);
    let output = if cfg!(target_os = "macos") {
        Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-s",
                "copilot-cli",
                "-a",
                account.as_deref()?,
                "-w",
            ])
            .output()
            .ok()
    } else if cfg!(target_os = "linux") {
        Command::new("secret-tool")
            .args([
                "lookup",
                "service",
                "copilot-cli",
                "username",
                account.as_deref()?,
            ])
            .output()
            .ok()
    } else {
        None
    };
    output
        .filter(|output| output.status.success())
        .and_then(|output| {
            let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
            (!token.is_empty()).then_some(token)
        })
        .or_else(|| config_credential(&config))
}

fn copilot_config_path() -> Option<PathBuf> {
    std::env::var_os("COPILOT_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".copilot")))
        .map(|home| home.join("config.json"))
}

fn config(path: &Path) -> Option<Value> {
    let document = std::fs::read_to_string(path).ok()?;
    let document = document
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    serde_json::from_str(&document).ok()
}

fn config_account(path: &Path) -> Option<String> {
    let config = config(path)?;
    let user = config.get("lastLoggedInUser")?;
    Some(format!(
        "{}:{}",
        user.get("host")?.as_str()?,
        user.get("login")?.as_str()?
    ))
}

fn config_credential(path: &Path) -> Option<String> {
    let config = config(path)?;
    config
        .get("copilotTokens")?
        .get(config_account(path)?)?
        .as_str()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

fn gh_candidates() -> Vec<&'static Path> {
    let mut candidates = Vec::new();
    if cfg!(target_os = "macos") {
        candidates.push(Path::new("/opt/homebrew/bin/gh"));
        candidates.push(Path::new("/usr/local/bin/gh"));
    }
    candidates.push(Path::new("gh"));
    candidates
}

fn fetch(token: &str) -> Result<Value, String> {
    let response = ureq::get(LIMITS_URL)
        .set("Authorization", &format!("token {token}"))
        .set("Accept", "application/json")
        .set("Editor-Version", "vscode/1.96.2")
        .set("Editor-Plugin-Version", "copilot-chat/0.26.7")
        .set("User-Agent", "GitHubCopilotChat/0.26.7")
        .set("X-Github-Api-Version", "2025-04-01")
        .timeout(Duration::from_secs(15))
        .call();
    match response {
        Ok(response) => response
            .into_string()
            .map_err(|error| error.to_string())
            .and_then(|body| serde_json::from_str(&body).map_err(|error| error.to_string()))
            .map_err(|error| format!("the vendor's answer could not be read: {error}")),
        Err(ureq::Error::Status(401 | 403, _)) => Err(format!(
            "{NOT_SIGNED_IN}: GitHub rejected the saved sign-in (401/403) — run `copilot login` or `gh auth login`"
        )),
        Err(ureq::Error::Status(status, _)) => Err(format!("the vendor answered {status}")),
        Err(error) => Err(format!("could not reach the vendor: {error}")),
    }
}

fn premium_requests(body: &Value) -> Result<WindowExport, String> {
    let limit = body
        .pointer("/quota_snapshots/premium_interactions")
        .and_then(Value::as_object)
        .ok_or_else(|| "the vendor's answer carried no Premium Requests Limit".to_string())?;
    if limit.get("unlimited").and_then(Value::as_bool) != Some(false) {
        return Err("the vendor's Premium Requests Limit is not finite".to_string());
    }
    let entitlement = limit
        .get("entitlement")
        .and_then(Value::as_f64)
        .filter(|value| *value > 0.0)
        .ok_or_else(|| "the vendor's Premium Requests entitlement is invalid".to_string())?;
    let remaining = ["quota_remaining", "remaining"]
        .iter()
        .find_map(|field| limit.get(*field).and_then(Value::as_f64))
        .filter(|value| *value >= 0.0 && *value <= entitlement)
        .ok_or_else(|| "the vendor's Premium Requests remainder is invalid".to_string())?;
    let resets_at = limit
        .get("quota_reset_at")
        .and_then(|value| {
            value
                .as_i64()
                .filter(|epoch| *epoch > 0)
                .or_else(|| value.as_str().and_then(iso_to_epoch))
        })
        .or_else(|| {
            body.get("quota_reset_date_utc")
                .and_then(Value::as_str)
                .and_then(iso_to_epoch)
        })
        .filter(|value| *value > 0)
        .ok_or_else(|| "the vendor's Premium Requests reset is invalid".to_string())?;

    Ok(WindowExport {
        key: "premium_requests".to_string(),
        window_minutes: None,
        used_pct: (entitlement - remaining) * 100.0 / entitlement,
        resets_at,
        evidence: WindowEvidence::default(),
    })
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_complete_finite_premium_limit_becomes_an_export() {
        let body = serde_json::json!({
            "copilot_plan": "individual_pro",
            "quota_reset_date_utc": "2026-09-01T00:00:00Z",
            "quota_snapshots": {
                "premium_interactions": {
                    "entitlement": 300,
                    "remaining": 225,
                    "quota_remaining": 225.0,
                    "unlimited": false,
                    "quota_reset_at": 0
                },
                "chat": {"unlimited": true},
                "completions": {"unlimited": true}
            }
        });
        let export = limits_export(&body, 1_786_579_200).unwrap();
        assert_eq!(export.plan.as_deref(), Some("individual_pro"));
        let window = &export.windows[0];
        assert_eq!(window.key, "premium_requests");
        assert_eq!(window.window_minutes, None);
        assert_eq!(window.used_pct, 25.0);
        assert!(limits_export(
            &serde_json::json!({
                "copilot_plan": "individual_pro",
                "quota_snapshots": {"premium_interactions": {
                    "entitlement": 300, "remaining": 225, "unlimited": true,
                    "quota_reset_at": "2026-09-01T00:00:00Z"
                }}
            }),
            1_786_579_200
        )
        .is_err());
        assert!(limits_export(
            &serde_json::json!({
                "quota_reset_date_utc": "2026-09-01T00:00:00Z",
                "quota_snapshots": {"premium_interactions": {
                    "entitlement": 300, "remaining": 225, "unlimited": false
                }}
            }),
            1_786_579_200
        )
        .is_err());
    }

    #[test]
    fn plaintext_cli_fallback_selects_the_last_logged_in_user() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        std::fs::write(
            &path,
            r#"// Copilot CLI settings
            {
              "lastLoggedInUser": {"host": "https://github.com", "login": "alice"},
              "copilotTokens": {
                "https://github.com:bob": "wrong",
                "https://github.com:alice": "saved-token"
              }
            }"#,
        )
        .unwrap();
        assert_eq!(
            config_account(&path).as_deref(),
            Some("https://github.com:alice")
        );
        assert_eq!(config_credential(&path).as_deref(), Some("saved-token"));
    }
}
