// The Menu Bar Extra (CONTEXT.md): TokenLedger's resident menu-bar presence
// per ADR-0005 — the app lives here, not in a window you keep open. Today's
// tokens + Cost are computed here once and reach the user by whatever route
// the platform offers (ADR-0010): beside the icon on macOS, in the icon's
// hover text on Windows, as the first row of a menu on Linux. The icon toggles
// the traypanel webview window, which renders design 2b pixel-faithfully per
// ADR-0007 (superseding ADR-0006's native menu) — except on Linux, whose tray
// delivers no click to toggle it with, and which gets the menu instead. Panel
// content and actions live in src/traypanel/; this file is the title math plus
// window glue.
use tauri::image::Image;
#[cfg(target_os = "linux")]
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
#[cfg(not(target_os = "linux"))]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Wry};

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
                    toggle_panel(tray.app_handle(), rect);
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
    let today = MenuItem::with_id(app, "today", today_row("…"), false, None::<&str>)?;
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
        "open" => show_main(app),
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

/// Show the panel beside the tray icon, or hide it if it's already up. Where
/// the panel lands is panel_position's business; this gathers the icon rect,
/// the panel size, and the work area of the monitor holding the icon. The
/// panel refetches on every show via the panel-shown event; hide-on-blur lives
/// in lib.rs's window-event handler. Absent on Linux, which never delivers the
/// click that would call it.
#[cfg(not(target_os = "linux"))]
fn toggle_panel(app: &AppHandle, rect: tauri::Rect) {
    let Some(w) = app.get_webview_window("traypanel") else {
        return;
    };
    if w.is_visible().unwrap_or(false) {
        let _ = w.hide();
        return;
    }
    let scale = w.scale_factor().unwrap_or(2.0);
    let icon = px_rect(rect, scale);
    let panel = w
        .outer_size()
        .map(|s| (f64::from(s.width), f64::from(s.height)))
        .unwrap_or((300.0 * scale, 480.0 * scale));
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
    let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
    let _ = w.show();
    let _ = w.set_focus();
    let _ = app.emit_to("traypanel", "panel-shown", ());
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
pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Recomputes Today's figures (Today's Summary + Settings → tray_title) and
/// rewrites them in place, wherever this platform shows them. Called after
/// every scan and on settings save; no-op until the tray exists. The db lock
/// is released before show_today: every one of those calls hops to the main
/// thread, and sync commands on the main thread take the same lock — holding
/// it here would deadlock.
pub fn refresh(app: &AppHandle) {
    let Some(tray) = app.try_state::<Tray>() else {
        return;
    };
    let state = app.state::<crate::AppState>();
    let title = {
        let Ok(db) = state.db.lock() else { return };
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
        tray_title(&today, &settings)
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
    let _ = tray.today.set_text(today_row(title));
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

// --- Menu Bar Extra title (design 2b) ---
// Pure: constructed inputs in, strings out. The Tauri glue above stays thin.

/// The bar title for Today's Summary: "3.4M · $12.84", and "0 · $0.00" on a
/// day with no usage — a day that recorded nothing has a Cost of zero, not a
/// missing one (queries::summary), so it needs no case of its own here. Cost
/// follows the glossary: "≥ " marker when Partial (priced total over a set
/// with Unpriced Models or Unattributed Usage), and a day with usage but no
/// available Cost shows tokens alone — never $0. ponytail: the bar drops the
/// missing-Cost wording for space; the menu's per-Source rows (#24) spell it
/// out.
fn tray_title(today: &crate::queries::Summary, settings: &crate::settings::Settings) -> String {
    let toks = fmt_tokens(today.total_tokens);
    match today.cost {
        None => toks,
        Some(c) => {
            let marker = if today.has_unpriced || today.unattributed_tokens > 0 { "≥ " } else { "" };
            format!("{toks} · {marker}{cost}", cost = fmt_cost(c, settings))
        }
    }
}

/// The Linux menu's Today row: the bar title, prefixed — a menu row, unlike a
/// title welded to the icon, has to name what it is counting.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn today_row(title: &str) -> String {
    format!("Today: {title}")
}

/// Token total in the frontend's compact form (format.ts
/// formatCompactTokenTotal): K/M/B suffix, up to 2 decimals with trailing
/// zeros trimmed, and the same 0.999995 rollover so 999,999 reads "1M".
fn fmt_tokens(n: i64) -> String {
    const UNITS: [(f64, &str); 3] = [(1e9, "B"), (1e6, "M"), (1e3, "K")];
    let n = n.max(0) as f64;
    for (div, suffix) in UNITS {
        if n >= div * 0.999995 {
            let s = format!("{:.2}", n / div);
            let s = s.trim_end_matches('0').trim_end_matches('.');
            return format!("{s}{suffix}");
        }
    }
    format!("{}", n as i64)
}

/// A USD Cost rendered in the Display Currency — the display-time
/// multiplication of ADR-0002: USD passes through, anything else multiplies
/// by the user's fixed usd_rate; stored figures never leave USD.
/// ponytail: hand-rolled symbol map + comma grouping (Rust has no Intl),
/// "21.00 SEK" fallback for unmapped codes. Not full Intl parity: locale
/// digit/symbol tables (e.g. zh-Hant variants) are the upgrade path if a
/// mismatch is ever reported.
fn fmt_cost(usd: f64, s: &crate::settings::Settings) -> String {
    let amount = if s.currency == "USD" { usd } else { usd * s.usd_rate };
    let (sym, dec): (&str, usize) = match s.currency.as_str() {
        "USD" => ("$", 2),
        "EUR" => ("€", 2),
        "GBP" => ("£", 2),
        "HKD" => ("HK$", 2),
        "TWD" => ("NT$", 2),
        "CNY" => ("CN¥", 2),
        "JPY" => ("¥", 0),
        "KRW" => ("₩", 0),
        code => return format!("{} {code}", fmt_amount(amount, 2)),
    };
    format!("{sym}{}", fmt_amount(amount, dec))
}

/// Rounds to `dec` places and comma-groups the integer part, matching the
/// frontend's Intl output ("1,560.00"). Costs are non-negative by
/// construction (list rates × token counts), so no sign handling.
fn fmt_amount(amount: f64, dec: usize) -> String {
    let s = format!("{amount:.dec$}");
    let (int, frac) = s.split_once('.').map_or((s.as_str(), ""), |(i, f)| (i, f));
    let mut grouped = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    if frac.is_empty() {
        grouped
    } else {
        format!("{grouped}.{frac}")
    }
}

/// [local midnight, next local midnight) as epoch seconds for the day
/// containing `now_epoch`. SQLite does the timezone math with the same
/// 'localtime' modifier the day buckets in queries.rs use, so the bar's
/// "Today" and the Overview's day buckets can never disagree.
fn day_window(conn: &rusqlite::Connection, now_epoch: i64) -> rusqlite::Result<(i64, i64)> {
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
    use crate::queries::Summary;
    use crate::settings::Settings;

    fn sum(total_tokens: i64, cost: Option<f64>, has_unpriced: bool) -> Summary {
        Summary {
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            total_tokens,
            requests: 0,
            cost,
            has_unpriced,
            unattributed_tokens: 0,
            unpriced_models: vec![],
            cache_estimated_models: vec![],
            cache_hit_rate: 0.0,
            convs: 0,
        }
    }

    // Zero is a figure the day has, so the bar reads it out — the icon never
    // stands alone wondering whether the app is still counting. The Some(0.0)
    // is what queries::summary reports for a window holding no usage; the
    // no-usage-costs-zero rule lives there, not here.
    #[test]
    fn no_usage_day_reads_zero_in_the_display_currency() {
        assert_eq!(
            tray_title(&sum(0, Some(0.0), false), &Settings::default()),
            "0 · $0.00"
        );
        assert_eq!(
            tray_title(&sum(0, Some(0.0), false), &currency("JPY", 155.0)),
            "0 · ¥0"
        );
    }

    #[test]
    fn plain_day_shows_tokens_and_cost() {
        assert_eq!(
            tray_title(&sum(3_400_000, Some(12.84), false), &Settings::default()),
            "3.4M · $12.84"
        );
    }

    #[test]
    fn partial_cost_carries_the_marker() {
        assert_eq!(
            tray_title(&sum(3_400_000, Some(12.8), true), &Settings::default()),
            "3.4M · ≥ $12.80"
        );
    }

    #[test]
    fn unattributed_usage_marks_priced_cost_partial() {
        let mut today = sum(3_400_000, Some(12.8), false);
        today.unattributed_tokens = 400;
        assert_eq!(
            tray_title(&today, &Settings::default()),
            "3.4M · ≥ $12.80"
        );
    }

    #[test]
    fn all_unattributed_day_shows_tokens_alone() {
        let mut today = sum(964_200, None, false);
        today.unattributed_tokens = 964_200;
        assert_eq!(
            tray_title(&today, &Settings::default()),
            "964.2K"
        );
    }

    fn currency(code: &str, rate: f64) -> Settings {
        Settings {
            currency: code.to_string(),
            usd_rate: rate,
            ..Settings::default()
        }
    }

    #[test]
    fn display_currency_multiplies_and_uses_its_symbol() {
        assert_eq!(
            tray_title(&sum(3_400_000, Some(10.0), false), &currency("HKD", 7.8)),
            "3.4M · HK$78.00"
        );
    }

    #[test]
    fn zero_decimal_currency_drops_the_cents() {
        assert_eq!(
            tray_title(&sum(3_400_000, Some(1.0), false), &currency("JPY", 155.0)),
            "3.4M · ¥155"
        );
    }

    #[test]
    fn large_amounts_group_thousands_like_the_frontend() {
        assert_eq!(
            tray_title(&sum(3_400_000, Some(200.0), false), &currency("HKD", 7.8)),
            "3.4M · HK$1,560.00"
        );
        assert_eq!(
            tray_title(&sum(3_400_000, Some(12345.6), false), &Settings::default()),
            "3.4M · $12,345.60"
        );
    }

    #[test]
    fn unmapped_currency_falls_back_to_amount_code() {
        assert_eq!(
            tray_title(&sum(3_400_000, Some(2.0), false), &currency("SEK", 10.5)),
            "3.4M · 21.00 SEK"
        );
    }

    #[test]
    fn all_unpriced_day_shows_tokens_alone_never_zero_dollars() {
        assert_eq!(
            tray_title(&sum(964_200, None, true), &Settings::default()),
            "964.2K"
        );
    }

    // Mirrors format.ts formatCompactTokenTotal: 0.999995 rollover, ≤2
    // decimals trimmed, plain digits under 1K.
    #[test]
    fn token_totals_use_the_frontend_compact_form() {
        let t = |n| tray_title(&sum(n, None, false), &Settings::default());
        assert_eq!(t(999_999), "1M");
        assert_eq!(t(847), "847");
        assert_eq!(t(1_912_345_678), "1.91B");
    }

    // --- Menu row (Linux) ---

    // The same title the bar carries, named — a row in a menu cannot rely on
    // sitting next to the icon to say what it is counting.
    #[test]
    fn the_menu_row_names_the_figures_the_bar_shows_bare() {
        assert_eq!(today_row("3.4M · $12.84"), "Today: 3.4M · $12.84");
        assert_eq!(today_row("964.2K"), "Today: 964.2K");
        assert_eq!(today_row("0 · $0.00"), "Today: 0 · $0.00");
    }

    // --- Panel placement ---
    // Physical pixels throughout. macOS numbers are a 3024×1964 retina screen
    // (scale 2, 48px menu bar, 600×960 panel); the Windows ones a 1920×1080
    // screen (scale 1, 48px taskbar, 300×480 panel).

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
