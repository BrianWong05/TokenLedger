//! antigravity-limits — companion tool, deliberately NOT part of the scan.
//!
//! Antigravity's Limit state lives only with Google, so a live gauge means
//! presenting the sign-in Antigravity already stores to `cloudcode-pa`.
//! ADR-0019 moves that fetch out of the app for the same reason as the other
//! Companions: the always-running process provably never touches a vendor
//! credential, checkable by grep rather than promised by code review.
//!
//! **This Companion differs from `claude-limits` and `codex-limits` in exactly
//! one way, and it is deliberate.** Those two do not even model a refresh
//! token — the cheapest possible guarantee that they never spend one. This one
//! does, because Google's access tokens die in about an hour, so under that
//! literal rule an Antigravity card could show a figure only within an hour of
//! the person last running the tool. [ADR-0020] re-derives the bound from the
//! property it was protecting — never corrupt the Source's own session — and
//! finds the Google refresh grant leaves it intact: the response carries no
//! replacement refresh token, and Google's grant-eviction cap counts
//! *authorization* grants, which a refresh never creates, so no number of
//! exchanges can evict Antigravity's own token.
//!
//! The bounds that come with that permission, all load-bearing:
//!
//! 1. The exchange goes to `https://oauth2.googleapis.com/token` and nowhere
//!    else, and the Keychain item is **never written**. A stored access token
//!    that has not expired is used as-is; the exchange happens only on expiry.
//! 2. **No cache.** The minted token lives only in this process's memory —
//!    never a file, never the Keychain — so "TokenLedger never writes a Google
//!    token" stays greppable rather than promised. (If a cache is ever added it
//!    must bind to a one-way fingerprint of the refresh credential, or a logout
//!    or account switch could serve the previous account's quota.)
//! 3. The client id/secret pair is hardcoded. Google's installed-app model
//!    ships them in every copy of each client: they identify the app, they are
//!    not keys. A vendor rotating its id fails the exchange, and the card
//!    degrades to signed-out pointing at Antigravity (ADR-0019 bound 4).
//! 4. ADR-0019 bounds 2–4 stand. Bound 3's floor between calls is also an
//!    obligation to Antigravity's own session, not only a consent rule: the
//!    refresh grant is rate-limited per client, and this Companion shares
//!    Antigravity's client id with Antigravity itself.
//!
//! [ADR-0020]: ../../../docs/adr/0020-a-companion-may-exchange-a-google-refresh-token.md

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde_json::{json, Value};

use tokenledger_lib::limits_artifact::{self, LimitsExport, WindowExport, NOT_SIGNED_IN};

/// Production only. The `daily-` canary resolves and answers, but it is a
/// release-channel frontend onto the *same* registered service — its own error
/// bodies name `cloudcode-pa.googleapis.com` — so it cannot return different
/// numbers, and reaching for it first (as openusage does) risks a canary 401
/// short-circuiting into a signed-out card for a perfectly good session.
const CLOUD_CODE: &str = "https://cloudcode-pa.googleapis.com/v1internal";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// Antigravity's own installed-app client, verified verbatim in its
/// `language_server` binary. Presenting it means presenting ourselves to Google
/// as Antigravity — the same posture as `claude-limits` presenting Claude
/// Code's own token, which ADR-0019 already accepts on this route.
const CLIENT_ID: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
/// The other half of the same installed-app pair, which ships in every copy of
/// Antigravity's `language_server`. Not a key (ADR-0020 bound 3) — but until it
/// carries the real value the refresh grant answers 400, which this Companion
/// would report as a signed-out card: a build error dressed as the user's
/// problem. `the_client_secret_is_the_real_one` below fails while it is unset,
/// so that can never ship quietly.
const CLIENT_SECRET: &str = "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VER-BINARY";

/// The Keychain item Antigravity writes. Here `-a` **is** used, unlike Claude's
/// item: the account is the literal string `antigravity` rather than a user name
/// (so the `@`-in-account bug that forced `-a` off Claude's lookup cannot
/// arise), and the service name `gemini` is generic enough that other Google
/// tooling could plausibly claim it.
const KEYCHAIN_SERVICE: &str = "gemini";
const KEYCHAIN_ACCOUNT: &str = "antigravity";
/// The wrapper `go-keyring` puts around a stored value. Antigravity's own
/// string, not a convention invented here.
const ENVELOPE_PREFIX: &str = "go-keyring-base64:";

/// `errSecItemNotFound` — the honest "not signed in". Every other non-zero exit
/// is a failure and must never be reported as an absence.
const EXIT_ITEM_NOT_FOUND: i32 = 44;

/// Bucket id → the Reading's key and duration. Matched **exactly**: the pool is
/// never inferred from `display_name` or the `window` string, both of which are
/// server-side vocabulary that can change without a client update. A future
/// `gemini-image-5h` must not silently join the Gemini pool.
///
/// The key carries the pool because Antigravity is the first Source where the
/// pool is a genuine second axis rather than a slot: two pools share both
/// durations, so `w300` alone addresses two different quotas with two different
/// fill levels, and one would overwrite the other in the content-keyed PK.
const BUCKETS: [(&str, &str, i64); 4] = [
    ("gemini-5h", "gemini:w300", 300),
    ("gemini-weekly", "gemini:w10080", 10080),
    ("3p-5h", "3p:w300", 300),
    ("3p-weekly", "3p:w10080", 10080),
];

fn main() {
    match run() {
        // stdout echoes the Artifact for a hand run and for inspection. It is not
        // the ingest path — the app reads the file, per ADR-0019.
        Ok(report) => println!("{report}"),
        Err(err) => {
            eprintln!("antigravity-limits: {err}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<String, String> {
    let credential = credential()?;
    let fetched_at = now();
    let access_token = access_token(&credential, fetched_at)?;

    // One call, two needs: the project the summary endpoint marks REQUIRED, and
    // the plan label.
    let assist = post(&access_token, "loadCodeAssist", json!({}))?;
    let project = assist
        .get("cloudaicompanionProject")
        .and_then(|p| p.as_str())
        .unwrap_or_default()
        .to_string();
    let body = post(
        &access_token,
        "retrieveUserQuotaSummary",
        json!({ "project": project }),
    )?;

    // `--shape` is the hand-run diagnostic for when the payload moves: keys,
    // numbers, and short enum-ish strings print, anything longer is redacted.
    // The app can never pass this flag (its sidecar allowlist carries no args).
    if std::env::args().skip(1).any(|a| a == "--shape") {
        return Ok(limits_artifact::shape(&body));
    }

    let export = LimitsExport {
        schema: limits_artifact::SCHEMA,
        source: "antigravity".to_string(),
        fetched_at,
        plan: plan(&assist),
        windows: windows(&body),
    };

    // A failed write is a failed run: the Artifact is how the reading reaches
    // the app at all, so exiting 0 here would report success having delivered
    // nothing, and the card would show an absence rather than the cause.
    if let Some(dir) = std::env::var_os("TOKENLEDGER_LIMITS_DIR") {
        limits_artifact::write(&PathBuf::from(dir), &export)
            .map_err(|err| format!("could not write the export: {err}"))?;
    }
    serde_json::to_string(&export).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// The credential document
// ---------------------------------------------------------------------------

/// What the stored document carries that a Limits fetch needs. `refresh_token`
/// is modelled here — and only here among the Companions — under ADR-0020.
struct Credential {
    access_token: String,
    /// Epoch seconds. The stored token is presented as-is while this is in the
    /// future; only past it is anything exchanged.
    expiry: i64,
    refresh_token: String,
}

fn credential() -> Result<Credential, String> {
    parse_envelope(&keystore_read()?)
}

/// The stored value, or the classified reason it could not be had. `Ok` carries
/// the raw envelope; the item's absence is the one case that reads as an
/// absence rather than a failure.
fn keystore_read() -> Result<String, String> {
    if !cfg!(target_os = "macos") {
        // go-keyring targets Secret Service and WinCred elsewhere, but that has
        // never been verified for this item, and guessing at a store would
        // report the wrong thing. Say what is true.
        return Err(format!(
            "{NOT_SIGNED_IN}: reading Antigravity's sign-in is only supported on macOS today"
        ));
    }
    let output = Command::new("/usr/bin/security")
        .args([
            "find-generic-password",
            "-s",
            KEYCHAIN_SERVICE,
            "-a",
            KEYCHAIN_ACCOUNT,
            "-w",
        ])
        .output()
        .map_err(|e| format!("could not run the credential reader: {e}"))?;

    if output.status.success() {
        // Never logged, here or anywhere.
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    if output.status.code() == Some(EXIT_ITEM_NOT_FOUND) {
        return Err(format!(
            "{NOT_SIGNED_IN}: no Antigravity sign-in found on this computer"
        ));
    }

    // Claude Code's own stderr taxonomy, so a locked keystore says it is locked
    // rather than being reported as an absence.
    let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
    let reason = if stderr.contains("locked") || stderr.contains("unlock") {
        "the login keystore is locked — unlock it and check again"
    } else if stderr.contains("interaction is not allowed") || stderr.contains("no user interaction")
    {
        "the credential store refused to answer without user interaction"
    } else if stderr.contains("cancel") {
        "the credential read was cancelled"
    } else {
        "the credential store could not be read"
    };
    Err(format!(
        "{reason} (security find-generic-password exited {})",
        output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "abnormally".into()),
    ))
}

/// `go-keyring-base64:<base64 JSON>` → the token triple. A malformed envelope is
/// a **failure**, never an absence: telling someone they are not signed in when
/// the document was merely unreadable sends them to redo a login they have.
fn parse_envelope(raw: &str) -> Result<Credential, String> {
    let unreadable = || "the stored Antigravity sign-in could not be read".to_string();
    let encoded = raw.strip_prefix(ENVELOPE_PREFIX).ok_or_else(unreadable)?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|_| unreadable())?;
    let v: Value = serde_json::from_slice(&decoded).map_err(|_| unreadable())?;

    let token = v.get("token").ok_or_else(unreadable)?;
    let string = |key: &str| token.get(key).and_then(|t| t.as_str()).unwrap_or_default();
    let access_token = string("access_token");
    let refresh_token = string("refresh_token");
    if access_token.trim().is_empty() && refresh_token.trim().is_empty() {
        return Err(format!(
            "{NOT_SIGNED_IN}: the stored sign-in carries no token"
        ));
    }
    Ok(Credential {
        access_token: access_token.to_string(),
        // Go marshals this in the machine's own location, so the offset is
        // load-bearing: read as UTC, a +08:00 stamp would look hours fresher
        // than it is and a dead token would be presented as live.
        expiry: token
            .get("expiry")
            .and_then(|e| e.as_str())
            .and_then(rfc3339_to_epoch)
            .unwrap_or(0),
        refresh_token: refresh_token.to_string(),
    })
}

fn rfc3339_to_epoch(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.timestamp())
}

// ---------------------------------------------------------------------------
// The token
// ---------------------------------------------------------------------------

/// The stored access token while it lives, a freshly minted one after that.
/// Nothing is written anywhere in either case (ADR-0020 bounds 1 and 2).
fn access_token(credential: &Credential, now: i64) -> Result<String, String> {
    // A minute of headroom: a token expiring while the request is in flight
    // would 401, and the exchange is the cheaper branch to take early.
    if credential.expiry > now + 60 && !credential.access_token.is_empty() {
        return Ok(credential.access_token.clone());
    }
    if credential.refresh_token.is_empty() {
        return Err(format!(
            "{NOT_SIGNED_IN}: the stored Antigravity sign-in has expired — open Antigravity once to renew it"
        ));
    }
    exchange(&credential.refresh_token)
}

fn exchange(refresh_token: &str) -> Result<String, String> {
    let response = ureq::post(TOKEN_URL)
        .timeout(Duration::from_secs(15))
        .send_form(&[
            ("client_id", CLIENT_ID),
            ("client_secret", CLIENT_SECRET),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ]);
    let body: Value = match response {
        Ok(response) => response
            .into_string()
            .map_err(|e| e.to_string())
            .and_then(|body| serde_json::from_str(&body).map_err(|e| e.to_string()))
            .map_err(|e| format!("the sign-in service's answer could not be read: {e}"))?,
        // A revoked or expired grant is the signed-out card, pointing at
        // Antigravity — never an attempt to repair a session we do not own.
        Err(ureq::Error::Status(400 | 401 | 403, _)) => {
            return Err(format!(
                "{NOT_SIGNED_IN}: Google would not renew the saved Antigravity sign-in — open Antigravity once to sign in again"
            ))
        }
        Err(ureq::Error::Status(code, _)) => {
            return Err(format!("the sign-in service answered {code}"))
        }
        Err(err) => return Err(format!("could not reach the sign-in service: {err}")),
    };
    // The response is expected to carry no replacement refresh token, and there
    // is no field here for one if it did: what is minted is used once, in this
    // process, and discarded when it exits.
    body.get("access_token")
        .and_then(|t| t.as_str())
        .filter(|t| !t.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| "the sign-in service returned no access pass".to_string())
}

// ---------------------------------------------------------------------------
// The fetch
// ---------------------------------------------------------------------------

fn post(access_token: &str, method: &str, body: Value) -> Result<Value, String> {
    let response = ureq::post(&format!("{CLOUD_CODE}:{method}"))
        .set("Authorization", &format!("Bearer {access_token}"))
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        .timeout(Duration::from_secs(15))
        .send_string(&body.to_string());
    match response {
        Ok(response) => response
            .into_string()
            .map_err(|e| e.to_string())
            .and_then(|body| serde_json::from_str::<Value>(&body).map_err(|e| e.to_string()))
            .map_err(|e| format!("the vendor's answer could not be read: {e}")),
        Err(ureq::Error::Status(401 | 403, _)) => Err(format!(
            "{NOT_SIGNED_IN}: Google rejected the saved Antigravity sign-in (401/403)"
        )),
        Err(ureq::Error::Status(code, _)) => Err(format!("the vendor answered {code} to {method}")),
        Err(err) => Err(format!("could not reach the vendor: {err}")),
    }
}

/// The plan pill. `paidTier` wins over `currentTier`, and the marketing string
/// is cut to one word: a pill reading "Gemini Code Assist in Google One AI Pro"
/// would not fit beside the tool name. The inherited Windsurf
/// `planStatus.planInfo.planName` is deliberately not read — it reads "Pro" for
/// every paid tier.
fn plan(assist: &Value) -> Option<String> {
    let name = ["paidTier", "currentTier"]
        .iter()
        .find_map(|k| assist.pointer(&format!("/{k}/name")).and_then(|n| n.as_str()))?;
    Some(tier_word(name))
}

fn tier_word(name: &str) -> String {
    // "Google AI Pro" → "Pro"; "Gemini Code Assist in Google One AI Pro" → "Pro".
    // The tier word is what follows the last "AI"; anything else is kept whole
    // rather than truncated into something that means less than it said.
    let words: Vec<&str> = name.split_whitespace().collect();
    match words.iter().rposition(|w| w.eq_ignore_ascii_case("AI")) {
        Some(i) if i + 1 < words.len() => words[i + 1..].join(" "),
        _ => name.to_string(),
    }
}

/// The summary's buckets → Readings. `groups[].buckets[]` is the live shape;
/// the top-level `buckets[]` is marked deprecated in the descriptor and is read
/// only when the groups say nothing.
///
/// **No row, no bar** in four cases, all of them v1's "an absent Capability is
/// unknown, never zero": a bucket with no `resetTime` is a rolling window that
/// has not started, and fabricating an anchor would corrupt the `max(resets_at)`
/// epoch derivation; a bucket carrying `remainingAmount` instead of
/// `remainingFraction` is a count with no denominator — a figure, not a bar;
/// `disabled` is a pool that exists but is off for this account; and an
/// unrecognised `bucketId` is not guessed into a pool it may not belong to.
fn windows(body: &Value) -> Vec<WindowExport> {
    let grouped: Vec<&Value> = body
        .get("groups")
        .and_then(|g| g.as_array())
        .map(|groups| groups.iter().flat_map(bucket_list).collect())
        .unwrap_or_default();
    let buckets = if grouped.is_empty() {
        bucket_list(body)
    } else {
        grouped
    };

    let mut out: Vec<WindowExport> = buckets.into_iter().filter_map(bucket_window).collect();
    out.sort_by(|a, b| a.window_minutes.cmp(&b.window_minutes).then(a.key.cmp(&b.key)));
    out
}

fn bucket_list(node: &Value) -> Vec<&Value> {
    node.get("buckets")
        .and_then(|b| b.as_array())
        .map(|b| b.iter().collect())
        .unwrap_or_default()
}

fn bucket_window(bucket: &Value) -> Option<WindowExport> {
    let id = bucket.get("bucketId").and_then(|b| b.as_str())?;
    let Some((_, key, minutes)) = BUCKETS.iter().find(|(known, _, _)| *known == id) else {
        eprintln!("antigravity-limits: skipping unrecognised bucket {id}");
        return None;
    };
    if bucket.get("disabled").and_then(|d| d.as_bool()) == Some(true) {
        return None;
    }
    let Some(remaining) = bucket.get("remainingFraction").and_then(|f| f.as_f64()) else {
        // The one wire change that would silently empty the card, so it is said
        // out loud rather than dropped: a count with no denominator cannot be a
        // bar, and inventing one would be worse than the blank.
        if bucket.get("remainingAmount").is_some() {
            eprintln!(
                "antigravity-limits: bucket {id} reports a remaining amount rather than a fraction — no bar can be drawn from a count with no total"
            );
        }
        return None;
    };
    let resets_at = bucket
        .get("resetTime")
        .and_then(|r| r.as_str())
        .and_then(rfc3339_to_epoch)?;

    Some(WindowExport {
        key: key.to_string(),
        window_minutes: Some(*minutes),
        // The fraction is what is LEFT, so 1.0 is untouched. Rounded to an
        // integer, which is what keeps the content-keyed PK to ≤101 rows per
        // window per epoch.
        used_pct: ((1.0 - remaining) * 100.0).round(),
        resets_at,
    })
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

    // The summary shape as the vendor's own descriptors define it: buckets under
    // `groups[]`, the balance as a *remaining* fraction, and `resetTime` an
    // absolute Timestamp.
    const SUMMARY: &str = r#"{
        "groups": [{
            "displayName": "Model quota",
            "buckets": [
                {"bucketId": "gemini-5h", "displayName": "Gemini 5-hour",
                 "remainingFraction": 0.42, "resetTime": "2026-08-12T15:14:00Z"},
                {"bucketId": "gemini-weekly", "remainingFraction": 0.69,
                 "resetTime": "2026-08-16T02:00:00Z"},
                {"bucketId": "3p-5h", "remainingFraction": 0.88,
                 "resetTime": "2026-08-12T15:14:00Z"},
                {"bucketId": "3p-weekly", "remainingFraction": 0.82,
                 "resetTime": "2026-08-16T02:00:00Z"}
            ]
        }]
    }"#;

    #[test]
    fn four_buckets_become_four_pool_keyed_windows() {
        let found = windows(&serde_json::from_str(SUMMARY).unwrap());
        assert_eq!(
            found
                .iter()
                .map(|w| (w.key.as_str(), w.window_minutes, w.used_pct))
                .collect::<Vec<_>>(),
            vec![
                // The pool is part of the key: without it the two 5-hour pools
                // would be one row overwriting the other.
                ("3p:w300", Some(300), 12.0),
                ("gemini:w300", Some(300), 58.0),
                ("3p:w10080", Some(10080), 18.0),
                ("gemini:w10080", Some(10080), 31.0),
            ],
        );
        // 2026-08-12T15:14:00Z, the 5-hour pools' shared reset.
        assert_eq!(found[1].resets_at, 1_786_547_640, "resetTime → unix seconds");
    }

    #[test]
    fn the_fraction_is_what_is_left_so_untouched_is_zero_used() {
        // Reading it the other way round prints "100% used" on a fresh account.
        let body: Value = serde_json::from_str(
            r#"{"groups":[{"buckets":[{"bucketId":"gemini-5h","remainingFraction":1.0,
                "resetTime":"2026-08-12T15:14:00Z"}]}]}"#,
        )
        .unwrap();
        assert_eq!(windows(&body)[0].used_pct, 0.0);
    }

    #[test]
    fn a_bucket_this_card_cannot_draw_yields_no_bar() {
        // Every one of these is "unknown", and none of them is zero: a window
        // that has not started, a count with no denominator, a pool that is off,
        // and an id nobody has seen.
        for bucket in [
            r#"{"bucketId":"gemini-5h","remainingFraction":0.5}"#,
            r#"{"bucketId":"gemini-5h","remainingAmount":"420","resetTime":"2026-08-12T15:14:00Z"}"#,
            r#"{"bucketId":"gemini-5h","remainingFraction":0.5,"disabled":true,"resetTime":"2026-08-12T15:14:00Z"}"#,
            r#"{"bucketId":"gemini-image-5h","remainingFraction":0.5,"resetTime":"2026-08-12T15:14:00Z"}"#,
        ] {
            let body: Value =
                serde_json::from_str(&format!(r#"{{"groups":[{{"buckets":[{bucket}]}}]}}"#)).unwrap();
            assert!(windows(&body).is_empty(), "{bucket}");
        }
    }

    #[test]
    fn the_deprecated_top_level_buckets_are_read_only_when_the_groups_say_nothing() {
        let body: Value = serde_json::from_str(
            r#"{"buckets":[{"bucketId":"gemini-weekly","remainingFraction":0.5,
                "resetTime":"2026-08-16T02:00:00Z"}]}"#,
        )
        .unwrap();
        assert_eq!(windows(&body).len(), 1);

        // Where both are present the live shape wins outright.
        let both: Value = serde_json::from_str(
            r#"{"groups":[{"buckets":[{"bucketId":"gemini-5h","remainingFraction":0.9,
                 "resetTime":"2026-08-12T15:14:00Z"}]}],
                "buckets":[{"bucketId":"gemini-weekly","remainingFraction":0.5,
                 "resetTime":"2026-08-16T02:00:00Z"}]}"#,
        )
        .unwrap();
        assert_eq!(
            both_keys(&both),
            vec!["gemini:w300"],
            "the deprecated list is not merged in",
        );
    }

    fn both_keys(body: &Value) -> Vec<String> {
        windows(body).into_iter().map(|w| w.key).collect()
    }

    #[test]
    fn the_envelope_yields_the_token_triple_and_a_malformed_one_is_a_failure() {
        let document = r#"{"token":{"access_token":"ya29-access","token_type":"Bearer",
            "refresh_token":"1//refresh","expiry":"2026-08-12T09:00:00.123456+08:00"},
            "auth_method":"oauth"}"#;
        let envelope = format!(
            "{ENVELOPE_PREFIX}{}",
            base64::engine::general_purpose::STANDARD.encode(document),
        );
        let Ok(credential) = parse_envelope(&envelope) else {
            panic!("a well-formed envelope must parse");
        };
        assert_eq!(credential.access_token, "ya29-access");
        assert_eq!(credential.refresh_token, "1//refresh");
        // The offset is load-bearing: 09:00+08:00 is 01:00Z, not 09:00Z. Read as
        // UTC, a dead token would look eight hours fresher than it is.
        assert_eq!(credential.expiry, 1_786_496_400);

        // A malformed envelope must NOT borrow the absence prefix — the app
        // classifies on it, and an unreadable document is a failure.
        for broken in ["not-an-envelope", "go-keyring-base64:!!!!", "go-keyring-base64:e30="] {
            // `.err()` rather than `expect_err`: `Credential` carries no Debug,
            // so a token can never reach a panic message by accident.
            let err = parse_envelope(broken).err().expect("must not parse");
            assert!(!err.starts_with(NOT_SIGNED_IN), "{broken}: {err}");
        }
    }

    #[test]
    fn a_live_stored_token_is_used_as_is_and_only_an_expired_one_is_exchanged() {
        let credential = |expiry| Credential {
            access_token: "ya29-stored".to_string(),
            expiry,
            refresh_token: String::new(),
        };
        // In the future: presented as-is, nothing is exchanged (ADR-0020 bound 1).
        assert_eq!(access_token(&credential(2_000), 1_000).as_deref(), Ok("ya29-stored"));
        // Past it, with no refresh token to spend, the card says signed out
        // rather than presenting a dead token and reading the 401 back.
        let err = access_token(&credential(1_000), 2_000).expect_err("must not be usable");
        assert!(err.starts_with(NOT_SIGNED_IN), "{err}");
    }

    #[test]
    fn the_client_secret_is_the_real_one() {
        // Shipping the placeholder would make every exchange answer 400, which
        // this Companion reports as "not signed in" — sending someone to redo a
        // login that is perfectly good. Read the pair out of Antigravity's own
        // `language_server` binary; it is a public app identifier, not a key.
        assert!(
            !CLIENT_SECRET.contains("SET-ME"),
            "the Antigravity client secret is still the placeholder",
        );
    }

    #[test]
    fn the_plan_pill_prefers_the_paid_tier_and_says_one_word() {
        let assist: Value = serde_json::from_str(
            r#"{"currentTier":{"name":"Google AI Free"},"paidTier":{"name":"Google AI Pro"}}"#,
        )
        .unwrap();
        assert_eq!(plan(&assist).as_deref(), Some("Pro"));

        assert_eq!(tier_word("Gemini Code Assist in Google One AI Ultra"), "Ultra");
        assert_eq!(tier_word("Google AI Ultra Lite"), "Ultra Lite");
        // A name with no tier word in it keeps what the vendor said rather than
        // being truncated into something that means less.
        assert_eq!(tier_word("Enterprise"), "Enterprise");
        assert_eq!(plan(&serde_json::json!({})), None);
    }
}
