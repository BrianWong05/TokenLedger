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
    let credential = credential()?;
    if let Some(scopes) = &credential.scopes {
        if !scopes.is_empty() && !scopes.iter().any(|s| s == REQUIRED_SCOPE) {
            return Err(format!(
                "{NOT_SIGNED_IN}: this Claude login cannot read Limits (no {REQUIRED_SCOPE} scope)"
            ));
        }
    }

    let body = fetch(&credential.access_token)?;

    // `--shape` is the hand-run diagnostic for when the vendor's payload moves:
    // it prints the response's structure — keys, numbers, short enum-ish strings —
    // with anything longer redacted, so a drifted shape can be diagnosed from a
    // transcript without usage identifiers in it. The app can never pass this
    // flag (its sidecar allowlist carries no args).
    if std::env::args().skip(1).any(|a| a == "--shape") {
        return Ok(limits_artifact::shape(&body));
    }
    let export = LimitsExport {
        schema: limits_artifact::SCHEMA,
        source: "claude".to_string(),
        fetched_at: now(),
        plan: credential.plan,
        // One meter answers this endpoint, whichever shape it answers in: the
        // usage limits themselves. Nothing in the response distinguishes a
        // second regime, so naming one would be inventing it — and if one ever
        // appears, this identity changes deliberately and a new Series starts.
        metering_regime: Some("claude:usage_limits".to_string()),
        account_id: account_id(&body),
        usage_resets_available: None,
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
