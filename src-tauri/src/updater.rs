// The GitHub-Releases updater (issue #15): auto-check + notify, install only
// on user approval, with every failure path degrading to "not-configured" —
// a config, endpoint, or signature failure never fakes an "up-to-date".
// An Update Notice routes to exactly one surface (ADR-0026): the window's
// cards when a webview exists, an OS notification when none does — a hidden
// login start, or the window closed while the app stays resident.
use std::path::Path;
use std::sync::Mutex;

use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::UpdaterExt;

use crate::settings::{AppliedUpdate, UpdateStatus};

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

/// The resident update check, run from the capture thread's daily tick
/// (ADR-0026). A visible main window owns the notice through its card, so
/// this stays quiet there; with no window a find goes out as an OS
/// notification, once per version (`notified` remembers what this run already
/// said). The setting is read on every tick, so turning auto-check off takes
/// effect without a restart.
pub async fn resident_check(app: &AppHandle, notified: &Mutex<Option<String>>) {
    let Ok(settings) =
        crate::read(&app.state::<crate::AppState>(), crate::settings::get_settings)
    else {
        return;
    };
    if !settings.auto_check_updates {
        return;
    }
    if app
        .get_webview_window("main")
        .is_some_and(|w| w.is_visible().unwrap_or(false))
    {
        return;
    }
    let status = check(app).await;
    if status.state != "available" {
        return;
    }
    let Some(version) = status.version else { return };
    {
        let mut seen = notified.lock().unwrap();
        if seen.as_deref() == Some(version.as_str()) {
            return;
        }
        *seen = Some(version.clone());
    }
    // The notification has no click action on desktop, so the body says where
    // to act instead.
    if settings.language == "zh-Hant" {
        notify(app, "有可用更新", &format!("TokenLedger {version} 已就緒 — 開啟應用程式以安裝"));
    } else {
        notify(
            app,
            "Update available",
            &format!("TokenLedger {version} is ready — open the app to install"),
        );
    }
}

/// The relaunch notice: advance the version record and route the climb to one
/// surface. A hidden start announces it as an OS notification here and hands
/// the window nothing; a visible start returns it for the window's "Updated"
/// card to collect through the `applied_update` command.
pub fn applied_update(app: &AppHandle, data_dir: &Path, hidden_start: bool) -> Option<AppliedUpdate> {
    let current = app.package_info().version.to_string();
    let climb = advance_version_record(&data_dir.join("last-version"), &current)
        .map(|from| AppliedUpdate { from, to: current });
    let (announce, defer) = route(climb, hidden_start);
    if let Some(applied) = announce {
        let zh = crate::read(&app.state::<crate::AppState>(), crate::settings::get_settings)
            .map(|s| s.language == "zh-Hant")
            .unwrap_or(false);
        let title = if zh { "已更新" } else { "Updated" };
        notify(app, title, &format!("TokenLedger {} → {}", applied.from, applied.to));
    }
    defer
}

/// Splits the climb between the two notice surfaces: announced now as an OS
/// notification (a hidden start has no webview to card it), or deferred for
/// the window's "Updated" card to take. Never both — one applied update is
/// announced exactly once.
fn route(
    climb: Option<AppliedUpdate>,
    hidden_start: bool,
) -> (Option<AppliedUpdate>, Option<AppliedUpdate>) {
    match (climb, hidden_start) {
        (Some(climb), true) => (Some(climb), None),
        (Some(climb), false) => (None, Some(climb)),
        (None, _) => (None, None),
    }
}

/// Posts an OS notification — the notice surface for an app with no webview.
/// A platform that refuses (no daemon, unbundled dev run) just stays quiet.
pub fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

/// Advances the version record file to `current`, returning the version it
/// replaced when this run is an upward move. A first run has no record —
/// nothing to announce, just start the memory; a downgrade is not an update;
/// and an empty record (fs::write is not atomic, so a crash can leave one)
/// reads as no record rather than announcing a climb from a blank version.
fn advance_version_record(record: &Path, current: &str) -> Option<String> {
    let last = std::fs::read_to_string(record)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|l| !l.is_empty());
    let _ = std::fs::write(record, current);
    last.filter(|l| is_newer(current, l))
}

/// Whether `next` is a later version than `prev`, numeric per dot-component,
/// so 1.10.0 beats 1.9.0 rather than losing a string comparison. The one
/// comparator: the window's card receives its climb from here too, through
/// the applied_update command, so the two surfaces cannot disagree.
///
/// ponytail: dot-numeric, not semver. A prerelease component parses as 0
/// (1.4.2-beta reads as 1.4.2's patch dropped to 0), which the
/// GitHub-Releases channel never ships; reach for a real semver compare if it
/// ever does.
fn is_newer(next: &str, prev: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> { v.split('.').map(|c| c.parse().unwrap_or(0)).collect() };
    parts(next) > parts(prev)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn applied(from: &str, to: &str) -> AppliedUpdate {
        AppliedUpdate { from: from.into(), to: to.into() }
    }

    #[test]
    fn is_newer_only_on_upward_numeric_moves() {
        assert!(is_newer("0.4.0", "0.3.0"));
        // Numeric collation, not lexicographic: "0.10.0" < "0.9.0" as strings.
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("0.3.0", "0.3.0"));
        assert!(!is_newer("0.2.0", "0.3.0"));
    }

    #[test]
    fn the_record_advances_and_reports_only_upgrades() {
        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join("last-version");
        // First run: no record yet, nothing to announce — just start the memory.
        assert_eq!(advance_version_record(&record, "0.3.0"), None);
        assert_eq!(std::fs::read_to_string(&record).unwrap(), "0.3.0");
        // Same version again: quiet.
        assert_eq!(advance_version_record(&record, "0.3.0"), None);
        // The climb is reported once, and the new version replaces the record.
        assert_eq!(advance_version_record(&record, "0.4.0"), Some("0.3.0".into()));
        assert_eq!(advance_version_record(&record, "0.4.0"), None);
        // A downgrade stays quiet but still re-records.
        assert_eq!(advance_version_record(&record, "0.3.5"), None);
        assert_eq!(std::fs::read_to_string(&record).unwrap(), "0.3.5");
    }

    /// fs::write is not atomic: a crash mid-write can leave an empty record,
    /// which must read as "no record", never as a climb from a blank version.
    #[test]
    fn an_empty_record_announces_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join("last-version");
        std::fs::write(&record, "").unwrap();
        assert_eq!(advance_version_record(&record, "0.4.0"), None);
        assert_eq!(std::fs::read_to_string(&record).unwrap(), "0.4.0");
    }

    /// One applied update announces exactly once: over the OS notification on
    /// a hidden start (no webview), through the window's card otherwise.
    #[test]
    fn the_climb_routes_to_exactly_one_surface() {
        let climb = || Some(applied("0.3.0", "0.4.0"));
        assert_eq!(route(climb(), true), (climb(), None));
        assert_eq!(route(climb(), false), (None, climb()));
        assert_eq!(route(None, true), (None, None));
        assert_eq!(route(None, false), (None, None));
    }
}
