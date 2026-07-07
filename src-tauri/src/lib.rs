mod adapters;
mod db;
mod pricing;
mod queries;
mod scan;
mod time;
mod types;

use std::sync::Mutex;

use rusqlite::Connection;
use tauri::{Manager, State};

use pricing::OverrideRates;
use queries::{BreakdownRow, Filters, Summary, TrendPoint};
use scan::{run_scan, SourceRoots};
use types::ScanStatus;

pub struct AppState {
    pub db: Mutex<Connection>,
    pub roots: SourceRoots,
    pub scan_lock: Mutex<()>,
}

#[tauri::command]
async fn scan(state: State<'_, AppState>) -> Result<ScanStatus, String> {
    // Serialize scans: a second caller blocks on scan_lock, then runs its own
    // incremental scan. That is the coalescing policy.
    let _guard = state.scan_lock.lock().map_err(|e| e.to_string())?;
    // ponytail: single Mutex<Connection> per the AppState contract. A scan
    // briefly blocks reads; incremental scans are cheap, so no separate read
    // connection. Add one only if UI jank during scans is ever measured.
    let mut db = state.db.lock().map_err(|e| e.to_string())?;
    Ok(run_scan(&mut db, &state.roots))
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
fn breakdown(
    state: State<'_, AppState>,
    by: String,
    filters: Filters,
) -> Result<Vec<BreakdownRow>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    queries::breakdown(&db, &by, &filters).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_price_override(
    state: State<'_, AppState>,
    model: String,
    rates: OverrideRates,
) -> Result<(), String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    pricing::set_override(&db, &model, rates).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_price_override(state: State<'_, AppState>, model: String) -> Result<(), String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    pricing::delete_override(&db, &model).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let conn = db::open_db(&data_dir.join("tokenledger.db"))?;
            app.manage(AppState {
                db: Mutex::new(conn),
                roots: SourceRoots::default_roots(),
                scan_lock: Mutex::new(()),
            });
            // Refresh LiteLLM prices off the main thread; any fetch failure falls
            // back to the cached/bundled snapshot inside refresh_prices.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let state = handle.state::<AppState>();
                if let Ok(mut db) = state.db.lock() {
                    let _ = pricing::refresh_prices(&mut db, &data_dir);
                };
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            scan,
            summary,
            trend,
            breakdown,
            set_price_override,
            delete_price_override
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
    // functions. Empty fixture roots => 4 source statuses, zero events.
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
        };
        let state = AppState {
            db: Mutex::new(conn),
            roots,
            scan_lock: Mutex::new(()),
        };

        let mut db = state.db.lock().unwrap();
        let status = scan::run_scan(&mut db, &state.roots);
        assert_eq!(status.sources.len(), 4);

        let sum = queries::summary(&db, &Filters::default()).unwrap();
        assert_eq!(sum.total_tokens, 0);
    }
}
