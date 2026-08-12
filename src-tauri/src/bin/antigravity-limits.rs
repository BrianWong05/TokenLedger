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
//!    or account switch could serve the previous account's figures.)
//! 3. The client id/secret pair identifies *Antigravity*, not us. Google's
//!    installed-app model ships both halves in every copy of each client: they
//!    are public identifiers, not keys. The id is hardcoded because the scan
//!    below needs a fixed thing to anchor on; the secret is read out of
//!    Antigravity's own installed client at run time, so this repository holds
//!    no copy of a vendor identifier that can go stale. Either one missing
//!    means no exchange, and the card degrades to signed-out pointing at
//!    Antigravity (ADR-0019 bound 4).
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

/// Both release-channel frontends, `daily-` first — because that is the one
/// Antigravity's own client actually calls. Its logs on this machine show
/// ~13,000 requests to `daily-` against four to production, which overturns the
/// research's reading (inferred from a 1:5 ratio of string occurrences in the
/// binary, and wrong: the plain host is the rarity).
///
/// Both are tried, and **a refusal is never a reason to stop trying** — that is
/// openusage's documented bug, where a canary 401 short-circuits the whole list
/// and reports a signed-out card for a perfectly good session. Here the first
/// success wins and the last failure is what gets reported.
const CLOUD_CODE_HOSTS: [&str; 2] = [
    "https://daily-cloudcode-pa.googleapis.com/v1internal",
    "https://cloudcode-pa.googleapis.com/v1internal",
];
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const USER_AGENT: &str = concat!("TokenLedger-limits/", env!("CARGO_PKG_VERSION"));

/// Antigravity's own installed-app client, verified verbatim in its
/// `language_server` binary. Presenting it means presenting ourselves to Google
/// as Antigravity — the same posture as `claude-limits` presenting Claude
/// Code's own token, which ADR-0019 already accepts on this route.
const CLIENT_ID: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
/// How Google spells the other half of an installed-app pair.
const SECRET_PREFIX: &[u8] = b"GOCSPX-";

/// Enough of the id to anchor the secret scan on — the account part, taken off
/// `CLIENT_ID` rather than written out again, since a second copy could drift
/// from the first. The `.apps.googleusercontent.com` tail is stored separately
/// in some string tables, so anchoring on the whole id would miss.
fn id_anchor() -> &'static [u8] {
    CLIENT_ID
        .split('.')
        .next()
        .unwrap_or(CLIENT_ID)
        .as_bytes()
}

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
/// durations, so `w300` alone addresses two different pools with two different
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

    // One call, two needs: the project the summary request names, and the plan
    // label.
    let assist = post(&access_token, "loadCodeAssist", json!({}))?;

    // `--shape` is the hand-run diagnostic for when a payload moves: keys,
    // numbers, and short enum-ish strings print, anything longer is redacted.
    // Both calls are reported, and the second's failure does not hide the
    // first's shape — the answer to "why did this stop working" is as often in
    // the call before the one that failed. The app can never pass this flag (its
    // sidecar allowlist carries no args).
    let shape_only = std::env::args().skip(1).any(|a| a == "--shape");

    // The descriptor marks `project` REQUIRED, so it is passed whenever the
    // vendor named one. When it does not — an individual-tier account may have
    // no companion project at all — the field is **omitted rather than sent
    // empty**. An empty string is a value the server would be entitled to answer
    // about, which is the silent-fallthrough this argument exists to prevent; an
    // absent field lets the server apply its own default, which is what the
    // prior art relies on. Refusing outright would be worse than either: it
    // would deny a card to an account whose Limits the server may answer for
    // perfectly well.
    let project = field(&assist, "cloudaicompanionProject")
        .and_then(|p| p.as_str())
        .filter(|p| !p.trim().is_empty());
    let request = match project {
        Some(project) => json!({ "project": project }),
        None => json!({}),
    };
    let summary = post(&access_token, "retrieveUserQuotaSummary", request);

    if shape_only {
        let summary = summary
            .as_ref()
            .map(limits_artifact::shape)
            .unwrap_or_else(|err| format!("<failed: {err}>\n"));
        return Ok(format!(
            "--- loadCodeAssist ---\n{}--- retrieveUserQuotaSummary (project {}) ---\n{summary}",
            limits_artifact::shape(&assist),
            if project.is_some() { "sent" } else { "omitted" },
        ));
    }
    let body = summary?;

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

    // The same stderr taxonomy `claude-limits` classifies by, so a locked
    // keystore says it is locked rather than being reported as an absence.
    // ponytail: deliberately duplicated rather than shared. The one library both
    // Companions already import is compiled into the app, and ADR-0019's whole
    // guarantee is that the always-running process never touches credential
    // machinery — checkable by grep. Hoisting this there would put keystore
    // vocabulary in the app to save twenty lines in two tools that are separate
    // processes on purpose.
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

/// Whether the stored pass still has life in it. A minute of headroom: one
/// expiring while the request is in flight would 401, and the exchange is the
/// cheaper branch to take early. Split out from the fetch so the gate itself is
/// testable — everything past it needs Google.
fn stored_pass_holds(credential: &Credential, now: i64) -> bool {
    credential.expiry > now + 60 && !credential.access_token.is_empty()
}

/// The stored access token while it lives, a freshly minted one after that.
/// Nothing is written anywhere in either case (ADR-0020 bounds 1 and 2).
fn access_token(credential: &Credential, now: i64) -> Result<String, String> {
    if stored_pass_holds(credential, now) {
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
    let client_secret = client_secret()?;
    let response = ureq::post(TOKEN_URL)
        .timeout(Duration::from_secs(15))
        .send_form(&[
            ("client_id", CLIENT_ID),
            ("client_secret", &client_secret),
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
// The vendor's own half of the pair
// ---------------------------------------------------------------------------

/// The client secret, read out of Antigravity's installed client (ADR-0020
/// bound 3). Only ever called on the exchange path, so a live stored pass costs
/// nothing, and the ≥60s floor already bounds how often it runs.
///
/// Not cached to disk and not carried in this repository: it is the vendor's
/// identifier, it can rotate, and a copy here would go stale silently — signing
/// every user out until somebody noticed and edited a constant.
fn client_secret() -> Result<String, String> {
    let path = language_server().ok_or_else(|| {
        "Antigravity is not installed where its sign-in could be renewed".to_string()
    })?;
    // ponytail: reads the whole image (~127MB) and scans it linearly, in a
    // process that exits moments later. Chunk it or memory-map it if the spike
    // or the ~1s ever shows up beside the network call it precedes.
    let image = std::fs::read(&path)
        .map_err(|err| format!("could not read Antigravity's client: {err}"))?;
    secret_in(&image).ok_or_else(|| {
        "Antigravity's installed client carries no sign-in pair this version knows".to_string()
    })
}

/// Where Antigravity keeps the client. Both halves of the pair ship inside it —
/// which is where they came from in the first place.
fn language_server() -> Option<PathBuf> {
    const BUNDLE: &str = "Antigravity.app/Contents/Resources/bin/language_server";
    let mut candidates = vec![PathBuf::from("/Applications").join(BUNDLE)];
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join("Applications").join(BUNDLE));
    }
    candidates.into_iter().find(|path| path.is_file())
}

/// The secret belonging to *our* client id: the nearest `GOCSPX-` string to the
/// id itself. A binary carrying one pair has exactly one candidate and the
/// distance never matters; one carrying several — a future build bundling
/// another Google client — would otherwise be a coin toss, and Google's own
/// codegen emits a pair's halves adjacent.
///
/// ponytail: proximity, not proof. If a build ever interleaves two clients'
/// constants, anchor on the surrounding struct instead.
fn secret_in(image: &[u8]) -> Option<String> {
    let anchor = find(image, id_anchor())?;
    let mut nearest: Option<(usize, String)> = None;
    let mut at = 0;
    while let Some(found) = find(&image[at..], SECRET_PREFIX) {
        let start = at + found;
        if let Some(secret) = secret_at(image, start) {
            let closer = nearest
                .as_ref()
                .is_none_or(|(best, _)| start.abs_diff(anchor) < best.abs_diff(anchor));
            if closer {
                nearest = Some((start, secret));
            }
        }
        at = start + SECRET_PREFIX.len();
    }
    nearest.map(|(_, secret)| secret)
}

/// The candidate starting at `at`, run out to the first byte that cannot belong
/// to one. A bare prefix with nothing after it is a string table's leftover, not
/// a secret.
fn secret_at(image: &[u8], at: usize) -> Option<String> {
    let end = image[at..]
        .iter()
        .position(|b| !(b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_'))
        .map_or(image.len(), |n| at + n);
    let candidate = std::str::from_utf8(image.get(at..end)?).ok()?;
    (candidate.len() >= SECRET_PREFIX.len() + 16).then(|| candidate.to_string())
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

// ---------------------------------------------------------------------------
// The fetch
// ---------------------------------------------------------------------------

fn post(access_token: &str, method: &str, body: Value) -> Result<Value, String> {
    let mut last = String::new();
    for host in CLOUD_CODE_HOSTS {
        match post_to(host, access_token, method, &body) {
            Ok(answer) => return Ok(answer),
            Err(err) => last = err,
        }
    }
    Err(last)
}

fn post_to(host: &str, access_token: &str, method: &str, body: &Value) -> Result<Value, String> {
    let response = ureq::post(&format!("{host}:{method}"))
        .set("Authorization", &format!("Bearer {access_token}"))
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        // Say who is actually asking, rather than arriving as an HTTP library's
        // default. This is deliberately *not* an impersonation: the client id on
        // the token already tells Google which app's quota is being asked about,
        // and inventing a vendor version string here would be a guess dressed as
        // a fact. If the surface ever gates on the client beyond that, the
        // refusal below now says so in Google's own words.
        .set("User-Agent", USER_AGENT)
        .timeout(Duration::from_secs(15))
        .send_string(&body.to_string());
    match response {
        Ok(response) => response
            .into_string()
            .map_err(|e| e.to_string())
            .and_then(|body| serde_json::from_str::<Value>(&body).map_err(|e| e.to_string()))
            .map_err(|e| format!("the vendor's answer could not be read: {e}")),
        // A refusal carries Google's own reason, and throwing it away is how a
        // diagnosable failure becomes "it doesn't work". A 401 answered to ONE
        // method while another succeeds on the same token is not a sign-in
        // problem at all, so the reason is what decides which card to draw.
        Err(ureq::Error::Status(code, response)) => {
            let reason = refusal(response);
            if matches!(code, 401 | 403) && reason.is_empty() {
                return Err(format!(
                    "{NOT_SIGNED_IN}: Google rejected the saved Antigravity sign-in ({code})"
                ));
            }
            Err(format!("the vendor answered {code} to {method}{reason}"))
        }
        Err(err) => Err(format!("could not reach the vendor: {err}")),
    }
}

/// Google's own explanation for a refusal — `status`, `message`, and any
/// `details[].reason` — which is structure and prose, never a credential. The
/// message is capped: it is a hint for a card and a bug report, not a log.
fn refusal(response: ureq::Response) -> String {
    let Some(body) = response.into_string().ok() else {
        return String::new();
    };
    let Ok(body) = serde_json::from_str::<Value>(&body) else {
        return String::new();
    };
    let error = body.get("error").unwrap_or(&body);
    let mut parts: Vec<String> = Vec::new();
    if let Some(status) = error.get("status").and_then(|s| s.as_str()) {
        parts.push(status.to_string());
    }
    if let Some(reason) = error
        .pointer("/details/0/reason")
        .and_then(|r| r.as_str())
    {
        parts.push(reason.to_string());
    }
    if let Some(message) = error.get("message").and_then(|m| m.as_str()) {
        parts.push(message.chars().take(200).collect());
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" — {}", parts.join(": "))
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

/// A field under either spelling the wire may use. Google's JSON transcoding
/// emits lowerCamelCase by default, which is what the descriptors' `json_name`
/// entries say and what openusage decodes — but the proto field names are
/// snake_case, and a server configured to emit those would otherwise drop every
/// bucket silently and leave an empty card with nothing to diagnose.
fn field<'a>(node: &'a Value, camel: &str) -> Option<&'a Value> {
    node.get(camel).or_else(|| {
        let mut snake = String::with_capacity(camel.len() + 2);
        for c in camel.chars() {
            if c.is_ascii_uppercase() {
                snake.push('_');
                snake.push(c.to_ascii_lowercase());
            } else {
                snake.push(c);
            }
        }
        node.get(&snake)
    })
}

/// The summary's buckets → Readings, read from `groups[].buckets[]`. The
/// top-level `buckets[]` is marked deprecated in the descriptor and is
/// deliberately not read: the spec asks for the summary shape and nothing else.
///
/// **No row, no bar** in four cases, all of them v1's "an absent Capability is
/// unknown, never zero": a bucket with no `resetTime` is a rolling window that
/// has not started, and fabricating an anchor would corrupt the `max(resets_at)`
/// epoch derivation; a bucket carrying `remainingAmount` instead of
/// `remainingFraction` is a count with no denominator — a figure, not a bar;
/// `disabled` is a pool that exists but is off for this account; and an
/// unrecognised `bucketId` is not guessed into a pool it may not belong to.
fn windows(body: &Value) -> Vec<WindowExport> {
    let mut out: Vec<WindowExport> = body
        .get("groups")
        .and_then(|g| g.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|group| group.get("buckets").and_then(|b| b.as_array()))
        .flatten()
        .filter_map(bucket_window)
        .collect();
    out.sort_by(|a, b| a.window_minutes.cmp(&b.window_minutes).then(a.key.cmp(&b.key)));
    out
}

fn bucket_window(bucket: &Value) -> Option<WindowExport> {
    let id = field(bucket, "bucketId").and_then(|b| b.as_str())?;
    let Some((_, key, minutes)) = BUCKETS.iter().find(|(known, _, _)| *known == id) else {
        eprintln!("antigravity-limits: skipping unrecognised bucket {id}");
        return None;
    };
    if field(bucket, "disabled").and_then(|d| d.as_bool()) == Some(true) {
        return None;
    }
    let Some(remaining) = field(bucket, "remainingFraction").and_then(|f| f.as_f64()) else {
        // The one wire change that would silently empty the card, so it is said
        // out loud rather than dropped: a count with no denominator cannot be a
        // bar, and inventing one would be worse than the blank.
        if field(bucket, "remainingAmount").is_some() {
            eprintln!(
                "antigravity-limits: bucket {id} reports a remaining amount rather than a fraction — no bar can be drawn from a count with no total"
            );
        }
        return None;
    };
    let resets_at = field(bucket, "resetTime")
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
    fn the_deprecated_top_level_buckets_are_not_read() {
        // Field 1 is marked deprecated in the descriptor and field 2 (`groups`)
        // is the live one. Reading both would double-count a server that sends
        // both, and the spec asks for the summary shape and nothing else.
        let body: Value = serde_json::from_str(
            r#"{"buckets":[{"bucketId":"gemini-weekly","remainingFraction":0.5,
                "resetTime":"2026-08-16T02:00:00Z"}]}"#,
        )
        .unwrap();
        assert!(windows(&body).is_empty());
    }

    #[test]
    fn a_snake_case_payload_reads_the_same_as_a_camel_case_one() {
        // The descriptors' `json_name` entries say camelCase and that is what
        // Google's transcoding emits — but the proto names are snake_case, and
        // the failure mode if the wire ever used them is an empty card with
        // nothing to diagnose. Both spellings are the same bucket.
        let body: Value = serde_json::from_str(
            r#"{"groups":[{"buckets":[{"bucket_id":"gemini-5h","remaining_fraction":0.42,
                "reset_time":"2026-08-12T15:14:00Z"}]}]}"#,
        )
        .unwrap();
        let found = windows(&body);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].key, "gemini:w300");
        assert_eq!(found[0].used_pct, 58.0);
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
        let credential = |expiry, access: &str| Credential {
            access_token: access.to_string(),
            expiry,
            refresh_token: "1//refresh".to_string(),
        };
        // Comfortably alive: presented as-is, nothing is exchanged and Google is
        // never reached at all (ADR-0020 bound 1).
        assert!(stored_pass_holds(&credential(2_000, "ya29-stored"), 1_000));
        assert_eq!(
            access_token(&credential(2_000, "ya29-stored"), 1_000).as_deref(),
            Ok("ya29-stored"),
        );
        // Expired, and also within the minute of headroom — a pass dying while
        // the request is in flight would come back a 401.
        assert!(!stored_pass_holds(&credential(1_000, "ya29-stored"), 2_000));
        assert!(!stored_pass_holds(&credential(1_030, "ya29-stored"), 1_000));
        // An envelope holding only a refresh token has nothing to present.
        assert!(!stored_pass_holds(&credential(9_999, ""), 1_000));

        // Past it with nothing to exchange, the card says signed out rather than
        // presenting a dead token and reading the 401 back.
        let spent = Credential {
            access_token: "ya29-stored".to_string(),
            expiry: 1_000,
            refresh_token: String::new(),
        };
        let err = access_token(&spent, 2_000).expect_err("must not be usable");
        assert!(err.starts_with(NOT_SIGNED_IN), "{err}");
    }

    // Fabricated stand-ins, built from the real prefix rather than written out
    // as literals — a hand-typed `GOCSPX-…` in this file would read as a leaked
    // secret to every scanner that ever looks at the repository, and would tie
    // the fixtures to a spelling the constant could change out from under.
    fn fake_secret(fill: char) -> String {
        format!("{}{}", std::str::from_utf8(SECRET_PREFIX).unwrap(), fill.to_string().repeat(28))
    }

    fn id() -> String {
        String::from_utf8(id_anchor().to_vec()).unwrap()
    }

    // A stand-in for the vendor's client: string-table noise around the two
    // halves of the pair, the way a Go binary carries its constants.
    fn image(body: &str) -> Vec<u8> {
        format!("\0\0some.other.symbol\0{body}\0trailing.noise\0").into_bytes()
    }

    #[test]
    fn the_secret_is_read_out_of_the_vendors_own_client() {
        let secret = fake_secret('a');
        let found = secret_in(&image(&format!("{}.apps.googleusercontent.com\0{secret}", id())));
        assert_eq!(found.as_ref(), Some(&secret));
    }

    #[test]
    fn a_second_google_client_in_the_same_binary_does_not_win() {
        // A build bundling another Google client would otherwise be a coin
        // toss. The pair's halves ship adjacent, so the nearest one is ours —
        // here the far candidate sits first, so position alone would pick wrong.
        let (theirs, ours) = (fake_secret('f'), fake_secret('n'));
        let found = secret_in(&image(&format!(
            "{theirs}{}{}\0{ours}",
            "\0".repeat(32),
            id(),
        )));
        assert_eq!(found.as_ref(), Some(&ours));
    }

    #[test]
    fn a_client_carrying_no_usable_pair_yields_nothing_rather_than_a_guess() {
        // No id, a bare prefix with nothing behind it, and an id with no secret
        // anywhere: each is a client this version cannot renew a sign-in with,
        // and none of them is an excuse to send Google something invented.
        let prefix = std::str::from_utf8(SECRET_PREFIX).unwrap();
        for body in [
            fake_secret('a'),
            format!("{}\0{prefix}", id()),
            format!("{}\0no pair here", id()),
        ] {
            assert_eq!(secret_in(&image(&body)), None, "{body}");
        }
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

/// Opt-in, machine-dependent: the scan above against a real installed client,
/// which is the only thing a fixture cannot stand in for. Ignored by default and
/// quiet where Antigravity is not installed, following the same pattern as this
/// crate's other real-data checks.
///
/// `cargo test --release --bin antigravity-limits real_client -- --ignored --nocapture`
///
/// It reports only *whether* a pair was found and how long the scan took —
/// never the pair, which has no business in a terminal or a transcript.
#[cfg(test)]
mod real_client {
    use super::*;

    #[test]
    #[ignore]
    fn the_installed_client_carries_a_pair_this_scan_can_find() {
        let Some(path) = language_server() else {
            println!("no installed client on this machine — nothing to check");
            return;
        };
        let started = std::time::Instant::now();
        let image = std::fs::read(&path).expect("the installed client must be readable");
        let found = secret_in(&image);
        println!(
            "image {} MB · found: {} · {} ms",
            image.len() / 1_000_000,
            found.is_some(),
            started.elapsed().as_millis(),
        );
        assert!(found.is_some(), "the scan found no pair in the installed client");
    }
}
