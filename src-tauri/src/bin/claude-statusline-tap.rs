//! claude-statusline-tap — a Reading source that presents nothing and fetches
//! nothing.
//!
//! Claude Code invokes the configured statusLine command on every render with
//! a JSON status document on stdin that includes `rate_limits`: the session's
//! own belief about the vendor's Limit windows. This tap sits in that pipe —
//! `~/.claude/settings.json` points at it, with the real statusline renderer
//! as its arguments:
//!
//! ```json
//! "statusLine": { "type": "command",
//!                 "command": "/path/to/claude-statusline-tap bunx -y ccstatusline@latest" }
//! ```
//!
//! It rename-writes the snapshot as the claude Export Artifact through the
//! same `limits_artifact` types and writer every Companion uses — one schema,
//! zero drift — then feeds the untouched stdin to the downstream command, so
//! the statusline renders exactly as before. Every tap failure is swallowed:
//! a tap bug must never blank a statusline.
//!
//! Where the Companion crosses ADR-0019's boundary with a credential, this
//! crosses nothing: Claude Code pushes its belief here unasked, on its own
//! cadence. That belief may itself be minutes old, and the Artifact's stamp is
//! receipt time — the bounded dishonesty accepted when this channel was
//! chosen. Two guards keep the file honest anyway: unchanged windows are the
//! same observation (no rewrite, no churn at render frequency), and a newer
//! Artifact — a live Companion fetch moments ago — is never regressed.
//!
//! The first render's full payload is kept beside Claude Code's config as
//! `claude-statusline-tap-payload.json` (delete it to re-capture): the
//! vendor's bucket set drifts, and a captured document beats guessing.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use tokenledger_lib::limits_artifact::{
    self, LimitsExport, WindowEvidence, WindowExport, CLAUDE_METERING_REGIME,
};

fn main() {
    let mut raw = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut raw);
    // The tap: best-effort, silent. The render below happens regardless.
    let _ = std::panic::catch_unwind(|| tap(&raw));

    // argv names the downstream renderer; no downstream means a bare install,
    // which renders nothing rather than failing.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((program, rest)) = args.split_first() else { return };
    let status = Command::new(program)
        .args(rest)
        .stdin(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            if let Some(stdin) = child.stdin.take() {
                let mut stdin = stdin;
                let _ = stdin.write_all(&raw);
            }
            child.wait()
        });
    match status {
        Ok(status) => std::process::exit(status.code().unwrap_or(0)),
        Err(err) => {
            eprintln!("claude-statusline-tap: could not run the statusline: {err}");
            std::process::exit(1);
        }
    }
}

fn tap(raw: &[u8]) {
    capture_once(raw);
    let Ok(document) = serde_json::from_slice::<Value>(raw) else { return };
    let Some(rate_limits) = document.get("rate_limits").and_then(|v| v.as_object()) else {
        return;
    };
    let windows = windows(rate_limits);
    if windows.is_empty() {
        return;
    }
    let now = now();
    // A long-idle session renders statuslines too, pushing a belief that can
    // be DAYS old (observed live: a five_hour window two days expired, pushed
    // as current). The snapshot dates itself: the vendor never reports an
    // expired window, so any expired window means the whole belief predates
    // that reset — none of its figures may be written. What this cannot catch
    // is a belief stale by less than the session window; that residue is the
    // bounded dishonesty this channel accepted, now with a provable bound.
    if windows.iter().any(|w| w.resets_at <= now) {
        return;
    }
    let dir = limits_dir();

    // Unchanged windows are the same observation — no churn at render
    // frequency — and a newer Artifact is never regressed. Beliefs order by
    // the windows themselves: usage within one window only grows, so on any
    // shared key an earlier reset instant — or the same instant with a lower
    // figure — is an older belief and yields. (A vendor recalculation that
    // lowers a figure mid-window — a limits promo moving the denominator —
    // is refused too; the next person-asked Companion check delivers it.)
    if let Some(held) = limits_artifact::read(&dir, "claude") {
        let same = serde_json::to_value(&held.windows).ok()
            == serde_json::to_value(&windows).ok();
        if same || held.fetched_at >= now || older_belief(&windows, &held.windows) {
            return;
        }
    }

    let (plan, account_id) = identity();
    let _ = limits_artifact::write(
        &dir,
        &LimitsExport {
            schema: limits_artifact::SCHEMA,
            source: "claude".to_string(),
            fetched_at: now,
            plan,
            metering_regime: Some(CLAUDE_METERING_REGIME.to_string()),
            account_id,
            windows,
            ..Default::default()
        },
    );
}

/// The buckets, generically: every object entry with a figure AND a reset
/// instant becomes a window under the vendor's own key — an unseen bucket
/// flows through rather than being dropped by a hand-kept list. A bucket
/// missing either proves nothing (the null experiment lanes, a no-reset
/// decoy) and is skipped, same as the Companion's parser decides.
fn windows(rate_limits: &serde_json::Map<String, Value>) -> Vec<WindowExport> {
    let mut keys: Vec<&String> = rate_limits.keys().collect();
    keys.sort();
    keys.into_iter()
        .filter_map(|key| {
            let bucket = rate_limits.get(key)?.as_object()?;
            let used_pct = bucket.get("used_percentage")?.as_f64()?;
            let resets_at = bucket.get("resets_at")?.as_i64()?;
            Some(WindowExport {
                key: key.clone(),
                window_minutes: minutes(key),
                used_pct,
                resets_at,
                evidence: evidence(key),
            })
        })
        .collect()
}

/// Whether `next` is an older belief than `held`, judged on the windows they
/// share: usage within one window never falls on its own, so an earlier reset
/// instant — or the same instant with a lower figure — says the pushing
/// session's knowledge predates the held Artifact's. No shared key says
/// nothing, and nothing is not a veto.
fn older_belief(next: &[WindowExport], held: &[WindowExport]) -> bool {
    next.iter().any(|n| {
        held.iter().any(|h| {
            h.key == n.key
                && (n.resets_at < h.resets_at
                    || (n.resets_at == h.resets_at && n.used_pct < h.used_pct))
        })
    })
}

fn minutes(key: &str) -> Option<i64> {
    match key {
        "five_hour" => Some(300),
        k if k.starts_with("seven_day") => Some(10080),
        _ => None,
    }
}

/// The Companion's own evidence mapping for the named windows; per-model keys
/// carry none there either, so none here — the two producers must agree on
/// what a window proves, or the estimator would trust one writer's word over
/// the other's silence for the same Series.
fn evidence(key: &str) -> WindowEvidence {
    let (limit_id, model_scope) = match key {
        "five_hour" => ("session", "all"),
        "seven_day" => ("weekly_all", "all"),
        _ => return WindowEvidence::default(),
    };
    WindowEvidence {
        limit_id: Some(limit_id.to_string()),
        model_scope: Some(model_scope.to_string()),
    }
}

/// plan + account from Claude Code's config document — the same non-credential
/// fields the Companion reports, read from the same override-aware location as
/// its cache reader, so both producers name one identity. Absent is absent,
/// never a blank that would split a Series from the Companion's Readings.
fn identity() -> (Option<String>, Option<String>) {
    let Some(document) = config_document() else { return (None, None) };
    let oauth = document.get("oauthAccount");
    let field = |node: Option<&Value>, key: &str| {
        node?
            .get(key)?
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let plan = field(oauth, "userRateLimitTier").or_else(|| field(oauth, "subscriptionType"));
    let account = field(oauth, "accountUuid")
        .or_else(|| field(document.get("cachedUsageUtilization"), "accountUuid"));
    (plan, account)
}

fn config_dir() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
}

fn config_document() -> Option<Value> {
    let raw = std::fs::read_to_string(config_dir()?.join(".claude.json")).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Where the Artifact goes: the app's own limits directory, overridable the
/// same way the Companions are (tests aim it at a tempdir).
fn limits_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("TOKENLEDGER_LIMITS_DIR") {
        return PathBuf::from(dir);
    }
    default_limits_dir()
}

/// The app reads Artifacts from `app_data_dir()/limits`, and Tauri spells
/// `app_data_dir()` as `dirs::data_dir()/<identifier>` (tauri's
/// `path/desktop.rs`). The Companions never need this — the app spawns them
/// with `TOKENLEDGER_LIMITS_DIR` already set — but Claude Code spawns the tap,
/// so this default is the only thing putting the file where the app looks.
///
/// `dirs::data_dir()`, not a hardcoded `~/Library/Application Support`: that
/// literal is only macOS's spelling of this, so off macOS the tap wrote to a
/// directory the app never opens and the Source read as idle.
fn default_limits_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_default().join(APP_IDENTIFIER).join("limits")
}

/// The bundle identifier, pinned to tauri.conf.json by the test below.
const APP_IDENTIFIER: &str = "com.brianwong.tokenledger";

/// One capture of the raw payload, for diagnosing the vendor's bucket set.
/// Written once — delete the file to re-capture — and never fatal. Lands in
/// `CLAUDE_CONFIG_DIR`, or `~/.claude/` where none is set (the document dir
/// `config_dir` names is the home root, which no capture should litter).
fn capture_once(raw: &[u8]) {
    let dir = match std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from) {
        Some(dir) => dir,
        None => match dirs::home_dir() {
            Some(home) => home.join(".claude"),
            None => return,
        },
    };
    let path = dir.join("claude-statusline-tap-payload.json");
    if dir.is_dir() && !path.exists() {
        let _ = std::fs::write(path, raw);
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tap is the one Artifact writer the app does not spawn, so nothing
    /// hands it `TOKENLEDGER_LIMITS_DIR` and this default is load-bearing: let
    /// it drift from where the app reads and the tap writes into a directory
    /// nobody opens, which the UI shows as a Source that simply never updates.
    ///
    /// Half of this bites only off macOS — there `dirs::data_dir()` IS
    /// `~/Library/Application Support`, so the hardcoded path this replaced
    /// passes here and fails on the other two. The identifier half bites
    /// everywhere.
    #[test]
    fn the_default_artifact_dir_is_the_one_the_app_reads() {
        let conf: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            conf["identifier"].as_str(),
            Some(APP_IDENTIFIER),
            "the bundle was renamed; the tap still writes under the old identifier"
        );
        assert_eq!(
            default_limits_dir(),
            dirs::data_dir().unwrap().join(APP_IDENTIFIER).join("limits"),
        );
    }
}
