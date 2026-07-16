// The resident menu-bar tray (ADR-0005): TokenLedger lives in the tray, not in a
// window you keep open. Menu mirrors design 1f — Open / Scan now / a disabled
// "last scan" caption / Quit (⌘Q). "Scan now" reuses the exact scan the `scan`
// command runs (crate::scan_now), so there is no second scan code path.
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, Wry};

use crate::scan_now;
use crate::types::ScanStatus;

// Held so each scan can rewrite the caption's text in place without rebuilding
// the whole menu.
pub struct TrayMenu {
    last_scan: MenuItem<Wry>,
}

/// Builds the tray once, from setup. Uses the app's own icon as a macOS template
/// image so it inverts with the menu bar.
pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open TokenLedger", true, None::<&str>)?;
    let scan = MenuItem::with_id(app, "scan", "Scan now", true, None::<&str>)?;
    // Disabled caption; its text is replaced after every scan (set_last_scan).
    let last_scan = MenuItem::with_id(app, "last_scan", "Last scan: never", false, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit TokenLedger", true, Some("CmdOrCtrl+Q"))?;
    let menu = Menu::with_items(app, &[&open, &scan, &last_scan, &sep, &quit])?;

    app.manage(TrayMenu { last_scan });

    let mut builder = TrayIconBuilder::new()
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(on_menu_event);
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone()).icon_as_template(true);
    }
    builder.build(app)?;
    Ok(())
}

fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        "open" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }
        "scan" => {
            // Off the UI thread: a scan can take a moment. On completion, emit the
            // one event a visible Overview listens for so it re-reads the DB
            // (prices-rebuilt is the store's only "reload" signal — reusing it
            // avoids a new frontend listener the Overview wave owns).
            let app = app.clone();
            std::thread::spawn(move || {
                if scan_now(&app).is_ok() {
                    let _ = app.emit("prices-rebuilt", ());
                }
            });
        }
        "quit" => app.exit(0),
        _ => {}
    }
}

/// Refreshes the tray's last-scan caption after a scan. No-op when the tray was
/// never built. ponytail: shows the new-event count, not a live "N minutes ago"
/// — the latter needs a per-second timer this caption does not merit.
pub fn set_last_scan(app: &AppHandle, status: &ScanStatus) {
    if let Some(tray) = app.try_state::<TrayMenu>() {
        let events: u64 = status.sources.iter().map(|s| s.events_inserted).sum();
        let _ = tray
            .last_scan
            .set_text(format!("Last scan: {events} new events"));
    }
}
