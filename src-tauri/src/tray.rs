// The Menu Bar Extra (CONTEXT.md): TokenLedger's resident menu-bar presence
// per ADR-0005 — the app lives here, not in a window you keep open. Today's
// tokens + Cost are computed here once and reach the user by whatever route
// the platform offers (ADR-0010): beside the icon on macOS, in the icon's
// hover text on Windows, as the first row of a menu on Linux. The icon toggles
// the traypanel webview window, which renders the panel design pixel-faithfully
// per ADR-0007 as amended (superseding ADR-0006's native menu) — except on
// Linux, whose tray delivers no click to toggle it with, and which gets the
// menu instead. Panel content and actions live in src/traypanel/; this file is
// the title math plus window glue.
use tauri::image::Image;
#[cfg(target_os = "linux")]
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
#[cfg(not(target_os = "linux"))]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
use tauri::{
    AppHandle, Emitter, Manager, Runtime, WebviewWindow, WebviewWindowBuilder, Wry,
};

// Held so each scan can rewrite Today's figures in place.
pub struct Tray {
    tray: TrayIcon<Wry>,
    // Re-texted rather than rebuilt, so a menu open under the user's cursor is
    // never yanked out from beneath it.
    #[cfg(target_os = "linux")]
    today: MenuItem<Wry>,
}

/// Builds the tray once, from setup: template glyph, live figures, and the way
/// in — a click that toggles the panel, or on Linux a menu.
pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let mut builder = TrayIconBuilder::new();

    #[cfg(not(target_os = "linux"))]
    {
        builder = builder
            .show_menu_on_left_click(false)
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    rect,
                    ..
                } = event
                {
                    let _ = toggle_panel(tray.app_handle(), rect);
                }
            });
    }

    #[cfg(target_os = "linux")]
    let (menu, today) = build_menu(app)?;
    #[cfg(target_os = "linux")]
    {
        builder = builder.menu(&menu).on_menu_event(on_menu_event);
    }

    // Design 2b's chart-line glyph as a macOS template image (black + alpha;
    // rasterized from the mock's mark into icons/tray.png). Its own file, not
    // the app icon: a template is flattened to one colour, so the app icon's
    // tile and its fill under the line would both reduce to a solid block.
    if let Ok(icon) = Image::from_bytes(include_bytes!("../icons/tray.png")) {
        builder = builder.icon(icon).icon_as_template(true);
    }
    let tray = builder.build(app)?;
    app.manage(Tray {
        tray,
        #[cfg(target_os = "linux")]
        today,
    });
    refresh(app);
    Ok(())
}

/// The Linux menu (ADR-0010), returning it with the Today row that refresh
/// re-texts. Today's figures lead and are disabled: they are a read-out, not
/// somewhere to click through to. ADR-0006's amendment records that choice
/// failing sign-off once on macOS, where disabled grey was too dim to read —
/// if GTK's is too, enabled-but-inert is the fix that ADR already landed.
#[cfg(target_os = "linux")]
fn build_menu(app: &AppHandle) -> tauri::Result<(Menu<Wry>, MenuItem<Wry>)> {
    // The row has to hold some text before the first refresh writes figures
    // into it. Normally that is the same instant — build calls refresh — but a
    // refresh that bails (a poisoned lock, a query that errors) leaves the
    // ellipsis standing, which claims nothing, rather than a figure that would.
    let today = MenuItem::with_id(app, "today", crate::readout::today_row("…"), false, None::<&str>)?;
    let menu = Menu::new(app)?;
    menu.append(&today)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(
        app,
        "open",
        "Open TokenLedger",
        true,
        None::<&str>,
    )?)?;
    menu.append(&MenuItem::with_id(app, "scan", "Scan now", true, None::<&str>)?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?)?;
    Ok((menu, today))
}

/// The Linux menu's three actions, each the same one the panel invokes
/// elsewhere.
#[cfg(target_os = "linux")]
fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        "open" => {
            let _ = show_main(app);
        }
        "scan" => {
            // Off the UI thread: a scan can take a moment. On completion, emit
            // the one event a visible Overview listens for so it re-reads the
            // Ledger. scan_now refreshes the tray itself.
            let app = app.clone();
            std::thread::spawn(move || {
                if crate::scan_now(&app).is_ok() {
                    let _ = app.emit("prices-rebuilt", ());
                }
            });
        }
        "quit" => app.exit(0),
        _ => {}
    }
}

/// The panel window's logical width — the one number lib.rs's resize_panel and
/// the fallback below hardcode. tauri.conf.json's traypanel entry and
/// TrayPanel.tsx's PANEL_WIDTH carry the same figure; tests pin each of them
/// to their conf (panel_width_matches_the_window_config here,
/// TrayPanel.css.test.js on the frontend).
///
/// Ungated: resize_panel is a `generate_handler!` command, so it compiles on
/// Linux too and reads this.
pub(crate) const PANEL_WIDTH: f64 = 320.0;

/// Show the panel beside the tray icon, or destroy it if it's already up. Where
/// the panel lands is panel_position's business; this gathers the icon rect,
/// the panel size, and the work area of the monitor holding the icon. The
/// panel fetches on mount; destroy-on-blur lives in lib.rs's window-event
/// handler. Absent on Linux, which never delivers the click that would call it.
#[cfg(not(target_os = "linux"))]
fn toggle_panel<R: Runtime>(app: &AppHandle<R>, rect: tauri::Rect) -> tauri::Result<()> {
    let w = if let Some(w) = app.get_webview_window("traypanel") {
        w
    } else {
        build_panel(app)?
    };
    if w.is_visible().unwrap_or(false) {
        close_panel(app);
        return Ok(());
    }
    let scale = w.scale_factor().unwrap_or(2.0);
    let icon = px_rect(rect, scale);
    let panel = w
        .outer_size()
        .map(|s| (f64::from(s.width), f64::from(s.height)))
        .unwrap_or((PANEL_WIDTH * scale, 480.0 * scale));
    // An unresolvable monitor collapses the work area to the icon, which
    // disables clamping (see clamp) and leaves the anchored position standing.
    let (cx, cy) = icon.center();
    let work = w
        .monitor_from_point(cx, cy)
        .ok()
        .flatten()
        .map(|m| {
            let a = m.work_area();
            PxRect {
                x: f64::from(a.position.x),
                y: f64::from(a.position.y),
                w: f64::from(a.size.width),
                h: f64::from(a.size.height),
            }
        })
        .unwrap_or(icon);
    let (x, y) = panel_position(icon, work, panel, 4.0 * scale);
    w.set_position(tauri::PhysicalPosition::new(x, y))?;
    w.show()?;
    w.set_focus()?;
    app.emit_to("traypanel", "panel-shown", ())?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn build_panel<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<WebviewWindow<R>> {
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == "traypanel")
        .expect("tray panel window config");
    WebviewWindowBuilder::from_config(app, config)?.build()
}

/// Dismissing the panel: destroyed, not hidden, so the next open remounts and
/// refetches. Every dismissal routes here — the icon clicked while it is up,
/// focus lost to another window (lib.rs), and Escape (the `close_panel`
/// command) — so the three cannot drift apart.
///
/// Ungated where the panel glue around it is compiled out on Linux
/// (ADR-0010), because `generate_handler!` takes no `#[cfg]` per entry, so the
/// command calling this must exist on every platform — as `resize_panel`'s
/// already does. Harmless there: Linux never creates the window, so this finds
/// nothing to destroy.
pub fn close_panel<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window("traypanel") {
        let _ = w.destroy();
    }
}

/// The tray's icon rect in physical pixels — the one unit panel_position
/// speaks, so the Logical/Physical split stops here.
#[cfg(not(target_os = "linux"))]
fn px_rect(rect: tauri::Rect, scale: f64) -> PxRect {
    let (x, y) = match rect.position {
        tauri::Position::Physical(p) => (f64::from(p.x), f64::from(p.y)),
        tauri::Position::Logical(l) => (l.x * scale, l.y * scale),
    };
    let (w, h) = match rect.size {
        tauri::Size::Physical(s) => (f64::from(s.width), f64::from(s.height)),
        tauri::Size::Logical(l) => (l.width * scale, l.height * scale),
    };
    PxRect { x, y, w, h }
}

/// Show + focus the main window; the panel's Open action and open_settings
/// both route here.
pub fn show_main<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let window = if let Some(window) = app.get_webview_window("main") {
        window
    } else {
        build_main(app, false)?
    };

    focus_main(&window)
}

/// Open Settings without racing the frontend listener on a newly-created
/// webview. Existing windows receive the event immediately; new ones receive
/// it after page load, once React has had a chance to mount its listener.
pub fn open_settings<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        focus_main(&window)?;
        window.emit("open-settings", ())?;
    } else {
        let window = build_main(app, true)?;
        focus_main(&window)?;
    }
    Ok(())
}

fn build_main<R: Runtime>(
    app: &AppHandle<R>,
    open_settings_on_load: bool,
) -> tauri::Result<WebviewWindow<R>> {
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == "main")
        .expect("main window config");
    let mut builder = WebviewWindowBuilder::from_config(app, config)?;
    if open_settings_on_load {
        builder = builder.on_page_load(|window, payload| {
            if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                let _ = window.emit("open-settings", ());
            }
        });
    }
    builder.build()
}

fn focus_main<R: Runtime>(window: &WebviewWindow<R>) -> tauri::Result<()> {
    window.show()?;
    window.unminimize()?;
    window.set_focus()?;
    Ok(())
}

/// Recomputes Today's figures (Today's Summary + Settings → bar_title) and
/// rewrites them in place, wherever this platform shows them. Called after
/// every scan and on settings save; no-op until the tray exists. Reads run on
/// `read_db` so opening the Menu Bar Extra during a scan never waits on it.
/// The lock is still released before show_today: those calls hop to the main
/// thread, and holding any connection across that hop invites a deadlock the
/// moment something on the main thread takes the same lock.
pub fn refresh(app: &AppHandle) {
    let Some(tray) = app.try_state::<Tray>() else {
        return;
    };
    let state = app.state::<crate::AppState>();
    let title = {
        let Ok(db) = state.read_db.lock() else { return };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let Ok((start, end)) = day_window(&db, now) else {
            return;
        };
        let filters = crate::queries::Filters {
            start_ts: Some(start),
            end_ts: Some(end),
            ..Default::default()
        };
        let (Ok(today), Ok(settings)) = (
            crate::queries::summary(&db, &filters),
            crate::settings::get_settings(&db),
        ) else {
            return;
        };
        let floor = crate::readout::tokens_are_floor(&crate::db::load_unreadable(&db), start);
        crate::readout::bar_title(&today, &settings, floor)
    };
    show_today(&tray, &title);
}

/// Puts Today's figures wherever this platform can show them. The three calls
/// are not interchangeable: set_title does nothing on Windows and set_tooltip
/// nothing on Linux, which is the whole reason the Menu Bar Extra wears three
/// faces (ADR-0010).
fn show_today(tray: &Tray, title: &str) {
    #[cfg(target_os = "macos")]
    let _ = tray.tray.set_title(Some(title));
    #[cfg(target_os = "windows")]
    let _ = tray.tray.set_tooltip(Some(title));
    #[cfg(target_os = "linux")]
    let _ = tray.today.set_text(crate::readout::today_row(title));
}

// --- Panel placement ---
// Pure: constructed inputs in, a position out. The Tauri glue above stays thin.

/// A rectangle in physical pixels, the unit every figure here is in.
#[derive(Clone, Copy, Debug)]
struct PxRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl PxRect {
    fn right(&self) -> f64 {
        self.x + self.w
    }
    fn bottom(&self) -> f64 {
        self.y + self.h
    }
    fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// Which screen edge the status area sits on — the menu bar's top on macOS,
/// wherever the user parked the taskbar on Windows.
enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

/// Where to put the panel so it hangs off the tray icon and stays on-screen.
/// Anchored to the boundary between the status area and the work area (a gap
/// clear of it), aligned with the icon's trailing edge along the bar, then
/// held inside the work area. Pure, so it stays compiled on Linux too and its
/// tests run on every platform of the matrix — only the caller is absent there.
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn panel_position(icon: PxRect, work: PxRect, panel: (f64, f64), gap: f64) -> (f64, f64) {
    let (pw, ph) = panel;
    let (x, y) = match status_edge(icon, work) {
        Edge::Top => (icon.right() - pw, icon.bottom().max(work.y) + gap),
        Edge::Bottom => (icon.right() - pw, icon.y.min(work.bottom()) - gap - ph),
        Edge::Left => (icon.right().max(work.x) + gap, icon.bottom() - ph),
        Edge::Right => (icon.x.min(work.right()) - gap - pw, icon.bottom() - ph),
    };
    (
        clamp_or_leave(x, work.x, work.right() - pw),
        clamp_or_leave(y, work.y, work.bottom() - ph),
    )
}

/// Which edge the status area runs along, read from where the icon sits
/// relative to the work area: a menu bar or taskbar is excluded from the work
/// area, so its icons lie outside the edge they occupy. Reading which side the
/// icon falls outside — rather than which edge it is nearest — is what keeps
/// an icon in a screen corner, close to two edges at once, on the bar it
/// actually sits in.
/// ponytail: Top doubles as the fallback for an icon that lands inside the
/// work area, which means the platform did not exclude its bar — it is macOS's
/// fixed truth and matches the pre-work-area behavior. Ask the platform for
/// the bar's own orientation if one ever reports a work area covering it.
fn status_edge(icon: PxRect, work: PxRect) -> Edge {
    let (cx, cy) = icon.center();
    if cy > work.bottom() {
        Edge::Bottom
    } else if cx < work.x {
        Edge::Left
    } else if cx > work.right() {
        Edge::Right
    } else {
        Edge::Top
    }
}

/// Clamps, except that a range too small to hold the panel — or an unknown
/// work area, which the caller collapses to the icon — leaves the anchored
/// position alone rather than inventing a corner out of an inverted range.
fn clamp_or_leave(v: f64, lo: f64, hi: f64) -> f64 {
    if hi < lo {
        v
    } else {
        v.clamp(lo, hi)
    }
}

// The title itself — figures, markers, currency — is readout.rs's: one home
// for how a Summary reads out, shared with the Linux menu row and the CSV
// report, and pinned to the frontend's Intl rendering by readout-cases.json.

/// [local midnight, next local midnight) as epoch seconds for the day
/// containing `now_epoch`. SQLite does the timezone math with the same
/// 'localtime' modifier the day buckets in queries.rs use, so the bar's
/// "Today" and the Overview's day buckets can never disagree. The resident loop
/// reads it too, to know when Today's figures stop being today's.
pub(crate) fn day_window(
    conn: &rusqlite::Connection,
    now_epoch: i64,
) -> rusqlite::Result<(i64, i64)> {
    conn.query_row(
        "SELECT unixepoch(?1, 'unixepoch', 'localtime', 'start of day', 'utc'), \
                unixepoch(?1, 'unixepoch', 'localtime', 'start of day', '+1 day', 'utc')",
        [now_epoch],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn tray_click_creates_and_shows_the_lazy_panel() {
        let app = tauri::test::mock_builder()
            .build(crate::app_context())
            .unwrap();
        let icon = tauri::Rect {
            position: tauri::Position::Physical(tauri::PhysicalPosition::new(100, 0)),
            size: tauri::Size::Physical(tauri::PhysicalSize::new(24, 24)),
        };

        toggle_panel(app.handle(), icon).unwrap();

        let panel = app
            .get_webview_window("traypanel")
            .expect("tray click should create the lazy panel");
        assert!(panel.is_visible().unwrap());
    }

    // Dismissal is one function, so the icon, focus loss and Escape cannot
    // drift apart. (That it *destroys* is not assertable here — the mock
    // runtime posts destroy to an event loop no unit test runs, so the window
    // map never updates. TrayPanel.test.tsx pins the key, and lib.rs's
    // every_command_the_panel_invokes_is_registered pins that the command
    // exists to receive it.)
    #[test]
    fn close_panel_is_a_no_op_without_a_panel() {
        let app = tauri::test::mock_builder()
            .build(crate::app_context())
            .unwrap();
        close_panel(app.handle());
        assert!(app.get_webview_window("traypanel").is_none());
    }

    // --- Panel placement ---
    // Physical pixels throughout. macOS numbers are a 3024×1964 retina screen
    // (scale 2, 48px menu bar, 600×960 panel); the Windows ones a 1920×1080
    // screen (scale 1, 48px taskbar, 300×480 panel). The panel sizes are
    // fixture geometry passed explicitly — panel_position is parametric, so
    // these stay valid whatever width the real panel ships at.

    const MAC_ICON: PxRect = PxRect { x: 2600.0, y: 0.0, w: 60.0, h: 48.0 };
    const MAC_WORK: PxRect = PxRect { x: 0.0, y: 48.0, w: 3024.0, h: 1916.0 };
    const MAC_PANEL: (f64, f64) = (600.0, 960.0);

    // Today's macOS placement, pinned: below the icon, right edges aligned.
    #[test]
    fn menu_bar_opens_the_panel_below_the_icon_right_aligned() {
        assert_eq!(
            panel_position(MAC_ICON, MAC_WORK, MAC_PANEL, 8.0),
            (2060.0, 56.0)
        );
    }

    #[test]
    fn bottom_taskbar_opens_the_panel_above_the_icon() {
        let icon = PxRect { x: 1700.0, y: 1044.0, w: 24.0, h: 24.0 };
        let work = PxRect { x: 0.0, y: 0.0, w: 1920.0, h: 1032.0 };
        // Right edges aligned; panel sits a gap above the taskbar, not above
        // the icon — icons are padded inside the bar, so anchoring to the icon
        // alone would overlap it.
        assert_eq!(panel_position(icon, work, (300.0, 480.0), 4.0), (1424.0, 548.0));
    }

    #[test]
    fn top_taskbar_opens_the_panel_below_the_icon() {
        let icon = PxRect { x: 1700.0, y: 12.0, w: 24.0, h: 24.0 };
        let work = PxRect { x: 0.0, y: 48.0, w: 1920.0, h: 1032.0 };
        assert_eq!(panel_position(icon, work, (300.0, 480.0), 4.0), (1424.0, 52.0));
    }

    #[test]
    fn left_taskbar_opens_the_panel_to_the_right_of_the_icon() {
        let icon = PxRect { x: 12.0, y: 900.0, w: 24.0, h: 24.0 };
        let work = PxRect { x: 48.0, y: 0.0, w: 1872.0, h: 1080.0 };
        // Bottom edges aligned — the vertical mirror of the right-alignment a
        // horizontal bar gets.
        assert_eq!(panel_position(icon, work, (300.0, 480.0), 4.0), (52.0, 444.0));
    }

    #[test]
    fn right_taskbar_opens_the_panel_to_the_left_of_the_icon() {
        let icon = PxRect { x: 1896.0, y: 900.0, w: 24.0, h: 24.0 };
        let work = PxRect { x: 0.0, y: 0.0, w: 1872.0, h: 1080.0 };
        assert_eq!(panel_position(icon, work, (300.0, 480.0), 4.0), (1568.0, 444.0));
    }

    // A tray icon in the screen's corner is close to two edges at once. It
    // still belongs to the bar it sits in, so the panel opens above it — not
    // flipped to its left as a nearest-edge reading would have it.
    #[test]
    fn corner_icon_belongs_to_the_bar_it_sits_in() {
        let icon = PxRect { x: 1890.0, y: 1044.0, w: 24.0, h: 24.0 };
        let work = PxRect { x: 0.0, y: 0.0, w: 1920.0, h: 1032.0 };
        assert_eq!(panel_position(icon, work, (300.0, 480.0), 4.0), (1614.0, 548.0));
    }

    // An icon far enough along the bar that the aligned edge would hang off
    // the screen: the panel slides back inside rather than rendering partly
    // off-monitor.
    #[test]
    fn panel_stays_inside_the_work_area() {
        let icon = PxRect { x: 40.0, y: 0.0, w: 60.0, h: 48.0 };
        assert_eq!(
            panel_position(icon, MAC_WORK, MAC_PANEL, 8.0),
            (0.0, 56.0)
        );
    }

    // A screen too short for the panel: no placement fits, so it stays against
    // the bar it belongs to (top cut off) rather than being jammed to a corner
    // that would cover the bar as well.
    #[test]
    fn screen_too_small_for_the_panel_keeps_it_against_the_bar() {
        let icon = PxRect { x: 700.0, y: 370.0, w: 24.0, h: 24.0 };
        let work = PxRect { x: 0.0, y: 0.0, w: 800.0, h: 360.0 };
        assert_eq!(panel_position(icon, work, (300.0, 480.0), 4.0), (424.0, -124.0));
    }

    // Degenerate work area (an unresolvable monitor collapses it to the icon):
    // the anchored position stands, which is exactly the pre-work-area
    // behavior — never a nonsense corner.
    #[test]
    fn unknown_work_area_leaves_the_anchored_position_alone() {
        assert_eq!(
            panel_position(MAC_ICON, MAC_ICON, MAC_PANEL, 8.0),
            (2060.0, 56.0)
        );
    }

    // Asset guard: the bundled template glyph must decode through the same
    // strict decoder the app uses (tauri's png feature) — sips/CoreImage
    // accepting a file proves nothing, and a corrupt asset fails silently
    // at runtime (icon-less tray).
    #[test]
    fn bundled_tray_icon_decodes() {
        assert!(
            Image::from_bytes(include_bytes!("../icons/tray.png")).is_ok(),
            "tray template icon must decode"
        );
    }

    #[test]
    fn day_window_brackets_the_local_calendar_day() {
        // Pinned to UTC like every queries.rs bucket test: parallel test
        // threads share the process TZ, so a non-UTC pin here would race
        // them. Under UTC the 'localtime' shift is a no-op — the shift path
        // itself is SQLite's own tested behavior.
        std::env::set_var("TZ", "UTC");
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // 2026-06-01T10:00:00Z (worked example from time.rs) → that UTC day.
        let (start, end) = day_window(&conn, 1_780_308_000).unwrap();
        assert_eq!(start, 1_780_272_000); // 2026-06-01T00:00:00Z
        assert_eq!(end, 1_780_358_400); // next midnight, end-exclusive
    }
}
