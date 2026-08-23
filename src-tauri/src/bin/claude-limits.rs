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
//!    manual Refresh, with a floor between calls, never on a timer. Within one
//!    such run a refused 429 earns a single bounded retry (seconds, clamped) —
//!    never a second run.
//! 4. A 401/403 says so and points at the Source's own CLI. It never tries to
//!    repair a session it does not own.
//!
//! One thing runs BEFORE any of that (TOKL-20): Claude Code caches its own
//! last answer from this same endpoint in its config document
//! (`cachedUsageUtilization`), stamped with when it fetched. Inside a short
//! freshness gate that cache IS the reading — no network, no credential, no
//! shared budget spent — and past the gate it remains the fallback when the
//! live fetch is rate-limited, but only while it is newer than the Artifact
//! already delivered: Claude Code has been seen to stop refreshing this cache
//! for days, and a Reading from behind the Artifact must never overwrite it
//! nor let a refused check exit 0. A person still has to ask (bound 3 is about
//! when this runs, not what it reads). Inside the gate no sign-in question is
//! asked at all; past it, a sign-in failure never borrows the cache — old
//! figures must not paper over a dead login, which therefore surfaces on the
//! first check past the gate.
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

use tokenledger_lib::limits_artifact::{
    self, LimitsExport, ModelScope, WindowEvidence, WindowExport, NOT_SIGNED_IN,
};
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
    // The diagnostics exist to inspect the LIVE payload and the STORED
    // credential, so neither may be answered from the cache.
    let diagnostic = std::env::args()
        .skip(1)
        .any(|a| a == "--shape" || a == "--credential-shape");

    // Claude Code's own last answer from this endpoint. Fresh enough, it IS the
    // reading: the moments the vendor is likeliest to refuse (a session actively
    // spending the shared budget) are exactly the moments this cache is being
    // refreshed, so serving it dodges the 429 AND stops spending the budget the
    // session is using. Held past the gate too — it is the 429 fallback below.
    let cache = if diagnostic { None } else { cache_reading() };
    // The Artifact this run would overwrite — the newest reading already
    // delivered. The cache is measured against it, because TOKL-20's premise
    // ("Claude Code refreshes this cache constantly while in use") has been
    // seen to fail for days at a stretch: a cache from behind the Artifact
    // must never regress it. The gate may re-serve an equal stamp (the
    // idempotent no-op every page open inside the gate depends on), but the
    // 429 fallback below demands strictly newer — re-delivering figures the
    // Ledger already holds would exit 0 with nothing to show, and the card
    // would sit silently stale with no refusal to explain why.
    let delivered_at = limits_dir()
        .and_then(|dir| limits_artifact::read(&dir, "claude"))
        .map(|held| held.fetched_at);
    if let Some(fresh) = cache.as_ref().filter(|c| {
        cache_is_fresh(c.fetched_at, now()) && delivered_at.is_none_or(|d| c.fetched_at >= d)
    }) {
        return deliver(fresh.clone());
    }

    let credential = credential()?;
    if let Some(scopes) = &credential.scopes {
        if !scopes.is_empty() && !scopes.iter().any(|s| s == REQUIRED_SCOPE) {
            return Err(format!(
                "{NOT_SIGNED_IN}: this Claude login cannot read Limits (no {REQUIRED_SCOPE} scope)"
            ));
        }
    }

    let body = match fetch(&usage_url(), &credential.access_token) {
        Ok(body) => body,
        // The 429 verdict — and ONLY it — falls back to the cache at any age,
        // provided the cache says something NEW (strictly newer than the
        // Artifact already delivered): a refusal does not invalidate the newest
        // answer the vendor already gave, and the Artifact carries the cache's
        // own stamp so the card dates it honestly. A cache the Ledger already
        // holds is no answer at all — delivering it would report success while
        // the card sat stale — so there the refusal stays the verdict. Sign-in
        // failures take the arm below instead (401/403) or returned before the
        // fetch (absent credential, missing scope), so a dead login is never
        // papered over with old figures. A diagnostic run never loaded a cache,
        // so its 429 stays a plain error here too.
        Err(err) if err == RATE_LIMITED => {
            return match cache.filter(|c| delivered_at.is_none_or(|d| c.fetched_at > d)) {
                Some(held) => deliver(held),
                None => Err(err),
            };
        }
        Err(err) => return Err(err),
    };

    // `--shape` is the hand-run diagnostic for when the vendor's payload moves:
    // it prints the response's structure — keys, numbers, short enum-ish strings —
    // with anything longer redacted, so a drifted shape can be diagnosed from a
    // transcript without usage identifiers in it. The app can never pass this
    // flag (its sidecar allowlist carries no args).
    if std::env::args().skip(1).any(|a| a == "--shape") {
        return Ok(limits_artifact::shape(&body));
    }
    // `--credential-shape` answers a different question: whether the stored
    // sign-in carries a stable opaque account identity, which is what Claude's
    // estimate lacks. Every value is redacted, so the answer can be pasted into
    // a transcript.
    if std::env::args().skip(1).any(|a| a == "--credential-shape") {
        return Ok(credential_shape(&credential_document()?));
    }
    deliver(LimitsExport {
        schema: limits_artifact::SCHEMA,
        source: "claude".to_string(),
        fetched_at: now(),
        plan: credential.plan,
        metering_regime: Some(METERING_REGIME.to_string()),
        account_id: account_id(&body),
        windows: windows(&body),
        ..Default::default()
    })
}

/// The one Claude regime, shared with every other producer of this Source's
/// Readings (the statusline tap included) — see the constant's own doc in
/// limits_artifact.rs for why there is exactly one.
const METERING_REGIME: &str = limits_artifact::CLAUDE_METERING_REGIME;

/// The end of every successful run, whichever path answered. The durable
/// Artifact is how the reading reaches the app at all — the scan and the
/// command both read the file, never this process's stdout (ADR-0019). So a
/// failed write is a failed run: exiting 0 here would report success having
/// delivered nothing, and the card would show an absence rather than the error
/// that caused it. A hand run with no directory named just prints.
fn deliver(export: LimitsExport) -> Result<String, String> {
    if let Some(dir) = limits_dir() {
        limits_artifact::write(&dir, &export)
            .map_err(|err| format!("could not write the export: {err}"))?;
    }
    serde_json::to_string(&export).map_err(|e| e.to_string())
}

/// Where the app told this run to write — and so where the previous run's
/// Artifact sits to be measured against. A hand run with no directory named
/// has neither, and the cache stands on its own stamp alone.
fn limits_dir() -> Option<PathBuf> {
    std::env::var_os("TOKENLEDGER_LIMITS_DIR").map(PathBuf::from)
}

// ---------------------------------------------------------------------------
// Claude Code's own cache of this endpoint
// ---------------------------------------------------------------------------

/// The freshness gate, in seconds. Inside it the cache answers INSTEAD of the
/// vendor; past it the cache only survives as the 429 fallback. ~5 minutes
/// tracks the refresh cadence observed while Claude Code is in use — a fresher
/// demand would miss real refreshes and re-open the shared-budget collision
/// this gate exists to avoid.
const CACHE_FRESH_SECS: i64 = 300;

fn cache_is_fresh(fetched_at: i64, now: i64) -> bool {
    // A future stamp (clock skew) reads as fresh, never as negative age.
    now - fetched_at <= CACHE_FRESH_SECS
}

/// Read and parse the cache, or None. Claude Code keeps this document BESIDE
/// its config dir's contents — `~/.claude.json`, not inside `~/.claude/` —
/// unless `CLAUDE_CONFIG_DIR` moves the whole root, the same override the
/// credential-file fallback honors. A missing, torn, or mid-rewrite document
/// is an absent cache, never an error: Claude Code rewrites this file
/// constantly underneath us.
fn cache_reading() -> Option<LimitsExport> {
    let path = match std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from) {
        Some(dir) => dir.join(".claude.json"),
        None => dirs::home_dir()?.join(".claude.json"),
    };
    let raw = std::fs::read_to_string(path).ok()?;
    parse_cache(&serde_json::from_str::<Value>(&raw).ok()?)
}

/// `cachedUsageUtilization` → a ready Export Artifact, or None. The
/// `utilization` value is the usage endpoint's own response body shape, so it
/// goes through the one window parser both response shapes already share — a
/// cache Reading and a live Reading can never disagree about what a window is.
///
/// `fetched_at` is the cache's OWN stamp, never now(): the Readings'
/// `observed_at` comes from it, and a stale answer wearing a fresh face is the
/// one dishonesty this whole feature must not commit.
fn parse_cache(document: &Value) -> Option<LimitsExport> {
    let cached = document.get("cachedUsageUtilization")?;
    // Millis (a JS timestamp) → the epoch seconds every Reading speaks.
    let fetched_at = cached.get("fetchedAtMs").and_then(|v| v.as_i64())? / 1000;
    let windows = windows(cached.get("utilization")?);
    // A cache with nothing usable in it must not outbid a live fetch.
    if windows.is_empty() {
        return None;
    }
    Some(LimitsExport {
        schema: limits_artifact::SCHEMA,
        source: "claude".to_string(),
        fetched_at,
        // The tier as this document states it. In the field it carries the
        // same grammar as the credential's `rateLimitTier` (the live path's
        // plan), but they are two fields in two documents — nothing PROVES
        // they agree, so this stays the cache document's own claim, and an
        // absent or blank tier means no plan pill, exactly as on the live
        // path. Blank identity fields are absent, never Some(""): an empty
        // string is a Series-identity component that would differ from NULL.
        plan: document
            .pointer("/oauthAccount/userRateLimitTier")
            .and_then(|v| v.as_str())
            .filter(|v| !v.trim().is_empty())
            .map(str::to_string),
        metering_regime: Some(METERING_REGIME.to_string()),
        // Claude Code wrote this identity beside the very payload it fetched
        // with it, so a cache Reading PROVES its account — which a live
        // Reading cannot (the response body names none, and stamping one from
        // a different document would synthesize evidence, ADR-0024). The
        // resulting proven/unproven mix in one timeline is deliberate: it is
        // the production shape the evidence walk already handles for Codex,
        // where unproven Readings pass through veto-only.
        account_id: cached
            .get("accountUuid")
            .and_then(|v| v.as_str())
            .filter(|v| !v.trim().is_empty())
            .map(str::to_string),
        windows,
        ..Default::default()
    })
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
    parse_credential(&credential_document()?)
}

/// The credential document as stored, before parsing.
///
/// The keystore route first where there is one, then the credential file — the
/// sole source elsewhere, and on macOS a fallback that must lose to a valid
/// keystore read.
fn credential_document() -> Result<String, String> {
    let mut trouble: Option<String> = None;
    if cfg!(target_os = "macos") {
        for service in service_candidates() {
            match keystore_read(&service) {
                Ok(Some(raw)) => return Ok(raw),
                Ok(None) => {}
                // Hold the failure: a later candidate may still be a clean hit,
                // and if none is, this is what the card must say.
                Err(err) => trouble = Some(err),
            }
        }
    }
    match credential_file().and_then(|path| std::fs::read_to_string(path).ok()) {
        Some(raw) => Ok(raw),
        None => Err(trouble.unwrap_or_else(|| {
            format!("{NOT_SIGNED_IN}: no Claude Code sign-in found on this computer")
        })),
    }
}

/// The credential document's structure, with **every value redacted**.
///
/// Hand-run only, like `--shape`: the app's sidecar allowlist carries no
/// arguments, so neither flag can be reached from the running app.
///
/// Stricter than `--shape` on purpose. That one prints short strings verbatim
/// because a vendor's enum values are the thing being diagnosed; this prints no
/// string from the document at all. A credential's short values may still be
/// secrets, and the question this exists to answer — is there a stable opaque
/// account identity in here, and what shape is it — needs only lengths. Booleans
/// and nulls carry nothing beyond presence, so they show as themselves; numbers
/// do not, because an expiry is a number.
fn credential_shape(raw: &str) -> String {
    let Ok(document) = serde_json::from_str::<Value>(raw) else {
        return "the stored Claude sign-in is not JSON".to_string();
    };
    let mut lines = Vec::new();
    walk_redacted(&document, "", &mut lines);
    lines.sort();
    lines.join("\n")
}

fn walk_redacted(node: &Value, prefix: &str, out: &mut Vec<String>) {
    match node {
        Value::Object(fields) => {
            for (key, value) in fields {
                walk_redacted(value, &format!("{prefix}.{key}"), out);
            }
        }
        // Element structure without element values: an array of scopes should
        // report how many there are and how long each is, never which they are.
        Value::Array(items) => {
            out.push(format!("{prefix}: [{} items]", items.len()));
            for (i, item) in items.iter().enumerate() {
                walk_redacted(item, &format!("{prefix}[{i}]"), out);
            }
        }
        Value::String(value) => {
            let uuidish = value.len() == 36
                && value.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
                && value.matches('-').count() == 4;
            out.push(format!(
                "{prefix}: <str len={}{}>",
                value.len(),
                if uuidish { " UUID-SHAPED" } else { "" },
            ));
        }
        Value::Number(_) => out.push(format!("{prefix}: <number>")),
        Value::Bool(value) => out.push(format!("{prefix}: {value}")),
        Value::Null => out.push(format!("{prefix}: null")),
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

/// The one 429 failure line, named so a test can hold it against the page's
/// signed-out classifier: a rate limit must never wear the "not signed in"
/// face (src/limits/LimitsPage.tsx `signedOut`). "a minute" is literal — the
/// page's LIVE_FLOOR_MS is when the next check is allowed.
const RATE_LIMITED: &str = "the vendor rate-limited this check (429) — try again in a minute";
/// Bounds on the one 429 retry's wait. The floor exists because the vendor
/// answers `retry-after: 0` in the field, and an instant re-fire is the retry
/// least likely to clear a budget this endpoint shares with Claude Code
/// itself; the cap so a hostile header cannot hold the check open.
const RETRY_FLOOR_SECS: u64 = 1;
const RETRY_FALLBACK_SECS: u64 = 2;
const RETRY_CAP_SECS: u64 = 10;

/// Where the fetch goes — the vendor, hardcoded, in every release build. The
/// debug-only override exists so the integration tests can aim this binary at
/// a fake vendor; it is compiled out of release builds because an
/// env-redirectable URL in a credential-presenting binary is an exfiltration
/// knob, and the shipped sidecars are release builds.
fn usage_url() -> String {
    #[cfg(debug_assertions)]
    if let Some(url) = std::env::var_os("TOKENLEDGER_CLAUDE_USAGE_URL") {
        return url.to_string_lossy().into_owned();
    }
    USAGE_URL.to_string()
}

fn fetch(url: &str, access_token: &str) -> Result<Value, String> {
    let mut retried = false;
    loop {
        let response = ureq::get(url)
            .set("Authorization", &format!("Bearer {access_token}"))
            .set("anthropic-beta", OAUTH_BETA)
            .timeout(Duration::from_secs(15))
            .call();
        match response {
            Ok(response) => {
                return response
                    .into_string()
                    .map_err(|e| e.to_string())
                    .and_then(|body| {
                        serde_json::from_str::<Value>(&body).map_err(|e| e.to_string())
                    })
                    .map_err(|e| format!("the vendor's answer could not be read: {e}"))
            }
            // Bound 4: report it and point at the Source's own CLI. Nothing is
            // written or refreshed in response.
            Err(ureq::Error::Status(401 | 403, _)) => {
                return Err(format!(
                    "{NOT_SIGNED_IN}: Claude rejected the saved sign-in (401/403)"
                ))
            }
            // A 429 is usually transient: this endpoint's budget is shared with
            // Claude Code itself, so a check landing beside a running session
            // can catch a momentary refusal. One retry after the vendor's own
            // Retry-After, still within this single person-asked run — never a
            // second run and never a timer (bound 3).
            Err(ureq::Error::Status(429, response)) => {
                if retried {
                    return Err(RATE_LIMITED.to_string());
                }
                retried = true;
                std::thread::sleep(Duration::from_secs(retry_after_secs(
                    response.header("retry-after"),
                )));
            }
            Err(ureq::Error::Status(code, _)) => {
                return Err(format!("the vendor answered {code}"))
            }
            Err(err) => return Err(format!("could not reach the vendor: {err}")),
        }
    }
}

/// The wait before the one 429 retry: the vendor's Retry-After where it names
/// seconds, the fallback where it is absent or an HTTP-date, clamped to the
/// bounds above.
fn retry_after_secs(header: Option<&str>) -> u64 {
    header
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(RETRY_FALLBACK_SECS)
        .clamp(RETRY_FLOOR_SECS, RETRY_CAP_SECS)
}

/// The response's windows. The modern shape carries a normalized `limits[]`
/// list, and a per-model window arrives ONLY there — a Fable weekly is a
/// `weekly_scoped` entry with `scope.model.display_name`, while the legacy
/// `seven_day_<model>` keys sit null beside it. So the list wins when it says
/// anything, and the named-key discovery below remains as the fallback for an
/// older response shape.
fn windows(body: &Value) -> Vec<WindowExport> {
    let listed = limits_list(body);
    if !listed.is_empty() {
        return listed;
    }
    let mut out = Vec::new();
    collect_windows(body, &mut out);
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

fn limits_list(body: &Value) -> Vec<WindowExport> {
    let Some(items) = body.get("limits").and_then(|l| l.as_array()) else {
        return Vec::new();
    };
    let mut out: Vec<WindowExport> = items.iter().filter_map(list_window).collect();
    out.sort_by(|a, b| a.window_minutes.cmp(&b.window_minutes).then(a.key.cmp(&b.key)));
    out
}

/// The two windows the vendor meters across the whole Source, and the one
/// mapping between the two names it gives each: `kind` in the modern `limits[]`
/// list, a response key in the older named shape. This table *is* the
/// one-to-one identity mapping the evidence contract asks an adapter to
/// document, and it is one table because three things read it — the key a
/// Reading is stored under, the duration the card ticks, and the Limit identity
/// evidence needs — so a third source-wide window is added here once.
///
/// The `kind` is the identity, because it names the window itself where the key
/// only labels it, and a label alone identifies nothing.
const SOURCE_WIDE: [SourceWide; 2] = [
    SourceWide { kind: "session", key: "five_hour", minutes: 300 },
    SourceWide { kind: "weekly_all", key: "seven_day", minutes: 10080 },
];

struct SourceWide {
    kind: &'static str,
    key: &'static str,
    minutes: i64,
}

/// The row the vendor named, by either of its names.
fn source_wide(kind_or_key: &str) -> Option<&'static SourceWide> {
    SOURCE_WIDE.iter().find(|w| w.kind == kind_or_key || w.key == kind_or_key)
}

/// The account this answer is for, if the response ever names it.
///
/// It does not. A `--shape` run against a live Max account (2026-08-14) returned
/// no account identity anywhere in the payload: the whole of it is windows,
/// `spend`, `extra_usage`, and unfamiliar codenames, and the only account-ish
/// keys are the booleans `extra_usage.user_disabled` and
/// `member_dashboard_available`. So this reads `account_uuid` and finds nothing,
/// which is deliberate rather than hopeful — the key is checked because the
/// response has carried it in an older named-key shape, and because the cost of
/// looking is nothing next to the cost of a Companion inventing an identity.
///
/// The consequence is honest and load-bearing: with no proven account, Claude's
/// Readings anchor no Limit Evidence Interval and its estimate stays **Blocked**
/// (spec: "if a current Source cannot populate the contract from data it already
/// reads, its estimate truthfully remains Blocked"). Giving Claude a real
/// identity means finding one in a document this tool already reads — the
/// credential — never a new endpoint, and never a token fingerprint, which the
/// contract rules out by name.
fn account_id(body: &Value) -> Option<String> {
    body.get("account_uuid")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(str::to_string)
}

/// What a window proves about itself, from either response shape.
///
/// A model-scoped window proves neither identity nor scope. `weekly_scoped`
/// names every one of them alike, so it cannot tell two apart, and what would
/// tell them apart is `scope.model.display_name` — a display name, which is no
/// mapping to the raw Models the Ledger logs. Both stay unknown and the window's
/// estimate stays Blocked, which is the honest answer until the vendor names a
/// raw Model.
fn window_evidence(kind_or_key: &str) -> WindowEvidence {
    let identity = source_wide(kind_or_key);
    WindowEvidence {
        limit_id: identity.map(|w| w.kind.to_string()),
        // Source-wide: every Claude Usage Record counts against these, including
        // Unattributed Usage.
        model_scope: identity.and_then(|_| ModelScope::All.stored()),
    }
}

/// One `limits[]` entry → a window. The keys are synthesized to match what the
/// legacy shape called the same windows — `five_hour`, `seven_day`,
/// `seven_day_<model>` — so a Reading from either response shape lands in the
/// same stored series, and the page's label discovery needs no second grammar.
/// `is_active` is not a gate: an inactive window still carries a real figure
/// and a real reset. An entry with no reset has not started and is no window.
fn list_window(item: &Value) -> Option<WindowExport> {
    let used_pct = item.get("percent").and_then(|p| p.as_f64())?;
    let resets_at = item
        .get("resets_at")
        .and_then(|r| r.as_i64().or_else(|| r.as_str().and_then(iso_to_epoch)))?;
    let kind = item.get("kind").and_then(|k| k.as_str()).unwrap_or("");
    let scoped_model = item
        .pointer("/scope/model/display_name")
        .and_then(|d| d.as_str());

    let (key, window_minutes) = match (source_wide(kind), kind, scoped_model) {
        (Some(w), _, _) => (w.key.to_string(), Some(w.minutes)),
        // A scoped weekly is the weekly window's length, under its own key.
        (None, "weekly_scoped", Some(model)) => (
            format!("seven_day_{}", slug(model)),
            source_wide("weekly_all").map(|w| w.minutes),
        ),
        // A kind nobody has seen still renders rather than vanishing: its kind
        // is its (opaque) key, and with no known duration it draws no tick.
        (None, _, _) if !kind.is_empty() => (kind.to_string(), None),
        _ => return None,
    };
    Some(WindowExport { key, window_minutes, used_pct, resets_at, evidence: window_evidence(kind) })
}

fn slug(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
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
        evidence: window_evidence(key),
    })
}

/// The window's length, read off the key that names it. The response states the
/// reset instant but not the duration, and the tick needs both — so a key that
/// names no duration yields None and the card draws no tick rather than
/// inventing an axis.
fn window_minutes(key: &str) -> Option<i64> {
    if let Some(window) = source_wide(key) {
        return Some(window.minutes);
    }
    // A scoped weekly runs for the weekly window's length under its own key.
    if key.starts_with("seven_day_") {
        return source_wide("weekly_all").map(|w| w.minutes);
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
    fn source_wide_windows_prove_their_limit_identity_and_scope() {
        let windows = windows(&serde_json::json!({"limits": [
            {"kind": "session", "percent": 25, "resets_at": "2026-08-12T13:00:00+00:00"},
            {"kind": "weekly_all", "percent": 35, "resets_at": "2026-08-16T13:00:00+00:00"},
            {"kind": "weekly_scoped", "percent": 30, "resets_at": "2026-08-16T13:00:00+00:00",
             "scope": {"model": {"display_name": "Fable 5"}}}
        ]}));
        let facts: Vec<(String, Option<String>, Option<String>)> = windows
            .iter()
            .map(|w| {
                (w.key.clone(), w.evidence.limit_id.clone(), w.evidence.model_scope.clone())
            })
            .collect();
        assert_eq!(
            facts,
            vec![
                // The vendor's own `kind`, which names the window itself rather
                // than describing it, and the whole-Source scope it meters.
                ("five_hour".to_string(), Some("session".to_string()), Some("all".to_string())),
                ("seven_day".to_string(), Some("weekly_all".to_string()), Some("all".to_string())),
                // A model-scoped window proves neither: `weekly_scoped` names
                // every one of them alike, and a display name is not a mapping
                // to the raw Models the Ledger logs. It stays Blocked.
                ("seven_day_fable_5".to_string(), None, None),
            ],
        );
    }

    #[test]
    fn the_legacy_response_shape_proves_the_same_identities() {
        let windows = windows(&serde_json::json!({
            "five_hour": {"utilization": 25.0, "resets_at": "2026-08-12T13:00:00+00:00"},
            "seven_day": {"utilization": 35.0, "resets_at": "2026-08-16T13:00:00+00:00"},
            "seven_day_opus": {"utilization": 30.0, "resets_at": "2026-08-16T13:00:00+00:00"}
        }));
        let facts: Vec<(String, Option<String>)> =
            windows.iter().map(|w| (w.key.clone(), w.evidence.limit_id.clone())).collect();
        // The two shapes name the same two windows, and the adapter documents
        // the mapping, so a Reading from either lands in one Series.
        assert_eq!(
            facts,
            vec![
                ("five_hour".to_string(), Some("session".to_string())),
                ("seven_day".to_string(), Some("weekly_all".to_string())),
                ("seven_day_opus".to_string(), None),
            ],
        );
    }

    // The modern response shape, verbatim from a real `--shape` run (2026-08-12,
    // Max 5x account): a normalized `limits[]` list beside the legacy named keys,
    // the per-model Fable window ONLY in the list (`seven_day_*` all null), and
    // an experiment field (`nimbus_quill`) that carries a `utilization` but no
    // reset — the decoy the fallback walker must keep skipping.
    const MODERN: &str = r#"{
        "five_hour": {"utilization": 25.0, "resets_at": "2026-08-12T13:00:00.000000+00:00"},
        "seven_day": {"utilization": 35.0, "resets_at": "2026-08-16T13:00:00.000000+00:00"},
        "seven_day_opus": null, "seven_day_sonnet": null,
        "nimbus_quill": {"utilization": 0.0, "resets_at": null},
        "limits": [
            {"kind": "session", "group": "session", "percent": 25, "is_active": false,
             "resets_at": "2026-08-12T13:00:00.000000+00:00", "scope": null, "severity": "normal"},
            {"kind": "weekly_all", "group": "weekly", "percent": 35, "is_active": true,
             "resets_at": "2026-08-16T13:00:00.000000+00:00", "scope": null, "severity": "normal"},
            {"kind": "weekly_scoped", "group": "weekly", "percent": 30, "is_active": false,
             "resets_at": "2026-08-16T13:00:00.000000+00:00",
             "scope": {"model": {"display_name": "Fable", "id": null}, "surface": null},
             "severity": "normal"}
        ],
        "extra_usage": {"is_enabled": false, "utilization": null},
        "spend": {"percent": 0, "severity": "normal"}
    }"#;

    #[test]
    fn one_real_response_reports_each_source_wide_window_under_both_names() {
        // The mapping this adapter documents is not an assumption: this captured
        // response names the same two windows twice at once — the modern list's
        // `kind` beside the older shape's key — and each pair reports the one
        // figure and the one reset instant. That co-observation is the proof the
        // identity rests on, so it is pinned here rather than argued in prose.
        let body: Value = serde_json::from_str(MODERN).unwrap();
        for window in SOURCE_WIDE {
            let listed = body["limits"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["kind"] == window.kind)
                .unwrap();
            let named = &body[window.key];
            assert_eq!(
                (listed["percent"].as_f64(), listed["resets_at"].as_str()),
                (named["utilization"].as_f64(), named["resets_at"].as_str()),
                "{} and {} are one window", window.kind, window.key,
            );
            // So either name proves the same identity, and the modern one is it.
            assert_eq!(window_evidence(window.kind), window_evidence(window.key));
            assert_eq!(
                window_evidence(window.key).limit_id.as_deref(),
                Some(window.kind),
            );
        }
    }

    #[test]
    fn the_limits_list_wins_and_carries_the_scoped_model_window() {
        let body: Value = serde_json::from_str(MODERN).unwrap();
        let found = windows(&body);

        assert_eq!(
            found
                .iter()
                .map(|w| (w.key.as_str(), w.window_minutes, w.used_pct))
                .collect::<Vec<_>>(),
            vec![
                ("five_hour", Some(300), 25.0),
                ("seven_day", Some(10080), 35.0),
                // The Fable window exists ONLY in the list; the key is
                // synthesized to the legacy grammar so the page's label
                // discovery renders it "Fable · Weekly" unchanged.
                ("seven_day_fable", Some(10080), 30.0),
            ],
        );
        // An inactive window still renders — `is_active` is not a gate — and the
        // experiment decoy with no reset contributes nothing.
        assert_eq!(found.len(), 3);
        assert_eq!(found[2].resets_at, iso_to_epoch("2026-08-16T13:00:00").unwrap());
    }

    #[test]
    fn an_unseen_list_kind_still_renders_under_its_own_kind() {
        let body: Value = serde_json::from_str(
            r#"{"limits":[{"kind":"monthly_all","group":"monthly","percent":12,
                "resets_at":"2026-09-01T00:00:00.000000+00:00"}]}"#,
        )
        .unwrap();
        let found = windows(&body);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].key, "monthly_all", "the kind is the opaque key");
        assert_eq!(found[0].window_minutes, None, "an unknown duration draws no tick");
    }

    #[test]
    fn an_empty_or_unusable_list_falls_back_to_named_key_discovery() {
        // Every list entry lacking a reset → nothing usable → the legacy walk
        // still reads the named keys.
        let body: Value = serde_json::from_str(
            r#"{"limits":[{"kind":"session","percent":25,"resets_at":null}],
                "five_hour":{"utilization":25.0,"resets_at":"2026-08-12T13:00:00Z"}}"#,
        )
        .unwrap();
        let found = windows(&body);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].key, "five_hour");
    }

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

        // `account_uuid` here is a DECOY: this fixture exists to prove the window
        // walk skips a non-window key, and the string is deliberately nonsense.
        // A live Max account's payload carries no account identity at all
        // (verified by `--shape`, 2026-08-14), so what matters is the empty and
        // absent cases — a response that names no account must yield none, which
        // is what keeps Claude honestly Blocked instead of inventing evidence.
        assert_eq!(account_id(&serde_json::json!({})), None, "the real shape");
        assert_eq!(account_id(&serde_json::json!({"account_uuid": "  "})), None);
        assert_eq!(account_id(&body).as_deref(), Some("not-a-window"), "the decoy, if present");

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
    fn the_credential_shape_echoes_no_value_from_the_document() {
        // The whole point of this diagnostic is that its output is safe to paste.
        // Every string below is a value a leak would expose, and none may appear
        // in what it prints — not the token, and not the short strings `--shape`
        // would have shown verbatim.
        let raw = r#"{"claudeAiOauth":{
            "accessToken":"sk-ant-oat01-SUPERSECRET-TOKEN",
            "refreshToken":"sk-ant-ort01-ANOTHER-SECRET",
            "rateLimitTier":"Max5x",
            "subscriptionType":"max",
            "scopes":["user:inference","user:profile"],
            "expiresAt":1786492800,
            "isMax":true,
            "organizationUuid":"8f14e45f-ceea-467a-9c1b-3f4b2d5a6e70",
            "nothing":null}}"#;

        let printed = credential_shape(raw);
        for secret in [
            "SUPERSECRET",
            "ANOTHER-SECRET",
            "sk-ant",
            "Max5x",
            "user:inference",
            "user:profile",
            "8f14e45f",
            "1786492800",
        ] {
            assert!(!printed.contains(secret), "leaked {secret}:\n{printed}");
        }

        // What it may say: where each key is, how long its value is, and that one
        // of them is shaped like the account identity this is hunting for.
        assert!(printed.contains(".claudeAiOauth.accessToken: <str len=30>"), "{printed}");
        assert!(
            printed.contains(".claudeAiOauth.organizationUuid: <str len=36 UUID-SHAPED>"),
            "{printed}",
        );
        assert!(printed.contains(".claudeAiOauth.scopes: [2 items]"), "{printed}");
        assert!(printed.contains(".claudeAiOauth.scopes[0]: <str len=14>"), "{printed}");
        assert!(printed.contains(".claudeAiOauth.expiresAt: <number>"), "{printed}");
        assert!(printed.contains(".claudeAiOauth.isMax: true"), "{printed}");
        assert!(printed.contains(".claudeAiOauth.nothing: null"), "{printed}");
    }

    #[test]
    fn the_429_retry_waits_the_vendors_own_delay_bounded() {
        assert_eq!(retry_after_secs(Some("5")), 5, "the vendor's stated seconds");
        assert_eq!(retry_after_secs(Some(" 3 ")), 3);
        assert_eq!(retry_after_secs(None), 2, "a short default when unstated");
        assert_eq!(
            retry_after_secs(Some("Wed, 21 Oct 2026 07:28:00 GMT")),
            2,
            "an HTTP-date falls back to the default",
        );
        assert_eq!(retry_after_secs(Some("3600")), 10, "capped — no hostage-taking");
        // The value the vendor actually sends in the field: an instant re-fire
        // is the retry least likely to clear a shared budget, so zero is floored.
        assert_eq!(retry_after_secs(Some("0")), 1, "floored — never an instant re-fire");
    }

    /// A throwaway server answering each canned response on its own connection.
    /// Every response says `connection: close`, so ureq cannot pool a socket
    /// across attempts and each retry is a fresh accept. When the responses run
    /// out the listener drops, and a further attempt fails to connect — which is
    /// how a retry loop that stopped being bounded would show itself here.
    fn serve(responses: Vec<String>) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for response in responses {
                let (mut socket, _) = listener.accept().unwrap();
                // One read suffices: the request is a small GET with no body.
                let _ = socket.read(&mut [0u8; 1024]);
                socket.write_all(response.as_bytes()).unwrap();
            }
        });
        url
    }

    // `retry-after: 1` keeps the retry real but the test fast — the floor and
    // cap are pinned by the delay test above, not re-proven here.
    const REFUSED: &str =
        "HTTP/1.1 429 Too Many Requests\r\nretry-after: 1\r\nconnection: close\r\ncontent-length: 0\r\n\r\n";

    fn answered() -> String {
        let body = r#"{"five_hour":{"utilization":1.0,"resets_at":1786503900}}"#;
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{body}",
            body.len(),
        )
    }

    #[test]
    fn a_refused_429_is_retried_once_and_the_second_answer_wins() {
        let url = serve(vec![REFUSED.to_string(), answered()]);
        let body = fetch(&url, "test-token").expect("the retry must deliver the second answer");
        assert_eq!(body["five_hour"]["utilization"], 1.0);
    }

    #[test]
    fn a_second_429_is_the_verdict_and_never_wears_the_signed_out_face() {
        let url = serve(vec![REFUSED.to_string(), REFUSED.to_string()]);
        let trouble = fetch(&url, "test-token").expect_err("two refusals are a failure");
        assert_eq!(trouble, RATE_LIMITED);
        // The page classifies by phrase (LimitsPage.tsx `signedOut`): a rate
        // limit reading as "sign in again" would send someone to re-authenticate
        // a login they already have — the same hazard EXIT_ITEM_NOT_FOUND guards.
        assert!(!trouble.starts_with(NOT_SIGNED_IN));
        assert!(!trouble.to_lowercase().contains("not signed in"), "{trouble}");
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

    // Claude Code's config document, verbatim in structure from a real machine
    // (2026-08-21, Max 5x account; identifier digits scrubbed): the null
    // experiment windows, the no-reset `nimbus_quill` decoy, and the
    // `extra_usage`/`spend` blocks all ride along so the parse is proven
    // against the document Claude Code actually writes, not a convenient one.
    // One file serves this seam and the process-level one (tests/), so the two
    // can never drift apart.
    const CACHED_DOCUMENT: &str =
        include_str!("../../tests/fixtures/claude/cached-config-document.json");

    #[test]
    fn the_cache_parses_through_the_same_window_parser_as_a_live_answer() {
        let document: Value = serde_json::from_str(CACHED_DOCUMENT).unwrap();
        let export = parse_cache(&document).expect("the real document must parse");

        // Millis → the epoch seconds every Reading speaks.
        assert_eq!(export.fetched_at, 1_787_256_759);
        assert_eq!(export.account_id.as_deref(), Some("ca9db76e-0000-4000-a000-a91c5789ed3a"));
        // The user tier, not the organization one beside it.
        assert_eq!(export.plan.as_deref(), Some("default_claude_max_5x"));
        // The one shared parser: the modern list wins, the scoped Fable window
        // rides under the legacy key grammar, and the no-reset decoy is skipped —
        // exactly what a live answer of this shape yields.
        assert_eq!(
            export
                .windows
                .iter()
                .map(|w| (w.key.as_str(), w.used_pct))
                .collect::<Vec<_>>(),
            vec![("five_hour", 36.0), ("seven_day", 31.0), ("seven_day_fable", 41.0)],
        );
    }

    #[test]
    fn blank_identity_fields_are_absent_never_empty_strings() {
        // An empty string is a Series-identity component that would differ
        // from NULL — the plan and account must vanish, not blank.
        let mut blanked: Value = serde_json::from_str(CACHED_DOCUMENT).unwrap();
        blanked["oauthAccount"]["userRateLimitTier"] = " ".into();
        blanked["cachedUsageUtilization"]["accountUuid"] = "".into();
        let export = parse_cache(&blanked).expect("blank identity does not unmake the cache");
        assert_eq!(export.plan, None);
        assert_eq!(export.account_id, None);
    }

    #[test]
    fn the_freshness_gate_is_a_pure_decision_over_the_caches_own_stamp() {
        let stamp = 1_787_256_759;
        assert!(cache_is_fresh(stamp, stamp), "just written");
        assert!(cache_is_fresh(stamp, stamp + CACHE_FRESH_SECS), "at the gate");
        assert!(!cache_is_fresh(stamp, stamp + CACHE_FRESH_SECS + 1), "past the gate");
        // Clock skew: a stamp from the future is fresh, never negative age.
        assert!(cache_is_fresh(stamp, stamp - 90));
    }

    #[test]
    fn a_cache_missing_or_unusable_is_absent_never_an_error() {
        let parse = |raw: &str| parse_cache(&serde_json::from_str::<Value>(raw).unwrap());
        // The key itself absent — an older Claude Code, or another machine.
        assert!(parse(r#"{"oauthAccount": {}}"#).is_none());
        // A stamp-less or body-less cache proves nothing.
        assert!(parse(r#"{"cachedUsageUtilization": {"utilization": {}}}"#).is_none());
        assert!(parse(r#"{"cachedUsageUtilization": {"fetchedAtMs": 1787256759360}}"#).is_none());
        // Windows present but none usable (no reset instant): nothing to say,
        // and it must not outbid a live fetch.
        assert!(parse(
            r#"{"cachedUsageUtilization": {"fetchedAtMs": 1787256759360,
                "utilization": {"five_hour": {"utilization": 0.0, "resets_at": null}}}}"#,
        )
        .is_none());
    }

    #[test]
    fn a_cache_export_carries_the_caches_stamp_and_the_one_shared_regime() {
        let document: Value = serde_json::from_str(CACHED_DOCUMENT).unwrap();
        let export = parse_cache(&document).unwrap();

        // The cache's OWN stamp, never now(): observed_at downstream is this
        // figure, and it is what keeps the card's freshness line honest.
        assert_eq!(export.fetched_at, 1_787_256_759);
        assert_eq!(export.schema, limits_artifact::SCHEMA);
        assert_eq!(export.source, "claude");
        // One regime constant for both producers: a cache Reading and a live
        // Reading land in one Series, never split over a divergent string.
        assert_eq!(export.metering_regime.as_deref(), Some(METERING_REGIME));
    }

    /// Runs only under `cargo test --release`, because it pins a fact about
    /// release builds: with the override compiled out, the vendor URL wins
    /// even when the variable is set. Deleting the `#[cfg(debug_assertions)]`
    /// gate on `usage_url` makes this fail.
    #[cfg(not(debug_assertions))]
    #[test]
    fn a_release_build_ignores_the_url_override() {
        std::env::set_var("TOKENLEDGER_CLAUDE_USAGE_URL", "http://127.0.0.1:1");
        assert_eq!(usage_url(), USAGE_URL);
        std::env::remove_var("TOKENLEDGER_CLAUDE_USAGE_URL");
    }
}
