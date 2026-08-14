mod adapters;
mod db;
pub mod export_artifact;
pub mod limits_artifact;
// Crate-only: the Limits query is the one reader of the evidence, estimator,
// and readiness chain, and no companion binary imports any of the three.
pub(crate) mod limits_estimator;
pub(crate) mod limits_evidence;
pub(crate) mod limits_readiness;
mod pricing;
pub mod proto;
mod queries;
mod scan;
mod settings;
mod source_catalog;
// Public so the companion binaries share the crate's own time arithmetic rather
// than each re-deriving it.
pub mod time;
mod tray;
mod types;
mod updater;
pub mod uri;

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

// Opt-in, release-mode performance standard over a deterministic synthetic
// Ledger. Kept out of normal tests because it deliberately seeds 100k records.
#[cfg(test)]
mod performance;

// Opt-in CSV report over a window of this machine's Ledger, for the numbers
// outside the app. Test-only for the same reason as e2e above — it runs the
// private `queries` so its figures are the Overview's, not a reimplementation.
#[cfg(test)]
mod report;

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

use rusqlite::Connection;
use tauri::{AppHandle, Emitter, Manager, State};

use pricing::{ModelPricing, RatesPerTok};
use queries::{
    BreakdownRow, CtxBuckets, CtxExecRow, CtxResource, CtxSkillRow, CtxToolRow, Filters, SeriesPoint,
    SourceLimits, Summary, TrendPoint,
};
use scan::{run_scan, SourceRoots};
use settings::{Settings, UpdateStatus};
use types::ScanStatus;

// The at-login LaunchAgent starts the app with this flag so it comes up hidden
// (tray only); a normal Dock/Finder launch has no flag and shows the window.
const HIDDEN_FLAG: &str = "--hidden";

// A window-system close can request process exit after the final webview is
// destroyed. TokenLedger is a resident capture app, so only a programmatic
// exit (tray Quit or restart) may terminate it.
fn should_prevent_exit(code: Option<i32>) -> bool {
    code.is_none()
}

// Keep the generated macOS bundle metadata in one macro expansion. Besides
// avoiding duplicate embedded Info.plist symbols in tests, the generic runtime
// lets production use Wry while lifecycle tests use Tauri's mock runtime.
fn app_context<R: tauri::Runtime>() -> tauri::Context<R> {
    tauri::generate_context!()
}

pub struct AppState {
    pub db: Mutex<Connection>,
    /// Second connection to the same database (WAL), for reads. A scan holds
    /// `db` for its whole pass, so reads sharing that mutex queue behind it —
    /// at start-up that put the first paint behind the launch scan (measured
    /// in docs/performance.md, "startup first paint"). WAL readers never
    /// block on the writer; they see the last committed state, which is
    /// exactly what a provisional paint wants. Route reads through `read()`.
    pub read_db: Mutex<Connection>,
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

/// Run a read on the read connection. The db-vs-read_db choice every read
/// makes, made in one place so the next read cannot silently pick the write
/// connection and queue behind a scan.
fn read<T, E: std::fmt::Display>(
    state: &AppState,
    f: impl FnOnce(&Connection) -> Result<T, E>,
) -> Result<T, String> {
    let db = state.read_db.lock().map_err(|e| e.to_string())?;
    f(&db).map_err(|e| e.to_string())
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
        Some(json) => {
            read(&state, |db| pricing::ledger_publisher_targets(db, json)).unwrap_or_default()
        }
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
        let (Ok(db), Ok(mut attempted)) = (state.read_db.lock(), state.price_lookups.lock())
        else {
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

/// Whether a scan landed Limit Reading changes an open Limits page should
/// re-query for (spec: "Evaluation timing" — evaluate after an ordinary Scan
/// changes relevant facts). A scan that re-read everything and learned nothing
/// emits nothing, so an idle resident tick never wakes the page.
fn limits_changed(status: &ScanStatus) -> bool {
    status.sources.iter().any(|source| source.limit_readings > 0)
}

// The one scan path, shared by the `scan` command and the tray's "Scan now" so
// neither duplicates the locking/coalescing policy. Serialize scans: a second
// caller blocks on scan_lock, then runs its own incremental scan.
pub(crate) fn scan_now(app: &AppHandle) -> Result<ScanStatus, String> {
    let state = app.state::<AppState>();
    let _guard = state.scan_lock.lock().map_err(|e| e.to_string())?;
    let status = {
        // The scan holds `db` for its whole pass; the read commands run on
        // `read_db` (WAL) so the launch scan cannot queue the first paint.
        let mut db = state.db.lock().map_err(|e| e.to_string())?;
        run_scan(&mut db, &state.roots)
    };
    // Every scan lands here — the command, the tray's "Scan now", and the
    // resident capture — so the panel's freshness read-out cannot miss one.
    state.last_scan.store(status.scanned_at, Ordering::Relaxed);
    // ... and so an open Limits page cannot miss a scan that changed the
    // Readings it is showing. Same mechanism as prices-rebuilt: one event, and
    // the page reissues its ordinary query.
    if limits_changed(&status) {
        let _ = app.emit("limits-changed", ());
    }
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
#[tauri::command(async)]
fn unreadable_artifacts(state: State<'_, AppState>) -> Vec<types::SourceUnreadable> {
    read(&state, |db| Ok::<_, rusqlite::Error>(db::load_unreadable(db))).unwrap_or_default()
}

/// Decrypt Antigravity's `.pb` Sessions by running the `antigravity-export`
/// companion (ADR-0018), then report what it managed. This is the only place
/// the app reaches a Source, and it stays honest about the boundary three ways:
/// a person has to ask for it, it happens in a separate process, and the scan
/// itself never calls it. The companion writes Artifacts; the next scan reads
/// them like any other file.
#[tauri::command]
async fn export_antigravity(app: tauri::AppHandle) -> Result<String, String> {
    use tauri_plugin_shell::ShellExt;

    let output = app
        .shell()
        .sidecar("antigravity-export")
        .map_err(|e| format!("the export companion is missing from this build: {e}"))?
        .output()
        .await
        .map_err(|e| format!("could not run the export companion: {e}"))?;

    // The companion narrates progress on stderr and its verdict on stdout, so a
    // failure without stdout still has something to say.
    let report = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let failure = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.success() {
        Ok(if report.is_empty() { failure } else { report })
    } else {
        Err(if failure.is_empty() { report } else { failure })
    }
}

// The DB read commands are `(async)`: sync commands execute on the main
// thread, so a range switch's eight serialized queries (plus anything queued
// behind the connection mutex) would stall the event loop for their combined
// duration. Writes stay sync — rare, cheap, and set_settings drives the tray,
// which is main-thread territory.
#[tauri::command(async)]
fn summary(state: State<'_, AppState>, filters: Filters) -> Result<Summary, String> {
    read(&state, |db| queries::summary(db, &filters))
}

#[tauri::command(async)]
fn trend(
    state: State<'_, AppState>,
    filters: Filters,
    bucket: String,
) -> Result<Vec<TrendPoint>, String> {
    read(&state, |db| queries::trend(db, &filters, &bucket))
}

#[tauri::command(async)]
fn series(
    state: State<'_, AppState>,
    filters: Filters,
    bucket: String,
) -> Result<Vec<SeriesPoint>, String> {
    read(&state, |db| queries::series(db, &filters, &bucket))
}

#[tauri::command(async)]
fn breakdown(
    state: State<'_, AppState>,
    by: String,
    filters: Filters,
) -> Result<Vec<BreakdownRow>, String> {
    read(&state, |db| queries::breakdown(db, &by, &filters))
}

#[tauri::command(async)]
fn ctx_resources(
    state: State<'_, AppState>,
    filters: Filters,
) -> Result<Vec<CtxResource>, String> {
    read(&state, |db| queries::ctx_resources(db, &filters))
}

#[tauri::command(async)]
fn ctx_buckets(state: State<'_, AppState>, filters: Filters) -> Result<Vec<CtxBuckets>, String> {
    read(&state, |db| queries::ctx_buckets(db, &filters))
}

#[tauri::command(async)]
fn ctx_tools(state: State<'_, AppState>, filters: Filters) -> Result<Vec<CtxToolRow>, String> {
    read(&state, |db| queries::ctx_tools(db, &filters))
}

#[tauri::command(async)]
fn ctx_skills(state: State<'_, AppState>, filters: Filters) -> Result<Vec<CtxSkillRow>, String> {
    read(&state, |db| queries::ctx_skills(db, &filters))
}

#[tauri::command(async)]
fn ctx_exec(state: State<'_, AppState>, filters: Filters) -> Result<Vec<CtxExecRow>, String> {
    read(&state, |db| queries::ctx_exec(db, &filters))
}

/// The current state of every Limit the Ledger holds Readings for. Takes no
/// Filters: the Limits page is *now*, not a range, and it ignores the Overview's
/// date window and Source selection entirely.
#[tauri::command(async)]
fn limits(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<Vec<SourceLimits>, String> {
    // One evaluation instant for the whole page, injected here: every window's
    // estimate is answered as of the same second, and a storage fault or a
    // broken invariant rejects the command rather than appearing as a state.
    let evaluated_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .map_err(|e| e.to_string())?;
    let mut cards = read(&state, |db| queries::limits(db, evaluated_at))?;
    if let Some(export) = limits_artifact::read(&limit_exports_dir(&app), "codex") {
        if let Some(card) = cards.iter_mut().find(|card| card.source == "codex") {
            card.usage_resets_available = export.usage_resets_available;
        }
    }
    Ok(cards)
}

/// Ask a `live` Source's Companion for a fresh reading (ADR-0019). This is the
/// only place the app reaches a vendor with a credential, and it stays honest
/// about the boundary the same three ways `export_antigravity` does: a person has
/// to ask for it, it happens in a separate process, and the scan itself never
/// calls it. The Companion writes an Export Artifact; this reads that file
/// through the same schema-checked path the scan uses, so the page has its
/// figures without waiting for a scan and a later scan re-reading the file is a
/// no-op. The Companion's stdout is an echo for inspection, never the ingest
/// path — which is why a Companion that cannot write its Artifact exits non-zero
/// rather than reporting success having delivered nothing.
///
/// Errs with the Companion's own failure line, which the page classifies: a
/// missing or refused credential reads as "not signed in", anything else as
/// "couldn't check". The two must never collapse into one another.
#[tauri::command]
async fn check_live_limits(app: tauri::AppHandle, source: String) -> Result<(), String> {
    use tauri_plugin_shell::ShellExt;

    // One Companion per `live` Source, named `<source>-limits` — the catalog
    // decides whether the Source asking is entitled to a live check at all, and
    // the name never comes from the frontend unchecked.
    if source_catalog::source(&source).and_then(|s| s.capabilities.limits.as_deref()) != Some("live")
    {
        return Err(format!("{source} has no live Limits to check"));
    }
    let dir = limit_exports_dir(&app);
    let output = app
        .shell()
        .sidecar(format!("{source}-limits"))
        .map_err(|e| format!("the limits companion is missing from this build: {e}"))?
        .env("TOKENLEDGER_LIMITS_DIR", dir.to_string_lossy().to_string())
        .output()
        .await
        .map_err(|e| format!("could not run the limits companion: {e}"))?;

    if !output.status.success() {
        let failure = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if failure.is_empty() {
            "the limits companion failed without saying why".to_string()
        } else {
            failure
        });
    }

    // The Artifact it just wrote, read through the same schema-checked path the
    // scan uses — one reader, so the two can never drift. The change count is
    // not needed here: the page that asked re-queries as this command settles.
    let state = app.state::<AppState>();
    let mut db = state.db.lock().map_err(|e| e.to_string())?;
    limits_artifact::ingest(&mut db, &dir, &source).map(|_| ())
}

fn limit_exports_dir(app: &AppHandle) -> std::path::PathBuf {
    app.path()
        .app_data_dir()
        .map(|dir| dir.join("limits"))
        .unwrap_or_default()
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

#[tauri::command(async)]
fn model_pricing(state: State<'_, AppState>) -> Result<Vec<ModelPricing>, String> {
    read(&state, pricing::model_pricing)
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
async fn show_main(app: AppHandle) -> Result<(), String> {
    tray::show_main(&app).map_err(|error| error.to_string())
}

#[tauri::command]
async fn open_settings(app: AppHandle) -> Result<(), String> {
    tray::open_settings(&app).map_err(|error| error.to_string())
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

#[tauri::command(async)]
fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    read(&state, settings::get_settings)
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
//
// `(async)` for the same reason the DB reads carry it, except here a sync
// command does not merely stall the event loop — it deadlocks. A sync command
// runs on the main thread, inside wry's URL-scheme handler, and
// `blocking_save_file` then waits for a panel only the main thread can present.
// The thread waits on itself: no dialog ever appears and the app stops
// answering the window server.
#[tauri::command(async)]
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
    let app = tauri::Builder::default()
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
        .plugin(tauri_plugin_shell::init())
        // Closing `main` uses Tauri's default lifecycle, destroying that
        // webview and releasing its renderer memory. The run-event handler
        // below keeps the Rust capture process resident; Quit lives in the tray.
        .on_window_event(|window, event| {
            // The traypanel behaves like a menu: clicking anywhere else
            // (focus loss) dismisses it.
            if let tauri::WindowEvent::Focused(false) = event {
                if window.label() == "traypanel" && window.is_visible().unwrap_or(false) {
                    let _ = window.destroy();
                }
            }
        })
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let conn = db::open_db(&data_dir.join("tokenledger.db"))?;
            // Opened second, after the first open has settled WAL mode and the
            // migrations — for this one both are no-ops.
            let read_conn = db::open_db(&data_dir.join("tokenledger.db"))?;
            app.manage(AppState {
                db: Mutex::new(conn),
                read_db: Mutex::new(read_conn),
                // The Companions' output directory is the app's own, not
                // something to find under home — so it is filled in here, where
                // the data dir is already resolved.
                roots: SourceRoots {
                    limit_exports: data_dir.join("limits"),
                    ..SourceRoots::default_roots()
                },
                scan_lock: Mutex::new(()),
                price_lookups: Mutex::new(Default::default()),
                last_scan: AtomicI64::new(0),
            });

            tray::build(app.handle())?;

            // Hidden at-login start has no main webview at all. A manual launch
            // creates it from the lazy tauri.conf.json entry without a flash.
            let hidden_startup = std::env::args().any(|argument| argument == HIDDEN_FLAG);
            if hidden_startup {
                // With no frontend mount to perform the start-up capture, do it
                // in Rust so ADR-0005's "on start" guarantee still holds.
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    if scan_now(&handle).is_ok() {
                        let _ = handle.emit("prices-rebuilt", ());
                    }
                });
            } else {
                tray::show_main(app.handle())?;
            }

            // Auto-check for updates on start (non-blocking), respecting the
            // saved setting. When an update is found, emit it for a listener to
            // surface; today the placeholder endpoint 404s so this quietly
            // no-ops until a signed release exists.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let auto = read(&handle.state::<AppState>(), settings::get_settings)
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
            // days without a re-login. The on-mount frontend scan covers a
            // manual start; the hidden-start branch above covers login start;
            // this thread covers the long tail. Emits prices-rebuilt so a
            // visible Overview refreshes too.
            // Every sixth tick also refreshes prices: daily catalog checks keep
            // today's Codex Auto Review snapshot current without adding another
            // timer thread. Past snapshots are immutable once their day closes.
            // ponytail: parked thread + 4h sleep, no timer framework needed.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let mut ticks = 0;
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(4 * 3600));
                    if scan_now(&handle).is_ok() {
                        let _ = handle.emit("prices-rebuilt", ());
                    }
                    ticks += 1;
                    if ticks == 6 {
                        refresh_catalogs(&handle);
                        ticks = 0;
                    }
                }
            });

            // Refresh both price catalogs off the main thread; each loader falls
            // back to its cached snapshot on a fetch failure (LiteLLM then to its
            // bundled copy, OpenRouter to None — ADR-0009). Scans re-run this same
            // routine whenever they surface a Model no catalog covers, and the
            // resident cadence above re-runs it daily.
            let handle = app.handle().clone();
            std::thread::spawn(move || refresh_catalogs(&handle));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            scan,
            last_scan,
            unreadable_artifacts,
            export_antigravity,
            summary,
            trend,
            series,
            breakdown,
            ctx_resources,
            ctx_buckets,
            ctx_tools,
            ctx_skills,
            ctx_exec,
            limits,
            check_live_limits,
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
        .build(app_context())
        .expect("error while building tauri application");

    app.run(|_app, event| {
        if let tauri::RunEvent::ExitRequested { code, api, .. } = event {
            if should_prevent_exit(code) {
                api.prevent_exit();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::AppState;
    use crate::queries::Filters;
    use crate::scan::SourceRoots;
    use crate::{db, queries, scan};
    use std::sync::atomic::AtomicI64;
    use std::sync::Mutex;
    use tauri::Manager;

    #[test]
    fn hidden_startup_does_not_create_the_main_webview() {
        let app = tauri::test::mock_builder()
            .build(super::app_context())
            .unwrap();

        let main = app
            .config()
            .app
            .windows
            .iter()
            .find(|window| window.label == "main")
            .expect("main window config");
        assert!(!main.create, "main must be opted out of eager creation");
        assert!(app.get_webview_window("main").is_none());
    }

    #[test]
    fn startup_does_not_create_the_tray_panel_webview() {
        let app = tauri::test::mock_builder()
            .build(super::app_context())
            .unwrap();

        let tray_panel = app
            .config()
            .app
            .windows
            .iter()
            .find(|window| window.label == "traypanel")
            .expect("tray panel window config");
        assert!(
            !tray_panel.create,
            "tray panel must be opted out of eager creation"
        );
        assert!(app.get_webview_window("traypanel").is_none());
    }

    #[test]
    fn show_main_creates_and_reuses_the_lazy_webview() {
        let app = tauri::test::mock_builder()
            .build(super::app_context())
            .unwrap();

        crate::tray::show_main(app.handle()).unwrap();
        let first = app
            .get_webview_window("main")
            .expect("show_main should create the lazy webview");
        assert!(first.is_visible().unwrap());

        crate::tray::show_main(app.handle()).unwrap();
        assert!(app.get_webview_window("main").is_some());
    }

    /// Every `live` Source needs three separate files to agree before its card
    /// can ever fetch: the catalog says it is live, `tauri.conf.json` ships a
    /// Companion by that name, and the window's capability lets the shell run
    /// it. Any one of them missing fails only at the moment a person presses
    /// **Enable** — which is how the v1 wiring shipped unverified. Here the
    /// three are read and compared instead.
    #[test]
    fn every_live_source_has_a_companion_that_the_shell_is_allowed_to_run() {
        let read = |path: &str| {
            serde_json::from_str::<serde_json::Value>(
                &std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/").to_string() + path)
                    .unwrap_or_else(|e| panic!("{path}: {e}")),
            )
            .unwrap_or_else(|e| panic!("{path}: {e}"))
        };
        let conf = read("tauri.conf.json");
        let external: Vec<&str> = conf
            .pointer("/bundle/externalBin")
            .and_then(|b| b.as_array())
            .expect("the bundle must declare its external binaries")
            .iter()
            .filter_map(|b| b.as_str())
            .collect();
        let capability = read("capabilities/default.json");
        let allowed: Vec<&serde_json::Value> = capability
            .pointer("/permissions")
            .and_then(|p| p.as_array())
            .expect("the window capability must list permissions")
            .iter()
            .filter(|p| p.get("identifier").and_then(|i| i.as_str()) == Some("shell:allow-execute"))
            .filter_map(|p| p.get("allow").and_then(|a| a.as_array()))
            .flatten()
            .collect();

        let live: Vec<&str> = crate::source_catalog::catalog()
            .sources
            .iter()
            .filter(|s| s.capabilities.limits.as_deref() == Some("live"))
            .map(|s| s.key.as_str())
            .collect();
        assert!(live.contains(&"antigravity"), "the catalog decides who is live: {live:?}");

        for key in live {
            let name = format!("binaries/{key}-limits");
            assert!(external.contains(&name.as_str()), "{name} is not shipped with the app");
            let entry = allowed
                .iter()
                .find(|a| a.get("name").and_then(|n| n.as_str()) == Some(name.as_str()))
                .unwrap_or_else(|| panic!("the shell may not run {name}"));
            assert_eq!(entry.get("sidecar").and_then(|s| s.as_bool()), Some(true));
            // The Companions' one hand-run diagnostic is `--shape`, and the app
            // must never be able to pass it: no argument reaches a Companion
            // from the frontend.
            assert_eq!(entry.get("args").and_then(|a| a.as_bool()), Some(false));
            assert!(
                std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bin"))
                    .join(format!("{key}-limits.rs"))
                    .is_file(),
                "{key}-limits has no source to build from",
            );
        }
    }

    /// The scan-to-page seam (spec: "Evaluation timing"): a scan that landed
    /// Reading changes must notify, one that learned nothing must not — the
    /// gate `scan_now` emits `limits-changed` through.
    #[test]
    fn a_scan_notifies_the_limits_page_exactly_when_readings_changed() {
        let status = |limit_readings| crate::types::ScanStatus {
            sources: vec![crate::types::SourceStatus {
                source: "codex".to_string(),
                events_inserted: 0,
                lines_skipped: 0,
                limit_readings,
                artifacts_unreadable: 0,
                unreadable_max_mtime: None,
                error: None,
            }],
            scanned_at: 0,
        };
        assert!(super::limits_changed(&status(1)));
        assert!(!super::limits_changed(&status(0)));
    }

    #[test]
    fn resident_app_prevents_automatic_exit_but_allows_explicit_quit() {
        assert!(super::should_prevent_exit(None));
        assert!(!super::should_prevent_exit(Some(0)));
    }

    // Proves AppState constructs and the exact call-shapes used by the IPC
    // commands (run_scan + queries::summary) type-check against the real
    // functions. Empty fixture roots => one status per catalog Source, zero events.
    #[test]
    fn appstate_wires_scan_and_query() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_db(&dir.path().join("tokenledger.db")).unwrap();
        let read_conn = db::open_db(&dir.path().join("tokenledger.db")).unwrap();
        let roots = SourceRoots {
            claude: dir.path().join("claude"),
            codex_sessions: vec![dir.path().join("codex")],
            copilot_db: dir.path().join("copilot/session-store.db"),
            gemini_tmp: dir.path().join("gemini"),
            gemini_projects_json: dir.path().join("projects.json"),
            hermes_db: dir.path().join("state.db"),
            grok_sessions: dir.path().join("grok"),
            grok_logs: dir.path().join("grok-logs"),
            antigravity_conversations: dir.path().join("antigravity"),
            antigravity_ide_conversations: dir.path().join("antigravity-ide"),
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
            limit_exports: dir.path().join("limits"),
        };
        let state = AppState {
            db: Mutex::new(conn),
            read_db: Mutex::new(read_conn),
            roots,
            scan_lock: Mutex::new(()),
            price_lookups: Mutex::new(Default::default()),
            last_scan: AtomicI64::new(0),
        };

        let mut db = state.db.lock().unwrap();
        let status = scan::run_scan(&mut db, &state.roots);
        assert_eq!(status.sources.len(), 17);

        // The IPC read commands query through `read` (the second connection);
        // a scan's committed writes must be visible there.
        let sum = super::read(&state, |db| queries::summary(db, &Filters::default())).unwrap();
        assert_eq!(sum.total_tokens, 0);
    }
}
