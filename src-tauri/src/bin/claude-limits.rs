//! claude-limits — companion tool, deliberately NOT part of the scan.
//!
//! Claude's Limit state lives only with its vendor, so a live gauge means
//! presenting the sign-in Claude Code already stores to `api.anthropic.com`.
//! ADR-0013 forbids the app to handle credentials or fetch private usage, and
//! ADR-0019 moves the fetch out here rather than spending that prohibition to
//! save a process spawn: the always-running process provably never touches a
//! vendor credential, checkable by grep rather than promised by code review.
//!
//! Four bounds, all load-bearing (ADR-0019):
//!
//! 1. It **reads** the credential document and never writes, refreshes, or
//!    spends it. `refreshToken` is not even modelled here — the cheapest
//!    possible guarantee. (tokscale rewrote the credential file, dropped
//!    fields, and left Claude Code reporting "Not logged in"; an app that
//!    replayed a dead token family tripped Anthropic's token-theft detection,
//!    which revokes the whole tree including the live session.)
//! 2. It fetches Limit state only, never usage.
//! 3. It runs only because a person asked — the app calls it on page open or
//!    manual Refresh, with a floor between calls, never on a timer.
//! 4. A 401/403 says so and points at the Source's own CLI. It never tries to
//!    repair a session it does not own.
//!
//! The credential read goes through `/usr/bin/security`, and that detail is the
//! whole reason this does not prompt: Claude Code creates the keystore item by
//! shelling out to `security add-generic-password` with neither `-A` nor `-T`,
//! so the item's access-control list trusts `/usr/bin/security` and nothing
//! else. Any process running that same Apple binary satisfies the ACL, and this
//! app's own signature never enters the evaluation — which is why an unsigned
//! release cannot lose a grant it never had. An in-process keystore API
//! (`SecItem`, the `keyring` crate) is the *governed* path and re-prompts per
//! release on ad-hoc-signed builds; it must stay out of this codebase.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use sha2::{Digest, Sha256};

use tokenledger_lib::limits_artifact::{self, LimitsExport, WindowExport, NOT_SIGNED_IN};
use tokenledger_lib::time::iso_to_epoch;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_BETA: &str = "oauth-2025-04-20";
/// `errSecItemNotFound` — the honest "not signed in". Every other non-zero exit
/// is a failure and must never be reported as an absence: telling someone they
/// are not signed in when the keystore was merely locked sends them to
/// re-authenticate a login they already have.
const EXIT_ITEM_NOT_FOUND: i32 = 44;
/// The usage endpoint needs this scope. An absent or empty list is unknown
/// rather than refused — credentials written before the field existed have none.
const REQUIRED_SCOPE: &str = "user:profile";

fn main() {
    match run() {
        // stdout echoes the Artifact for a hand run and for inspection. It is not
        // the ingest path — the app reads the file, per ADR-0019 — so this is an
        // echo, not a contract.
        Ok(report) => println!("{report}"),
        Err(err) => {
            eprintln!("claude-limits: {err}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<String, String> {
    let credential = credential()?;
    if let Some(scopes) = &credential.scopes {
        if !scopes.is_empty() && !scopes.iter().any(|s| s == REQUIRED_SCOPE) {
            return Err(format!(
                "{NOT_SIGNED_IN}: this Claude login cannot read Limits (no {REQUIRED_SCOPE} scope)"
            ));
        }
    }

    let body = fetch(&credential.access_token)?;
    let export = LimitsExport {
        schema: limits_artifact::SCHEMA,
        source: "claude".to_string(),
        fetched_at: now(),
        plan: credential.plan,
        windows: windows(&body),
    };

    // The durable Artifact is how the reading reaches the app at all — the scan
    // and the command both read the file, never this process's stdout (ADR-0019).
    // So a failed write is a failed run: exiting 0 here would report success
    // having delivered nothing, and the card would show an absence rather than
    // the error that caused it. A hand run with no directory named just prints.
    if let Some(dir) = std::env::var_os("TOKENLEDGER_LIMITS_DIR") {
        limits_artifact::write(&PathBuf::from(dir), &export)
            .map_err(|err| format!("could not write the export: {err}"))?;
    }
    serde_json::to_string(&export).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// The credential document
// ---------------------------------------------------------------------------

/// Only what a read-only Limits fetch needs. `refreshToken` is absent by design:
/// this tool has no use for a credential it must not spend.
struct Credential {
    access_token: String,
    scopes: Option<Vec<String>>,
    plan: Option<String>,
}

fn credential() -> Result<Credential, String> {
    // The keystore route first where there is one, then the credential file —
    // the sole source elsewhere, and on macOS a fallback that must lose to a
    // valid keystore read.
    let mut trouble: Option<String> = None;
    if cfg!(target_os = "macos") {
        for service in service_candidates() {
            match keystore_read(&service) {
                Ok(Some(raw)) => return parse_credential(&raw),
                Ok(None) => {}
                // Hold the failure: a later candidate may still be a clean hit,
                // and if none is, this is what the card must say.
                Err(err) => trouble = Some(err),
            }
        }
    }
    match credential_file().and_then(|path| std::fs::read_to_string(path).ok()) {
        Some(raw) => parse_credential(&raw),
        None => Err(trouble.unwrap_or_else(|| {
            format!("{NOT_SIGNED_IN}: no Claude Code sign-in found on this computer")
        })),
    }
}

/// The item's service name is *built* by Claude Code, and the derivation has
/// changed across versions — one machine in the field holds it under a hash of
/// the default config dir with no override set at all. So probe the known
/// spellings and take the first hit rather than computing one and trusting it.
///
/// Never with `-a`: the account name falls back through `$USER`, and a real
/// account containing `@` breaks the lookup (a filed bug against exactly that).
/// Service-only resolution stays correct if the derivation changes again.
fn service_candidates() -> Vec<String> {
    let mut names = vec!["Claude Code-credentials".to_string()];
    let dirs = [
        std::env::var_os("CLAUDE_SECURESTORAGE_CONFIG_DIR").map(PathBuf::from),
        std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from),
        dirs::home_dir().map(|home| home.join(".claude")),
    ];
    for dir in dirs.into_iter().flatten() {
        // ponytail: hashes the path's own bytes. Claude Code hashes its NFC
        // normalization, which differs only for a decomposed non-ASCII home
        // directory; normalize here if one ever turns up.
        let digest = Sha256::digest(dir.to_string_lossy().as_bytes());
        let suffix: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
        let candidate = format!("Claude Code-credentials-{suffix}");
        if !names.contains(&candidate) {
            names.push(candidate);
        }
    }
    names
}

/// `Ok(None)` is "this service name holds nothing" — try the next one. `Err` is a
/// failure to read, classified with Claude Code's own stderr taxonomy so a locked
/// keystore says it is locked.
fn keystore_read(service: &str) -> Result<Option<String>, String> {
    let output = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", service, "-w"])
        .output()
        .map_err(|e| format!("could not run the credential reader: {e}"))?;

    if output.status.success() {
        // Never logged, here or anywhere.
        return Ok(Some(String::from_utf8_lossy(&output.stdout).trim().to_string()));
    }
    if output.status.code() == Some(EXIT_ITEM_NOT_FOUND) {
        return Ok(None);
    }

    let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
    let reason = if stderr.contains("locked") || stderr.contains("unlock") {
        "the login keystore is locked — unlock it and check again"
    } else if stderr.contains("interaction is not allowed") || stderr.contains("no user interaction")
    {
        "the credential store refused to answer without user interaction"
    } else if stderr.contains("cancel") {
        "the credential read was cancelled"
    } else if stderr.contains("authorization")
        || stderr.contains("authentication")
        || stderr.contains("name or passphrase")
    {
        "the credential store refused authorization"
    } else {
        "the credential store could not be read"
    };
    Err(format!(
        "{reason} (security find-generic-password exited {})",
        output.status.code().map(|c| c.to_string()).unwrap_or_else(|| "abnormally".into()),
    ))
}

fn credential_file() -> Option<PathBuf> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")))?;
    let path = dir.join(".credentials.json");
    path.is_file().then_some(path)
}

fn parse_credential(raw: &str) -> Result<Credential, String> {
    let v: Value = serde_json::from_str(raw)
        .map_err(|_| "the stored Claude Code sign-in could not be read".to_string())?;
    let oauth = v.get("claudeAiOauth").unwrap_or(&v);
    let access_token = oauth
        .get("accessToken")
        .and_then(|t| t.as_str())
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| format!("{NOT_SIGNED_IN}: the stored sign-in carries no access token"))?;
    Ok(Credential {
        access_token: access_token.to_string(),
        scopes: oauth.get("scopes").and_then(|s| s.as_array()).map(|a| {
            a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()
        }),
        // The plan pill. `rateLimitTier` is the Limits-relevant one; the
        // subscription name is the fallback where it is absent.
        plan: ["rateLimitTier", "subscriptionType"]
            .iter()
            .find_map(|k| oauth.get(*k).and_then(|p| p.as_str()))
            .map(str::to_string),
    })
}

// ---------------------------------------------------------------------------
// The fetch
// ---------------------------------------------------------------------------

fn fetch(access_token: &str) -> Result<Value, String> {
    let response = ureq::get(USAGE_URL)
        .set("Authorization", &format!("Bearer {access_token}"))
        .set("anthropic-beta", OAUTH_BETA)
        .timeout(Duration::from_secs(15))
        .call();
    match response {
        Ok(response) => response
            .into_string()
            .map_err(|e| e.to_string())
            .and_then(|body| serde_json::from_str::<Value>(&body).map_err(|e| e.to_string()))
            .map_err(|e| format!("the vendor's answer could not be read: {e}")),
        // Bound 4: report it and point at the Source's own CLI. Nothing is
        // written or refreshed in response.
        Err(ureq::Error::Status(401 | 403, _)) => Err(format!(
            "{NOT_SIGNED_IN}: Claude rejected the saved sign-in (401/403)"
        )),
        Err(ureq::Error::Status(code, _)) => Err(format!("the vendor answered {code}")),
        Err(err) => Err(format!("could not reach the vendor: {err}")),
    }
}

/// The response's named windows. Discovered from the keys themselves, never a
/// fixed list: a per-model window nobody has seen yet still has to render. Any
/// object carrying a numeric `utilization` is a window, which also survives the
/// figures being nested one level down.
fn windows(body: &Value) -> Vec<WindowExport> {
    let mut out = Vec::new();
    collect_windows(body, &mut out);
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

fn collect_windows(node: &Value, out: &mut Vec<WindowExport>) {
    let Some(object) = node.as_object() else { return };
    for (key, value) in object {
        match window(key, value) {
            Some(window) => out.push(window),
            // Not a window itself: it may still contain them.
            None => collect_windows(value, out),
        }
    }
}

fn window(key: &str, value: &Value) -> Option<WindowExport> {
    let used_pct = value.get("utilization").and_then(|u| u.as_f64())?;
    // A window with no reset instant has not started — a session window with no
    // active session. It reports no epoch, so there is nothing to record.
    let resets_at = value
        .get("resets_at")
        .and_then(|r| r.as_i64().or_else(|| r.as_str().and_then(iso_to_epoch)))?;
    Some(WindowExport {
        key: key.to_string(),
        window_minutes: window_minutes(key),
        used_pct,
        resets_at,
    })
}

/// The window's length, read off the key that names it. The response states the
/// reset instant but not the duration, and the tick needs both — so a key that
/// names no duration yields None and the card draws no tick rather than
/// inventing an axis.
fn window_minutes(key: &str) -> Option<i64> {
    if key == "five_hour" {
        return Some(300);
    }
    if key == "seven_day" || key.starts_with("seven_day_") {
        return Some(10080);
    }
    None
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

    #[test]
    fn windows_are_discovered_from_the_response_keys() {
        // The per-model set is never a fixed list: `seven_day_zephyr` is a key
        // nobody has seen, and it still has to render.
        let body: Value = serde_json::from_str(
            r#"{"five_hour":{"utilization":18.0,"resets_at":"2026-08-12T03:05:00Z"},
                "seven_day":{"utilization":59.0,"resets_at":"2026-08-16T00:00:00Z"},
                "seven_day_zephyr":{"utilization":37.0,"resets_at":"2026-08-16T00:00:00Z"},
                "account_uuid":"not-a-window"}"#,
        )
        .unwrap();

        let found = windows(&body);
        assert_eq!(
            found.iter().map(|w| w.key.as_str()).collect::<Vec<_>>(),
            vec!["five_hour", "seven_day", "seven_day_zephyr"],
        );
        assert_eq!(found[0].window_minutes, Some(300));
        assert_eq!(found[0].used_pct, 18.0);
        assert_eq!(found[0].resets_at, 1_786_503_900);
        assert_eq!(found[2].window_minutes, Some(10080), "a model window is a weekly one");
    }

    #[test]
    fn a_window_with_no_reset_instant_is_not_recorded() {
        // A session window with no active session reports no epoch.
        let body: Value =
            serde_json::from_str(r#"{"five_hour":{"utilization":0.0,"resets_at":null}}"#).unwrap();
        assert!(windows(&body).is_empty());
    }

    #[test]
    fn windows_nested_one_level_down_are_still_found() {
        let body: Value = serde_json::from_str(
            r#"{"rate_limits":{"five_hour":{"utilization":4.0,"resets_at":1786503900}}}"#,
        )
        .unwrap();
        assert_eq!(windows(&body).len(), 1);
    }

    #[test]
    fn a_key_naming_no_duration_yields_no_axis() {
        assert_eq!(window_minutes("five_hour"), Some(300));
        assert_eq!(window_minutes("seven_day"), Some(10080));
        assert_eq!(window_minutes("seven_day_opus"), Some(10080));
        assert_eq!(window_minutes("monthly_something"), None);
    }

    #[test]
    fn the_credential_is_read_for_a_token_scopes_and_a_plan_and_nothing_else() {
        // Deliberately destructured rather than unwrapped: `Credential` carries no
        // Debug, so a token can never reach a log or a panic message by accident.
        let Ok(credential) = parse_credential(
            r#"{"claudeAiOauth":{"accessToken":"sk-tok","refreshToken":"sk-refresh",
                "expiresAt":1786492800,"scopes":["user:inference","user:profile"],
                "subscriptionType":"max","rateLimitTier":"Team 5x"}}"#,
        ) else {
            panic!("a well-formed credential document must parse");
        };
        assert_eq!(credential.access_token, "sk-tok");
        assert_eq!(credential.plan.as_deref(), Some("Team 5x"));
        assert_eq!(
            credential.scopes.as_deref(),
            Some(&["user:inference".to_string(), "user:profile".to_string()][..]),
        );
        // The refresh token has no field to land in — the cheapest possible
        // guarantee that it is never spent (ADR-0019 bound 1).
        assert!(!std::any::type_name::<Credential>().contains("Refresh"));
    }

    #[test]
    fn a_document_with_no_token_reads_as_not_signed_in() {
        let trouble = |raw: &str| parse_credential(raw).err().expect("must not parse");
        assert!(
            trouble(r#"{"claudeAiOauth":{"accessToken":"  "}}"#).starts_with(NOT_SIGNED_IN),
        );
        // The app classifies on this prefix, so a malformed document must NOT
        // borrow it: an unreadable file is a failure, not an absence.
        assert!(!trouble("{{{").starts_with(NOT_SIGNED_IN));
    }

    #[test]
    fn the_default_config_dir_is_among_the_probed_service_names() {
        // #423 is a real machine holding the item under a hash of the DEFAULT
        // config dir with no override set, which the current derivation would
        // not produce — so the plain name alone is not enough.
        let names = service_candidates();
        assert_eq!(names[0], "Claude Code-credentials");
        assert!(names.len() > 1, "the hashed spellings must be probed too");
        assert!(names[1..].iter().all(|n| n.starts_with("Claude Code-credentials-")));
        // No `-a` anywhere: an account containing `@` breaks that lookup.
        assert!(names.iter().all(|n| !n.contains("-a ")));
    }

}
