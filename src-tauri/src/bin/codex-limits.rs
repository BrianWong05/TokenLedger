//! codex-limits — companion tool, deliberately NOT part of the scan.
//!
//! Codex writes a Limit Reading into its logs on every request, and the scan
//! captures those passively — but between requests the figures can only age.
//! A current reading on demand means presenting the sign-in the Codex CLI
//! already stores to its vendor's usage endpoint, and ADR-0019 puts that fetch
//! out here rather than in the app, for the same reason as `claude-limits`:
//! the always-running process provably never touches a vendor credential,
//! checkable by grep rather than promised by code review.
//!
//! The same four bounds apply (ADR-0019):
//!
//! 1. It **reads** `auth.json` and never writes, refreshes, or spends it.
//!    `refresh_token` is not even modelled here. An expired access token is
//!    reported as "not signed in" and pointed at the Source's own CLI — running
//!    `codex` refreshes it; this tool never does.
//! 2. It fetches Limit state only, never usage or content.
//! 3. It runs only because a person asked — page open or manual Refresh, with
//!    the app's floor between calls, never on a timer.
//! 4. A 401/403 says so and points at the CLI. It never repairs a session it
//!    does not own.
//!
//! The endpoint is the one all four prior-art projects read
//! (docs/research/codex-rate-limits.md): `GET chatgpt.com/backend-api/wham/usage`,
//! authenticated with the ChatGPT OAuth access token plus the account id. Its
//! payload is shaped differently from the in-log `rate_limits` block, so the
//! parser here tolerates both spellings of each field and converts everything
//! to the same Reading the log ingest produces — keyed by duration through the
//! shared `window_key` grammar, so a live reading and a logged reading of the
//! same window land in the same series.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use tokenledger_lib::limits_artifact::{
    self, window_key, LimitsExport, WindowEvidence, WindowExport, NOT_SIGNED_IN,
};

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

fn main() {
    match run() {
        // stdout echoes the Artifact for a hand run and for inspection. It is not
        // the ingest path — the app reads the file, per ADR-0019.
        Ok(report) => println!("{report}"),
        Err(err) => {
            eprintln!("codex-limits: {err}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<String, String> {
    let credential = credential()?;
    let fetched_at = now();
    let body = fetch(&credential)?;

    let windows = windows(&body, fetched_at);
    if windows.is_empty() {
        // Parsed fine but nothing matched the window shape: the payload moved.
        // Report its structure — keys only, never values — so the drift can be
        // diagnosed from the error line without a credential in anyone's hands.
        return Err(format!(
            "the vendor's answer carried no recognisable window (fields: {})",
            field_names(&body).join(", ")
        ));
    }

    let export = LimitsExport {
        schema: limits_artifact::SCHEMA,
        source: "codex".to_string(),
        fetched_at,
        plan: body
            .get("plan_type")
            .and_then(|p| p.as_str())
            .map(str::to_string),
        // One meter answers this endpoint — the same rate_limits the rollouts
        // snapshot — so the regime is the constant the log adapter documents,
        // spelled identically or a Series would split on the difference.
        metering_regime: Some("codex:rate_limits".to_string()),
        // The credential document's own opaque id, already sent as the
        // `chatgpt-account-id` header on this very fetch: the answer is for
        // this account, and saying so is what lets a Reading anchor evidence.
        // Never the token, never anything reversible.
        account_id: credential.account_id.clone(),
        usage_resets_available: usage_resets_available(&body),
        windows,
    };

    // A failed write is a failed run, for the same reason as claude-limits: the
    // Artifact is how the reading reaches the app at all.
    if let Some(dir) = std::env::var_os("TOKENLEDGER_LIMITS_DIR") {
        limits_artifact::write(&PathBuf::from(dir), &export)
            .map_err(|err| format!("could not write the export: {err}"))?;
    }
    serde_json::to_string(&export).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// The credential document
// ---------------------------------------------------------------------------

/// Only what a read-only fetch needs. `refresh_token` and `id_token` have no
/// fields to land in — this tool has no use for a credential it must not spend.
struct Credential {
    access_token: String,
    account_id: Option<String>,
}

fn credential() -> Result<Credential, String> {
    let path = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
        .map(|dir| dir.join("auth.json"))
        .ok_or_else(|| "no home directory to look for a Codex sign-in under".to_string())?;
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(_) => {
            return Err(format!(
                "{NOT_SIGNED_IN}: no Codex sign-in found on this computer"
            ))
        }
    };
    parse_credential(&raw)
}

fn parse_credential(raw: &str) -> Result<Credential, String> {
    let v: Value = serde_json::from_str(raw)
        .map_err(|_| "the stored Codex sign-in could not be read".to_string())?;
    let tokens = v.get("tokens").unwrap_or(&Value::Null);
    let access_token = tokens
        .get("access_token")
        .and_then(|t| t.as_str())
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| format!("{NOT_SIGNED_IN}: the stored sign-in carries no access token"))?;
    Ok(Credential {
        access_token: access_token.to_string(),
        account_id: tokens
            .get("account_id")
            .and_then(|a| a.as_str())
            .map(str::to_string),
    })
}

// ---------------------------------------------------------------------------
// The fetch
// ---------------------------------------------------------------------------

fn fetch(credential: &Credential) -> Result<Value, String> {
    let mut request = ureq::get(USAGE_URL)
        .set("Authorization", &format!("Bearer {}", credential.access_token))
        .timeout(Duration::from_secs(15));
    if let Some(account) = &credential.account_id {
        request = request.set("chatgpt-account-id", account);
    }
    match request.call() {
        Ok(response) => response
            .into_string()
            .map_err(|e| e.to_string())
            .and_then(|body| serde_json::from_str::<Value>(&body).map_err(|e| e.to_string()))
            .map_err(|e| format!("the vendor's answer could not be read: {e}")),
        // Bound 4: report it and point at the Source's own CLI. Running `codex`
        // refreshes the token; this tool never does.
        Err(ureq::Error::Status(401 | 403, _)) => Err(format!(
            "{NOT_SIGNED_IN}: Codex rejected the saved sign-in (401/403) — run `codex` once to renew it"
        )),
        Err(ureq::Error::Status(code, _)) => Err(format!("the vendor answered {code}")),
        Err(err) => Err(format!("could not reach the vendor: {err}")),
    }
}

/// The response's windows, discovered by shape rather than by position: any
/// object carrying a used percentage plus a duration is a window, wherever it
/// sits. Arrays are deliberately NOT walked — `additional_rate_limits[]`
/// carries other limit families (per-model pools), and keying those by
/// duration would collide with the main family's windows, the same trap the
/// log ingest's `limit_id == "codex"` filter guards against.
fn windows(body: &Value, fetched_at: i64) -> Vec<WindowExport> {
    let mut out = Vec::new();
    collect_windows(body, fetched_at, &mut out);
    out.sort_by(|a, b| a.window_minutes.cmp(&b.window_minutes));
    out
}

fn usage_resets_available(body: &Value) -> Option<u64> {
    body.get("rate_limit_reset_credits")?
        .get("available_count")?
        .as_u64()
}

fn collect_windows(node: &Value, fetched_at: i64, out: &mut Vec<WindowExport>) {
    let Some(object) = node.as_object() else { return };
    match window(object, fetched_at) {
        Some(window) => out.push(window),
        None => {
            for value in object.values() {
                collect_windows(value, fetched_at, out);
            }
        }
    }
}

fn window(object: &serde_json::Map<String, Value>, fetched_at: i64) -> Option<WindowExport> {
    // The in-log block spells it `used_percent`; the wham payload has been seen
    // with both that and `used_percentage`. `utilization` is Claude's word for
    // the same figure — accepted so a converged payload still parses.
    let used_pct = ["used_percent", "used_percentage", "utilization"]
        .iter()
        .find_map(|k| object.get(*k).and_then(|v| v.as_f64()))?;
    let minutes = object
        .get("limit_window_seconds")
        .and_then(|v| v.as_i64())
        .map(|s| s / 60)
        .or_else(|| object.get("window_minutes").and_then(|v| v.as_i64()))?;
    // Absolute epoch where given; the wham schema also carries the relative
    // remaining time, which anchors to the fetch instant.
    let resets_at = ["reset_at", "resets_at"]
        .iter()
        .find_map(|k| object.get(*k).and_then(|v| v.as_i64()))
        .or_else(|| {
            ["reset_after_seconds", "resets_in_seconds"]
                .iter()
                .find_map(|k| object.get(*k).and_then(|v| v.as_i64()))
                .map(|rel| fetched_at + rel)
        })?;
    let key = window_key(minutes);
    Some(WindowExport {
        // The documented one-to-one mapping, in the same grammar the log
        // adapter writes: every window reaching here is the main `rate_limit`
        // block's (`collect_windows` walls the additional families off), and
        // that block is the codex entitlement itself — the one whose snapshots
        // the rollouts carry with `limit_id == "codex"`. One meter, described
        // twice; the identity is shared, not guessed.
        evidence: WindowEvidence {
            limit_id: Some(format!("codex:{key}")),
            model_scope: Some("all".to_string()),
        },
        key,
        window_minutes: Some(minutes),
        used_pct,
        resets_at,
    })
}

/// Top-level and one-deep field names — structure only, for the no-window error.
fn field_names(body: &Value) -> Vec<String> {
    let Some(object) = body.as_object() else {
        return vec!["<not an object>".to_string()];
    };
    let mut names = Vec::new();
    for (key, value) in object {
        match value.as_object() {
            Some(inner) => {
                let inner_keys: Vec<&str> = inner.keys().map(String::as_str).collect();
                names.push(format!("{key}{{{}}}", inner_keys.join(",")));
            }
            None => names.push(key.clone()),
        }
    }
    names
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The wham shape as prior art documents it (docs/research/codex-rate-limits.md):
    // windows under `rate_limit`, durations in seconds, and both the absolute and
    // relative reset spellings in the schema.
    const WHAM: &str = r#"{
        "plan_type": "plus",
        "rate_limit": {
            "primary_window": {"used_percent": 42.0, "limit_window_seconds": 18000, "reset_at": 1786517999},
            "secondary_window": {"used_percent": 65.0, "limit_window_seconds": 604800, "reset_after_seconds": 350000}
        },
        "additional_rate_limits": [
            {"limit_name": "gpt-5.3-codex-spark",
             "rate_limit": {"primary_window": {"used_percent": 10.0, "limit_window_seconds": 604800, "reset_at": 1786885199}}}
        ],
        "rate_limit_reset_credits": {"available_count": 1},
        "credits": {"has_credits": false, "unlimited": false, "balance": "0"}
    }"#;

    #[test]
    fn windows_are_keyed_by_duration_through_the_shared_grammar() {
        let body: Value = serde_json::from_str(WHAM).unwrap();
        let found = windows(&body, 1_786_492_800);

        assert_eq!(found.len(), 2, "the main family's two windows and nothing else");
        assert_eq!(found[0].key, "w300", "18000s snaps to the same key the log ingest uses");
        assert_eq!(found[0].used_pct, 42.0);
        // The same identity grammar the log adapter writes, or a live Reading
        // and a logs Reading of one window would start two Series.
        assert_eq!(found[0].evidence.limit_id.as_deref(), Some("codex:w300"));
        assert_eq!(found[1].evidence.limit_id.as_deref(), Some("codex:w10080"));
        assert_eq!(found[0].evidence.model_scope.as_deref(), Some("all"));
        assert_eq!(found[0].resets_at, 1_786_517_999, "absolute reset taken as-is");
        assert_eq!(found[1].key, "w10080");
        assert_eq!(
            found[1].resets_at,
            1_786_492_800 + 350_000,
            "a relative reset anchors to the fetch instant",
        );
    }

    #[test]
    fn additional_limit_families_are_never_walked() {
        // Keying a per-model pool by duration would collide with the main
        // family's weekly window — the same trap the log ingest's
        // `limit_id == "codex"` filter guards against.
        let body: Value = serde_json::from_str(WHAM).unwrap();
        assert!(
            windows(&body, 0).iter().all(|w| w.used_pct != 10.0),
            "the spark pool's window must not enter the codex series",
        );
    }

    #[test]
    fn usage_resets_are_source_state_and_preserve_zero() {
        let body: Value = serde_json::from_str(WHAM).unwrap();
        assert_eq!(usage_resets_available(&body), Some(1));
        assert_eq!(usage_resets_available(&serde_json::json!({})), None);
        assert_eq!(
            usage_resets_available(&serde_json::json!({
                "rate_limit_reset_credits": {"available_count": 0}
            })),
            Some(0),
        );
    }

    #[test]
    fn a_window_without_a_duration_or_reset_is_no_window() {
        let body: Value = serde_json::from_str(
            r#"{"rate_limit":{"primary_window":{"used_percent":50.0},
                "secondary_window":{"limit_window_seconds":604800,"reset_at":1}}}"#,
        )
        .unwrap();
        assert!(windows(&body, 0).is_empty(), "neither half-shape can be keyed or placed");
    }

    #[test]
    fn an_unrecognised_payload_reports_structure_only() {
        let body: Value =
            serde_json::from_str(r#"{"totally":{"new":"shape"},"version":2}"#).unwrap();
        assert!(windows(&body, 0).is_empty());
        assert_eq!(field_names(&body), vec!["totally{new}", "version"]);
    }

    #[test]
    fn the_credential_is_read_for_a_token_and_account_and_nothing_else() {
        let Ok(credential) = parse_credential(
            r#"{"OPENAI_API_KEY": null, "auth_mode": "chatgpt", "last_refresh": "2026-08-09T07:25:00Z",
                "tokens": {"access_token": "eyJ-tok", "account_id": "acc-1",
                           "id_token": "eyJ-id", "refresh_token": "eyJ-refresh"}}"#,
        ) else {
            panic!("a well-formed auth.json must parse");
        };
        assert_eq!(credential.access_token, "eyJ-tok");
        assert_eq!(credential.account_id.as_deref(), Some("acc-1"));
        // The refresh token has no field to land in (ADR-0019 bound 1).
        assert!(!std::any::type_name::<Credential>().contains("Refresh"));
    }

    #[test]
    fn a_document_with_no_token_reads_as_not_signed_in() {
        let trouble = |raw: &str| parse_credential(raw).err().expect("must not parse");
        assert!(trouble(r#"{"tokens":{"access_token":"  "}}"#).starts_with(NOT_SIGNED_IN));
        assert!(trouble(r#"{"OPENAI_API_KEY":"sk-key"}"#).starts_with(NOT_SIGNED_IN));
        // A malformed document is a failure, not an absence.
        assert!(!trouble("{{{").starts_with(NOT_SIGNED_IN));
    }
}
