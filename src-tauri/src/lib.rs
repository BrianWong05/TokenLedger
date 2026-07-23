mod adapters;
mod db;
mod pricing;
mod queries;
mod scan;
mod settings;
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

// Shared cross-Source partition invariants + a hermetic seven-Source test that
// runs them on synthetic logs every `cargo test`. Test-only, like e2e above.
#[cfg(test)]
mod invariants;

use std::sync::Mutex;

use rusqlite::Connection;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::MacosLauncher;

use pricing::{ModelPricing, RatesPerTok};
use queries::{BreakdownRow, CtxBuckets, CtxExecRow, CtxResourceCount, CtxToolRow, Filters, SeriesPoint, Summary, TrendPoint};
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
    tray::refresh(app);
    Ok(status)
}

#[tauri::command]
async fn scan(app: AppHandle) -> Result<ScanStatus, String> {
    scan_now(&app)
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
) -> Result<Vec<CtxResourceCount>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    queries::ctx_resources(&db, &filters).map_err(|e| e.to_string())
}

#[tauri::command]
fn ctx_buckets(
    state: State<'_, AppState>,
    filters: Filters,
) -> Result<Vec<CtxBuckets>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    queries::ctx_buckets(&db, &filters).map_err(|e| e.to_string())
}

#[tauri::command]
fn ctx_tools(
    state: State<'_, AppState>,
    filters: Filters,
) -> Result<Vec<CtxToolRow>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    queries::ctx_tools(&db, &filters).map_err(|e| e.to_string())
}

#[tauri::command]
fn ctx_exec(
    state: State<'_, AppState>,
    filters: Filters,
) -> Result<Vec<CtxExecRow>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    queries::ctx_exec(&db, &filters).map_err(|e| e.to_string())
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
        // Launch at login as a LaunchAgent that passes HIDDEN_FLAG, so an
        // at-login start comes up hidden (tray only) while a manual launch does
        // not. Enrollment itself is driven from the frontend (first-run dialog +
        // Settings toggle → startup.ts).
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .macos_launcher(MacosLauncher::LaunchAgent)
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
            });

            tray::build(app.handle())?;

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

            // Refresh LiteLLM prices off the main thread; any fetch failure falls
            // back to the cached/bundled snapshot inside load_prices_json. The
            // blocking network fetch runs BEFORE the DB lock so scan/summary/etc.
            // never block behind it on cold start.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let json = pricing::load_prices_json(&data_dir); // no lock, network here
                let state = handle.state::<AppState>();
                if let Ok(mut db) = state.db.lock() {
                    let _ = pricing::rebuild_prices(&mut db, &json);
                };
                // Tell the frontend so it re-fetches costs: without this, a
                // fresh install renders 'unpriced' until the next range change.
                let _ = handle.emit("prices-rebuilt", ());
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            scan,
            summary,
            trend,
            series,
            breakdown,
            ctx_resources,
            ctx_buckets,
            ctx_tools,
            ctx_exec,
            model_pricing,
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
    use std::sync::Mutex;

    // Proves AppState constructs and the exact call-shapes used by the IPC
    // commands (run_scan + queries::summary) type-check against the real
    // functions. Empty fixture roots => 7 source statuses, zero events.
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
            pi_sessions: vec![dir.path().join("pi")],
        };
        let state = AppState {
            db: Mutex::new(conn),
            roots,
            scan_lock: Mutex::new(()),
        };

        let mut db = state.db.lock().unwrap();
        let status = scan::run_scan(&mut db, &state.roots);
        assert_eq!(status.sources.len(), 7);

        let sum = queries::summary(&db, &Filters::default()).unwrap();
        assert_eq!(sum.total_tokens, 0);
    }
}
