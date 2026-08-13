//! grok-limits — companion tool, deliberately NOT part of the scan.
//!
//! Grok Build already logs every credits reading it fetches to
//! `~/.grok/logs/unified.jsonl`, and the scan captures those passively (the
//! `logs` path in `adapters/grok.rs`). This Companion is the *fresh-on-demand*
//! path openusage uses: it presents the sign-in the Grok CLI stores to xAI's
//! billing endpoint, the same request the CLI makes for its own `/usage` view.
//! Both feed `limit_readings`; between live checks the logged readings stand, so
//! this mirrors the Codex dual model (passive log capture + live Companion).
//!
//! **This Companion carries a risk the others do not, and it was accepted
//! deliberately.** xAI **rotates** refresh tokens: a refresh mints a new refresh
//! token server-side and invalidates the old one, whether or not we keep the
//! result. The build issue (#126) chose the log path precisely to avoid this,
//! and the owner has since asked for the live path anyway, informed of the cost.
//! The bounds that keep the cost as small as it can be:
//!
//! 1. **Read-only, always.** Like every other Companion, this never writes
//!    `~/.grok/auth.json` — the "never writes a credential" invariant stays
//!    greppable. A refreshed token lives only in this process's memory and is
//!    discarded on exit. The consequence of NOT writing it back, given rotation,
//!    is that a refresh here can leave the CLI's stored refresh token stale, so
//!    the next `grok` run may need `grok login` again. That is the accepted risk,
//!    and it is the same remedy the signed-out card already points at.
//! 2. **Refresh only on expiry.** A stored access token that is still valid is
//!    presented as-is and nothing is sent to the sign-in service at all — most
//!    checks cost no refresh and carry no risk.
//! 3. It fetches Limit state only, and runs only because a person asked (page
//!    open / manual Refresh), with the app's ≥60s floor between calls. That floor
//!    is also an obligation to xAI's rate limit, which the CLI shares.
//! 4. A refusal reports itself and points at the CLI (ADR-0019 bound 4); it
//!    never tries to repair a session it does not own.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use tokenledger_lib::limits_artifact::{self, grok_credit_window, LimitsExport, NOT_SIGNED_IN};
use tokenledger_lib::time::iso_to_epoch;

/// The CLI's own billing surface — the credits reading it shows in `/usage`.
/// `?format=credits` is load-bearing: it selects the modern
/// `creditUsagePercent`/`currentPeriod` shape; bare `/v1/billing` returns the
/// deprecated monthly meter. Overridable for self-hosted proxies, which the CLI
/// supports through the same variable.
const DEFAULT_BASE: &str = "https://cli-chat-proxy.grok.com/v1";
const BASE_ENV: &str = "GROK_CLI_CHAT_PROXY_BASE_URL";
/// xAI's token endpoint. The CLI discovers this from the issuer's
/// `.well-known/openid-configuration`; openusage hardcodes this host and it
/// works. ponytail: hardcoded — switch to issuer-discovery if a non-x.ai issuer
/// (`accounts.mouseion.dev`, a self-hosted IdP) ever needs refreshing.
const TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
/// Documented as required: it tells the auth middleware to validate the bearer as
/// a CLI session token. Omitting it is the likeliest cause of a spurious 401.
const TOKEN_AUTH: &str = "xai-grok-cli";

fn main() {
    match run() {
        // stdout echoes the Artifact for a hand run; the app reads the file.
        Ok(report) => println!("{report}"),
        Err(err) => {
            eprintln!("grok-limits: {err}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<String, String> {
    let credential = credential()?;
    let fetched_at = now();
    let access_token = access_token(&credential, fetched_at)?;
    let base = std::env::var(BASE_ENV)
        .ok()
        .filter(|b| !b.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BASE.to_string());
    let base = base.trim_end_matches('/');

    let billing = fetch(
        &format!("{base}/billing?format=credits"),
        &access_token,
        credential.user_id.as_deref(),
    )?;

    if std::env::args().skip(1).any(|a| a == "--shape") {
        return Ok(limits_artifact::shape(&billing));
    }

    let config = billing.get("config").unwrap_or(&billing);
    // Same mapper the scan's log ingest uses — the live `config` and the logged
    // `ctx.config` are the identical shape, so a live reading and a logged one of
    // the same window land in the same series.
    let windows = grok_credit_window(config).into_iter().collect::<Vec<_>>();
    if windows.is_empty() {
        return Err(format!(
            "the vendor's answer carried no usable credit window (fields: {})",
            field_names(config).join(", ")
        ));
    }

    let export = LimitsExport {
        schema: limits_artifact::SCHEMA,
        source: "grok".to_string(),
        fetched_at,
        // The billing response overwrites its own `subscriptionTier` from cached
        // remote settings, so the live plan label costs a second call. Non-fatal:
        // a card with no pill beats a failed check.
        plan: plan(base, &access_token, credential.user_id.as_deref()),
        // This Companion proves no metering regime yet.
        metering_regime: None,
        usage_resets_available: None,
        windows,
    };

    if let Some(dir) = std::env::var_os("TOKENLEDGER_LIMITS_DIR") {
        limits_artifact::write(&PathBuf::from(dir), &export)
            .map_err(|err| format!("could not write the export: {err}"))?;
    }
    serde_json::to_string(&export).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// The credential document
// ---------------------------------------------------------------------------

/// Only what a read-only fetch needs. The refresh token IS modelled here (unlike
/// claude/codex) because xAI's short-lived access token must sometimes be
/// refreshed — under the accepted risk in the module header.
struct Credential {
    access_token: String,
    refresh_token: String,
    /// The OAuth client the stored token belongs to; the refresh presents it.
    client_id: String,
    /// Carried on the billing request as `x-userid` where present.
    user_id: Option<String>,
    /// Epoch seconds. The stored token is used as-is while this is in the future.
    expiry: i64,
}

fn credential() -> Result<Credential, String> {
    let path = std::env::var_os("GROK_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".grok")))
        .map(|dir| dir.join("auth.json"))
        .ok_or_else(|| "no home directory to look for a Grok sign-in under".to_string())?;
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(_) => return Err(format!("{NOT_SIGNED_IN}: no Grok sign-in found on this computer")),
    };
    parse_credential(&raw)
}

/// `auth.json` is a **map** keyed by `{issuer}::{client_id}` — a machine may hold
/// several, so the entries are iterated rather than one assumed. The first entry
/// carrying an access token (`key`) is taken; its refresh token, if any, is kept
/// for the on-expiry refresh, and the client id is the entry's own
/// `oidc_client_id`, falling back to the tail of the map key.
fn parse_credential(raw: &str) -> Result<Credential, String> {
    let v: Value = serde_json::from_str(raw)
        .map_err(|_| "the stored Grok sign-in could not be read".to_string())?;
    let entries = v
        .as_object()
        .ok_or_else(|| "the stored Grok sign-in could not be read".to_string())?;

    for (map_key, entry) in entries {
        let access_token = entry.get("key").and_then(|k| k.as_str()).unwrap_or("");
        let refresh_token = entry.get("refresh_token").and_then(|r| r.as_str()).unwrap_or("");
        if access_token.trim().is_empty() {
            continue;
        }
        let client_id = entry
            .get("oidc_client_id")
            .and_then(|c| c.as_str())
            .filter(|c| !c.is_empty())
            .map(str::to_string)
            .or_else(|| map_key.rsplit("::").next().map(str::to_string))
            .unwrap_or_default();
        return Ok(Credential {
            access_token: access_token.to_string(),
            refresh_token: refresh_token.to_string(),
            client_id,
            user_id: entry.get("user_id").and_then(|u| u.as_str()).map(str::to_string),
            expiry: entry.get("expires_at").and_then(expiry_epoch).unwrap_or(0),
        });
    }
    Err(format!("{NOT_SIGNED_IN}: the stored Grok sign-in carries no access token"))
}

/// `expires_at` seen as an epoch (seconds or milliseconds) or an RFC3339 string.
fn expiry_epoch(value: &Value) -> Option<i64> {
    if let Some(n) = value.as_i64() {
        return Some(if n > 1_000_000_000_000 { n / 1000 } else { n });
    }
    value.as_str().and_then(iso_to_epoch)
}

// ---------------------------------------------------------------------------
// The token
// ---------------------------------------------------------------------------

/// True while the stored access token still has life in it — a minute of headroom
/// so one expiring in flight does not 401. Split out so the gate is testable
/// without the network.
fn stored_token_holds(credential: &Credential, now: i64) -> bool {
    credential.expiry > now + 60 && !credential.access_token.is_empty()
}

fn access_token(credential: &Credential, now: i64) -> Result<String, String> {
    if stored_token_holds(credential, now) {
        return Ok(credential.access_token.clone());
    }
    if credential.refresh_token.is_empty() {
        return Err(format!(
            "{NOT_SIGNED_IN}: the stored Grok sign-in has expired — run `grok` once to renew it"
        ));
    }
    refresh(credential)
}

/// Mint a fresh access token. Read-only in the one way that matters: the response
/// (including any rotated refresh token) lives only in memory and is discarded —
/// `auth.json` is never written. See the module header for why that leaves a real
/// but bounded risk with xAI specifically.
fn refresh(credential: &Credential) -> Result<String, String> {
    let response = ureq::post(TOKEN_URL)
        .timeout(Duration::from_secs(15))
        .send_form(&[
            ("grant_type", "refresh_token"),
            ("client_id", &credential.client_id),
            ("refresh_token", &credential.refresh_token),
        ]);
    let body: Value = match response {
        Ok(response) => response
            .into_string()
            .map_err(|e| e.to_string())
            .and_then(|body| serde_json::from_str(&body).map_err(|e| e.to_string()))
            .map_err(|e| format!("the sign-in service's answer could not be read: {e}"))?,
        Err(ureq::Error::Status(400 | 401 | 403, _)) => {
            return Err(format!(
                "{NOT_SIGNED_IN}: xAI would not renew the saved Grok sign-in — run `grok` once to sign in again"
            ))
        }
        Err(ureq::Error::Status(code, _)) => {
            return Err(format!("the sign-in service answered {code}"))
        }
        Err(err) => return Err(format!("could not reach the sign-in service: {err}")),
    };
    ["access_token", "accessToken"]
        .iter()
        .find_map(|k| body.get(*k).and_then(|t| t.as_str()))
        .filter(|t| !t.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| "the sign-in service returned no access token".to_string())
}

// ---------------------------------------------------------------------------
// The fetch
// ---------------------------------------------------------------------------

fn fetch(url: &str, access_token: &str, user_id: Option<&str>) -> Result<Value, String> {
    let mut request = ureq::get(url)
        .set("Authorization", &format!("Bearer {access_token}"))
        .set("X-XAI-Token-Auth", TOKEN_AUTH)
        .set("Accept", "application/json")
        .timeout(Duration::from_secs(15));
    if let Some(user_id) = user_id {
        request = request.set("x-userid", user_id);
    }
    // The CLI also sends `x-grok-client-version`, but it is deliberately omitted:
    // openusage omits it and works, server enforcement is unconfirmed (the
    // research could not determine it), and we have no honest version to send —
    // the CLI's own changes with each release, and a stale guess risks a spurious
    // reject. ponytail: if a 400 ever traces to it, read `~/.grok/version.json`.
    match request.call() {
        Ok(response) => response
            .into_string()
            .map_err(|e| e.to_string())
            .and_then(|body| serde_json::from_str::<Value>(&body).map_err(|e| e.to_string()))
            .map_err(|e| format!("the vendor's answer could not be read: {e}")),
        Err(ureq::Error::Status(401 | 403, _)) => Err(format!(
            "{NOT_SIGNED_IN}: xAI rejected the saved Grok sign-in (401/403) — run `grok` once to renew it"
        )),
        Err(ureq::Error::Status(code, _)) => Err(format!("the vendor answered {code}")),
        Err(err) => Err(format!("could not reach the vendor: {err}")),
    }
}

/// The plan label, from `/v1/settings` (`subscription_tier_display`, falling back
/// to `subscription_tier`). Non-fatal: a failed lookup yields no pill, never a
/// failed check.
fn plan(base: &str, access_token: &str, user_id: Option<&str>) -> Option<String> {
    let settings = fetch(&format!("{base}/settings"), access_token, user_id).ok()?;
    ["subscription_tier_display", "subscription_tier"]
        .iter()
        .find_map(|k| settings.get(*k).and_then(|t| t.as_str()))
        .filter(|t| !t.trim().is_empty())
        .map(str::to_string)
}

/// Top-level field names — structure only, for the no-window error.
fn field_names(config: &Value) -> Vec<String> {
    config
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_else(|| vec!["<not an object>".to_string()])
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

    // The `config` → window mapping is shared with the log path and tested in
    // `limits_artifact` (grok_credit_window). What is specific to this Companion —
    // the credential document and the token gate — is what is tested here.

    #[test]
    fn the_credential_is_read_from_the_first_entry_carrying_a_token() {
        // auth.json is a MAP keyed by issuer::client_id; a reader iterates it.
        let Ok(credential) = parse_credential(
            r#"{"https://auth.x.ai::client-uuid":{
                "key":"eyJ-access","refresh_token":"rt-1","oidc_client_id":"client-uuid",
                "user_id":"user-1","expires_at":1786492800,"email":"x"}}"#,
        ) else {
            panic!("a well-formed auth.json must parse");
        };
        assert_eq!(credential.access_token, "eyJ-access");
        assert_eq!(credential.refresh_token, "rt-1");
        assert_eq!(credential.client_id, "client-uuid");
        assert_eq!(credential.user_id.as_deref(), Some("user-1"));
        assert_eq!(credential.expiry, 1_786_492_800);
    }

    #[test]
    fn the_client_id_falls_back_to_the_map_key_tail() {
        let credential = parse_credential(
            r#"{"https://auth.x.ai::key-suffix-uuid":{"key":"a","refresh_token":"r"}}"#,
        )
        .unwrap();
        assert_eq!(credential.client_id, "key-suffix-uuid");
    }

    #[test]
    fn a_document_with_no_token_reads_as_not_signed_in() {
        let trouble = |raw: &str| parse_credential(raw).err().expect("must not parse");
        assert!(trouble(r#"{"iss::cid":{"key":"  "}}"#).starts_with(NOT_SIGNED_IN));
        // A malformed document is a failure, not an absence — the app classifies
        // on the prefix, and a corrupt file must not send someone to re-auth.
        assert!(!trouble("{{{").starts_with(NOT_SIGNED_IN));
    }

    #[test]
    fn a_live_token_is_used_as_is_and_only_an_expired_one_refreshes() {
        let credential = |expiry, access: &str| Credential {
            access_token: access.to_string(),
            refresh_token: "rt".to_string(),
            client_id: "cid".to_string(),
            user_id: None,
            expiry,
        };
        // Comfortably alive: presented as-is, xAI is never reached, no rotation.
        assert!(stored_token_holds(&credential(2_000, "tok"), 1_000));
        assert_eq!(access_token(&credential(2_000, "tok"), 1_000).as_deref(), Ok("tok"));
        // Expired, and within the minute of headroom, both need a refresh.
        assert!(!stored_token_holds(&credential(1_000, "tok"), 2_000));
        assert!(!stored_token_holds(&credential(1_030, "tok"), 1_000));
        // Expired with nothing to refresh → signed out, not a dead-token 401.
        let spent = Credential { refresh_token: String::new(), ..credential(1_000, "tok") };
        assert!(access_token(&spent, 2_000).unwrap_err().starts_with(NOT_SIGNED_IN));
    }

    #[test]
    fn expires_at_is_read_as_seconds_millis_or_rfc3339() {
        assert_eq!(expiry_epoch(&serde_json::json!(1_786_492_800)), Some(1_786_492_800));
        assert_eq!(expiry_epoch(&serde_json::json!(1_786_492_800_000i64)), Some(1_786_492_800));
        assert_eq!(
            expiry_epoch(&serde_json::json!("2026-08-12T00:00:00Z")),
            iso_to_epoch("2026-08-12T00:00:00Z"),
        );
    }
}
