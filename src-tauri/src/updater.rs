// The GitHub-Releases updater (issue #15): auto-check + notify, install only on
// user approval. The signing pubkey does not exist yet, so tauri.conf.json
// carries a placeholder and every failure path degrades to "not-configured" —
// we never fake an "up-to-date". Once a signed release + real pubkey land, the
// same code returns "available"/"downloaded" unchanged.
use std::path::Path;

use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::UpdaterExt;

use crate::settings::UpdateStatus;

/// Maps the plugin's check outcome to an honest UpdateStatus. A config failure
/// (bad/empty endpoint or pubkey) or an endpoint/network failure both become
/// "not-configured"; only a reachable, well-formed response yields up-to-date
/// or available.
pub async fn check(app: &AppHandle) -> UpdateStatus {
    let updater = match app.updater() {
        Ok(u) => u,
        Err(_) => return UpdateStatus::not_configured(),
    };
    match updater.check().await {
        Ok(Some(update)) => UpdateStatus::available(update.version),
        Ok(None) => UpdateStatus::up_to_date(),
        Err(_) => UpdateStatus::not_configured(),
    }
}

/// Downloads and stages the pending update. Driven only by the user-approved
/// Settings banner button. Signature verification happens here (against the
/// configured pubkey); a bad/unsigned artifact returns Err rather than a fake
/// success. On Ok the update is staged for the next restart.
pub async fn download(app: &AppHandle) -> Result<UpdateStatus, String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no update available".to_string())?;
    let version = update.version.clone();
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    Ok(UpdateStatus::downloaded(version))
}

/// Posts an OS notification — the notice channel for a start with no webview
/// (hidden at-login launch), where the in-window update cards cannot render.
/// A platform that refuses (no daemon, unbundled dev run) just stays quiet.
pub fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

/// The relaunch notice: record the running version and, on a hidden start
/// whose version climbed, announce the applied update via OS notification.
/// Every start records; a visible start leaves the announcing to the window's
/// "Updated" card, which keeps its own localStorage memory of the same fact.
pub fn announce_applied(app: &AppHandle, data_dir: &Path, hidden_start: bool) {
    let current = app.package_info().version.to_string();
    let jumped = applied_jump(&data_dir.join("last-version"), &current);
    if !hidden_start {
        return;
    }
    if let Some(last) = jumped {
        notify(app, "Updated", &format!("TokenLedger {last} → {current}"));
    }
}

/// Reads the recorded version, re-records `current`, and returns the previous
/// version only when this run is an upward move. A first run has no record —
/// nothing to announce, just start the memory — and a downgrade is not an
/// update, so it stays quiet (the same rules as updateFlow.ts).
fn applied_jump(record: &Path, current: &str) -> Option<String> {
    let last = std::fs::read_to_string(record)
        .ok()
        .map(|s| s.trim().to_string());
    let _ = std::fs::write(record, current);
    last.filter(|l| is_newer(current, l))
}

/// Whether `next` is a later version than `prev`, numeric per dot-component —
/// the Rust twin of updateFlow.ts's isNewerVersion, so 1.10.0 beats 1.9.0
/// rather than losing a string comparison.
fn is_newer(next: &str, prev: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> { v.split('.').map(|c| c.parse().unwrap_or(0)).collect() };
    parts(next) > parts(prev)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_newer_only_on_upward_numeric_moves() {
        assert!(is_newer("0.4.0", "0.3.0"));
        // Numeric collation, not lexicographic: "0.10.0" < "0.9.0" as strings.
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("0.3.0", "0.3.0"));
        assert!(!is_newer("0.2.0", "0.3.0"));
    }

    #[test]
    fn applied_jump_reports_upgrades_and_rerecords() {
        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join("last-version");
        // First run: no record yet, nothing to announce — just start the memory.
        assert_eq!(applied_jump(&record, "0.3.0"), None);
        assert_eq!(std::fs::read_to_string(&record).unwrap(), "0.3.0");
        // Same version again: quiet.
        assert_eq!(applied_jump(&record, "0.3.0"), None);
        // The climb is announced once, and the new version replaces the record.
        assert_eq!(applied_jump(&record, "0.4.0"), Some("0.3.0".into()));
        assert_eq!(applied_jump(&record, "0.4.0"), None);
        // A downgrade stays quiet but still re-records.
        assert_eq!(applied_jump(&record, "0.3.5"), None);
        assert_eq!(std::fs::read_to_string(&record).unwrap(), "0.3.5");
    }
}
