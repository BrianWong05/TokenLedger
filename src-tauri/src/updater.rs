// The GitHub-Releases updater (issue #15): auto-check + notify, install only on
// user approval. The signing pubkey does not exist yet, so tauri.conf.json
// carries a placeholder and every failure path degrades to "not-configured" —
// we never fake an "up-to-date". Once a signed release + real pubkey land, the
// same code returns "available"/"downloaded" unchanged.
use tauri::AppHandle;
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
