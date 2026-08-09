mod adapters;
mod db;
mod pricing;
mod queries;
mod scan;
mod settings;
mod source_catalog;
mod time;
mod tray;
mod types;
mod updater;

// Task 16 end-to-end verification against the real logs on this machine.
// ponytail: lives inside the crate (not src-tauri/tests/) because db, scan,
// pricing, and queries are private (`mod`, not `pub mod`) above — an
// external integration-test crate can't see them, and widening that
// visibility is out of scope for a verification-only task. An internal
// #[cfg(test)] module gets full crate access for free instead.
#[cfg(test)]
mod e2e_real_logs;

// Opt-in validation against one contributor-selected private Source Artifact.
// The test is ignored by default and emits only privacy-safe aggregate evidence.
#[cfg(test)]
mod source_artifact_validation;

// Shared cross-Source partition invariants + a hermetic eight-Source test that
// runs them on synthetic logs every `cargo test`. Test-only, like e2e above.
#[cfg(test)]
mod invariants;

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

use rusqlite::Connection;
use tauri::{AppHandle, Emitter, Manager, State};

use pricing::{ModelPricing, RatesPerTok};
use queries::{
    BreakdownRow, CtxBuckets, CtxExecRow, CtxResource, CtxSkillRow, CtxToolRow, Filters, SeriesPoint,
    Summary, TrendPoint,
};
use scan::{run_scan, SourceRoots};
use settings::{Settings, UpdateStatus};
use types::ScanStatus;

// The at-login LaunchAgent starts the app with this flag so it comes up hidden
// (tray only); a normal Dock/Finder launch has no flag and shows the window.
const HIDDEN_FLAG: &str = "--hidden";

pub struct AppState {
    pub db: Mutex<Connection>,
    pub roots: SourceRoots,
    pub scan_lock: Mutex<()>,
    /// Model names already looked up against the catalogs during this run of the
    /// app. In memory only, so it resets on restart: a Model no catalog carries
    /// costs one fetch per launch instead of one per scan, and nothing about a
    /// failed lookup is worth persisting.
    pub price_lookups: Mutex<std::collections::HashSet<String>>,
    /// When the most recent scan finished, epoch seconds; 0 until one runs. In
    /// memory only, like `price_lookups`: the Menu Bar Extra asks how fresh its
    /// figures are, and the resident capture scans at start-up, so freshness
    /// from a previous launch would answer a question nobody asked.
    pub last_scan: AtomicI64,
}

/// Fetch both price catalogs and rebuild the prices table, then tell the frontend
/// so open views re-fetch their Cost figures. The blocking fetches run BEFORE the
/// DB lock is taken, so scanning and querying never wait on the network. Shared
/// by start-up and the newly-unpriced trigger so the two cannot drift apart.
fn refresh_catalogs(app: &AppHandle) {
    let Ok(data_dir) = app.path().app_data_dir() else {
        return;
    };
    let state = app.state::<AppState>();

    // No lock held: network here.
    let litellm = pricing::load_prices_json(&data_dir);
    let openrouter = pricing::load_openrouter_json(&data_dir);

    // Publisher rates are per-Model, so this needs to know which Models the
    // Ledger holds. Take the lock only to ask, and drop it before fetching —
    // the reads that follow are one request per Model.
    // Never return early from here: the emit at the bottom is what stops a fresh
    // install rendering 'unpriced' until the next range change. A failure to read
    // the Ledger costs the publisher tier this run, not the whole refresh.
    let targets = match openrouter.as_deref() {
        Some(json) => state
            .db
            .lock()
            .ok()
            .and_then(|db| pricing::ledger_publisher_targets(&db, json).ok())
            .unwrap_or_default(),
        None => Vec::new(),
    };
    let publishers = pricing::load_publisher_rates(&data_dir, &targets);

    if let Ok(mut db) = state.db.lock() {
        let _ = pricing::rebuild_prices(&mut db, &litellm, openrouter.as_deref(), &publishers);
    }
    // Without this a fresh install renders 'unpriced' until the next range change.
    let _ = app.emit("prices-rebuilt", ());
}

/// If a scan left the Ledger holding a Model that resolves to no rate and that we
/// have not tried yet this run, re-read the catalogs once for it — the catalogs
/// are otherwise only read at launch, so a Model first used mid-session would stay
/// Unpriced until a restart. Spawns, so a scan never waits on the network.
fn lookup_new_unpriced(app: &AppHandle) {
    let state = app.state::<AppState>();
    let fresh = {
        let (Ok(db), Ok(mut attempted)) = (state.db.lock(), state.price_lookups.lock()) else {
            return;
        };
        let fresh = pricing::models_needing_lookup(&db, &attempted).unwrap_or_default();
        // Record before fetching, not after: a Model no catalog covers must not
        // re-fetch on every subsequent scan just because the lookup found nothing.
        attempted.extend(fresh.iter().cloned());
        fresh
    };
    if fresh.is_empty() {
        return;
    }
    let handle = app.clone();
    std::thread::spawn(move || refresh_catalogs(&handle));
}

// The one scan path, shared by the `scan` command and the tray's "Scan now" so
// neither duplicates the locking/coalescing policy. Serialize scans: a second
// caller blocks on scan_lock, then runs its own incremental scan.
pub(crate) fn scan_now(app: &AppHandle) -> Result<ScanStatus, String> {
    let state = app.state::<AppState>();
    let _guard = state.scan_lock.lock().map_err(|e| e.to_string())?;
    let status = {
        // ponytail: single Mutex<Connection> per the AppState contract. A scan
        // briefly blocks reads; incremental scans are cheap, so no separate read
        // connection. Add one only if UI jank during scans is ever measured.
        let mut db = state.db.lock().map_err(|e| e.to_string())?;
        run_scan(&mut db, &state.roots)
    };
    // Every scan lands here — the command, the tray's "Scan now", and the
    // resident capture — so the panel's freshness read-out cannot miss one.
    state.last_scan.store(status.scanned_at, Ordering::Relaxed);
    tray::refresh(app);
    // Release scan_lock BEFORE the lookup: it reads the whole prices table to
    // decide, and holding the scan gate across that would delay the next scan for
    // no reason. Both the `scan` command and the tray's "Scan now" reach this, so
    // where the scan started never changes whether a Model gets looked up.
    drop(_guard);
    lookup_new_unpriced(app);
    Ok(status)
}

#[tauri::command]
async fn scan(app: AppHandle) -> Result<ScanStatus, String> {
    scan_now(&app)
}

/// Epoch seconds of the last scan this launch, 0 if none has run yet — the
/// Menu Bar Extra's freshness read-out. Cheap enough to answer on every open.
#[tauri::command]
fn last_scan(state: State<'_, AppState>) -> i64 {
    state.last_scan.load(Ordering::Relaxed)
}

/// Sources currently holding Unreadable Artifacts (ADR-0017), from the
/// persisted per-scan state — no rescan, and honest from launch because the
/// answer survives restarts. The traypanel reads this to put the ≥ floor
/// marker on its own window's token figure.
#[tauri::command]
fn unreadable_artifacts(state: State<'_, AppState>) -> Vec<types::SourceUnreadable> {
    state
        .db
        .lock()
        .map(|db| db::load_unreadable(&db))
        .unwrap_or_default()
}

#[tauri::command]
fn summary(state: State<'_, AppState>, filters: Filters) -> Result<Summary, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    queries::summary(&db, &filters).map_err(|e| e.to_string())
}

#[tauri::command]
fn trend(
    state: State<'_, AppState>,
    filters: Filters,
    bucket: String,
) -> Result<Vec<TrendPoint>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    queries::trend(&db, &filters, &bucket).map_err(|e| e.to_string())
}

#[tauri::command]
fn series(
    state: State<'_, AppState>,
    filters: Filters,
    bucket: String,
) -> Result<Vec<SeriesPoint>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    queries::series(&db, &filters, &bucket).map_err(|e| e.to_string())
}

#[tauri::command]
fn breakdown(
    state: State<'_, AppState>,
    by: String,
    filters: Filters,
) -> Result<Vec<BreakdownRow>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    queries::breakdown(&db, &by, &filters).map_err(|e| e.to_string())
}

#[tauri::command]
fn ctx_resources(
    state: State<'_, AppState>,
    filters: Filters,
) -> Result<Vec<CtxResource>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    queries::ctx_resources(&db, &filters).map_err(|e| e.to_string())
}

#[tauri::command]
fn ctx_buckets(state: State<'_, AppState>, filters: Filters) -> Result<Vec<CtxBuckets>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    queries::ctx_buckets(&db, &filters).map_err(|e| e.to_string())
}

#[tauri::command]
fn ctx_tools(state: State<'_, AppState>, filters: Filters) -> Result<Vec<CtxToolRow>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    queries::ctx_tools(&db, &filters).map_err(|e| e.to_string())
}

#[tauri::command]
fn ctx_skills(state: State<'_, AppState>, filters: Filters) -> Result<Vec<CtxSkillRow>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    queries::ctx_skills(&db, &filters).map_err(|e| e.to_string())
}

#[tauri::command]
fn ctx_exec(state: State<'_, AppState>, filters: Filters) -> Result<Vec<CtxExecRow>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    queries::ctx_exec(&db, &filters).map_err(|e| e.to_string())
}

// Re-read both catalogs on demand from the Pricing tab. Deliberately ignores the
// price_lookups guard that rate-limits the automatic trigger: asking explicitly is
// the one case where retrying a Model we already tried this run is the point. The
// set is left as-is, so this never causes the next scan to re-fetch as well.
#[tauri::command]
async fn refresh_prices(app: AppHandle) -> Result<(), String> {
    refresh_catalogs(&app);
    Ok(())
}

#[tauri::command]
fn model_pricing(state: State<'_, AppState>) -> Result<Vec<ModelPricing>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    pricing::model_pricing(&db).map_err(|e| e.to_string())
}

// The Pricing tab's Override mutations. Both emit the SAME prices-rebuilt event
// the price refresh emits, so the Overview recomputes Cost without a restart.
#[tauri::command]
fn set_model_override(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    model: String,
    rates: RatesPerTok,
) -> Result<(), String> {
    {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        pricing::set_override(&db, &model, rates.into()).map_err(|e| e.to_string())?;
    }
    app.emit("prices-rebuilt", ()).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_model_override(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    model: String,
) -> Result<(), String> {
    {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        pricing::delete_override(&db, &model).map_err(|e| e.to_string())?;
    }
    app.emit("prices-rebuilt", ()).map_err(|e| e.to_string())
}

// The traypanel's four actions (src/traypanel/TrayPanel.tsx). Rescan reuses
// the `scan` command; these three are window/lifecycle glue.
#[tauri::command]
fn show_main(app: AppHandle) {
    tray::show_main(&app);
}

#[tauri::command]
fn open_settings(app: AppHandle) -> Result<(), String> {
    tray::show_main(&app);
    // The shell's onOpenSettings listener lands on the Settings tab.
    app.emit("open-settings", ()).map_err(|e| e.to_string())
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

// The panel reports its rendered content height (logical px) and the window
// hugs it — the panel must never scroll or clip.
#[tauri::command]
fn resize_panel(app: AppHandle, height: f64) {
    if let Some(w) = app.get_webview_window("traypanel") {
        let _ = w.set_size(tauri::LogicalSize::new(300.0, height.max(1.0)));
    }
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    settings::get_settings(&db).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<(), String> {
    {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        settings::set_settings(&db, &settings).map_err(|e| e.to_string())?;
    }
    // A Display Currency change must reach the bar title promptly, not on the
    // next scan tick.
    tray::refresh(&app);
    Ok(())
}

#[tauri::command]
async fn check_updates(app: AppHandle) -> UpdateStatus {
    updater::check(&app).await
}

// User-approved from the Settings banner: downloads and stages the update.
#[tauri::command]
async fn download_update(app: AppHandle) -> Result<UpdateStatus, String> {
    updater::download(&app).await
}

// Applies a staged update by relaunching. Diverges (never returns).
#[tauri::command]
fn restart_app(app: AppHandle) {
    app.restart();
}

// The app's only file-save surface: opens the native save dialog seeded with a
// suggested name, and writes `contents` verbatim to the chosen path. The
// frontend assembles the CSV; this owns only the dialog + write. Returns whether
// a file was written (false = the user cancelled — a no-op).
#[tauri::command]
fn save_csv(app: AppHandle, filename: String, contents: String) -> Result<bool, String> {
    use tauri_plugin_dialog::DialogExt;
    let Some(file) = app
        .dialog()
        .file()
        .set_file_name(filename)
        .add_filter("CSV", &["csv"])
        .blocking_save_file()
    else {
        return Ok(false);
    };
    let path = file.into_path().map_err(|e| e.to_string())?;
    std::fs::write(path, contents).map_err(|e| e.to_string())?;
    Ok(true)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        // Launch at login passing HIDDEN_FLAG, so an at-login start comes up
        // hidden (tray only) while a manual launch does not. Enrollment itself
        // is driven from the frontend (first-run dialog + Settings toggle →
        // startup.ts). The mechanism is the plugin's per-platform default — a
        // LaunchAgent on macOS, a registry Run entry on Windows, a desktop
        // entry on Linux. Do not set macos_launcher to say so: the setter is
        // itself macOS-only and breaks the Windows and Linux builds.
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .args([HIDDEN_FLAG])
                .build(),
        )
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        // Closing the window must not kill capture (ADR-0005): hide it instead,
        // keeping the webview (and its auto-refresh scans) alive. Quit lives in
        // the tray.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            // The traypanel behaves like a menu: clicking anywhere else
            // (focus loss) dismisses it.
            if let tauri::WindowEvent::Focused(false) = event {
                if window.label() == "traypanel" {
                    let _ = window.hide();
                }
            }
        })
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let conn = db::open_db(&data_dir.join("tokenledger.db"))?;
            app.manage(AppState {
                db: Mutex::new(conn),
                roots: SourceRoots::default_roots(),
                scan_lock: Mutex::new(()),
                price_lookups: Mutex::new(Default::default()),
                last_scan: AtomicI64::new(0),
            });

            tray::build(app.handle())?;

            // No panel on Linux (ADR-0010): its tray delivers no click to
            // toggle one, so the window would be a webview nobody can open.
            // ponytail: destroyed rather than never built — tauri.conf.json has
            // no per-platform window list, and the cost is one hidden webview
            // for the moments before setup runs. Declare the panel window in
            // Rust under cfg(not(linux)) if that start-up cost ever shows.
            #[cfg(target_os = "linux")]
            if let Some(w) = app.get_webview_window("traypanel") {
                let _ = w.destroy();
            }

            // Hidden at-login start vs. normal launch: the window is created
            // hidden (tauri.conf.json visible:false) so there is no flash; show
            // it unless HIDDEN_FLAG is present. Either way the webview loads and
            // runs its initial scan.
            if !std::env::args().any(|a| a == HIDDEN_FLAG) {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                }
            }

            // Auto-check for updates on start (non-blocking), respecting the
            // saved setting. When an update is found, emit it for a listener to
            // surface; today the placeholder endpoint 404s so this quietly
            // no-ops until a signed release exists.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let auto = handle
                    .state::<AppState>()
                    .db
                    .lock()
                    .ok()
                    .and_then(|db| settings::get_settings(&db).ok())
                    .map(|s| s.auto_check_updates)
                    .unwrap_or(false);
                if auto {
                    let status = updater::check(&handle).await;
                    if status.state == "available" {
                        let _ = handle.emit("update-available", status.version);
                    }
                }
            });
            // Resident capture cadence (ADR-0005): scan every few hours so a
            // hidden app keeps recording even when the machine stays up across
            // days without a re-login. The on-mount frontend scan covers start;
            // this thread covers the long tail. Emits prices-rebuilt so a
            // visible Overview refreshes too.
            // ponytail: parked thread + 4h sleep, no timer framework needed.
            let handle = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(4 * 3600));
                if scan_now(&handle).is_ok() {
                    let _ = handle.emit("prices-rebuilt", ());
                }
            });

            // Refresh both price catalogs off the main thread; each loader falls
            // back to its cached snapshot on a fetch failure (LiteLLM then to its
            // bundled copy, OpenRouter to None — ADR-0009). Scans re-run this same
            // routine whenever they surface a Model no catalog covers.
            let handle = app.handle().clone();
            std::thread::spawn(move || refresh_catalogs(&handle));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            scan,
            last_scan,
            unreadable_artifacts,
            summary,
            trend,
            series,
            breakdown,
            ctx_resources,
            ctx_buckets,
            ctx_tools,
            ctx_skills,
            ctx_exec,
            model_pricing,
            refresh_prices,
            set_model_override,
            delete_model_override,
            show_main,
            open_settings,
            quit_app,
            resize_panel,
            get_settings,
            set_settings,
            check_updates,
            download_update,
            restart_app,
            save_csv
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::AppState;
    use crate::queries::Filters;
    use crate::scan::SourceRoots;
    use crate::{db, queries, scan};
    use std::sync::atomic::AtomicI64;
    use std::sync::Mutex;

    // Proves AppState constructs and the exact call-shapes used by the IPC
    // commands (run_scan + queries::summary) type-check against the real
    // functions. Empty fixture roots => 14 source statuses, zero events.
    #[test]
    fn appstate_wires_scan_and_query() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_db(&dir.path().join("tokenledger.db")).unwrap();
        let roots = SourceRoots {
            claude: dir.path().join("claude"),
            codex: dir.path().join("codex"),
            gemini_tmp: dir.path().join("gemini"),
            gemini_projects_json: dir.path().join("projects.json"),
            hermes_db: dir.path().join("state.db"),
            grok_sessions: dir.path().join("grok"),
            antigravity_conversations: dir.path().join("antigravity"),
            antigravity_cli_conversations: dir.path().join("antigravity-cli"),
            goose_sessions: vec![dir.path().join("goose")],
            pi_sessions: vec![dir.path().join("pi")],
            omp_sessions: vec![dir.path().join("omp")],
            opencode_data: dir.path().join("opencode"),
            opencode_legacy: dir.path().join("opencode/storage"),
            opencode_db: None,
            kilo_db: dir.path().join("kilo.db"),
            zed_databases: vec![dir.path().join("zed/threads/threads.db")],
            cline: vec![dir.path().join("cline")],
            workbuddy: dir.path().join("workbuddy"),
            codebuddy: dir.path().join("codebuddy"),
            qoder_databases: vec![dir.path().join("qoder.db")],
            qoder_cli_projects: vec![dir.path().join("qoder-cli")],
        };
        let state = AppState {
            db: Mutex::new(conn),
            roots,
            scan_lock: Mutex::new(()),
            price_lookups: Mutex::new(Default::default()),
            last_scan: AtomicI64::new(0),
        };

        let mut db = state.db.lock().unwrap();
        let status = scan::run_scan(&mut db, &state.roots);
        assert_eq!(status.sources.len(), 16);

        let sum = queries::summary(&db, &Filters::default()).unwrap();
        assert_eq!(sum.total_tokens, 0);
    }
}
