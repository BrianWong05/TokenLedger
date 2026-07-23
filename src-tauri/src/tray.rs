// The Menu Bar Extra (CONTEXT.md): TokenLedger's resident menu-bar presence
// per ADR-0005 — the app lives here, not in a window you keep open. The tray
// shows a live bar title (Today's tokens + Cost, computed here) and toggles
// the traypanel webview window, which renders design 2b pixel-faithfully per
// ADR-0007 (superseding ADR-0006's native menu). Panel content and actions
// live in src/traypanel/; this file is the title math plus window glue.
use tauri::image::Image;
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Wry};

// Held so each scan can rewrite the bar title in place.
pub struct Tray {
    tray: TrayIcon<Wry>,
}

/// Builds the tray once, from setup: template glyph, live title, and a click
/// handler that toggles the panel.
pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let mut builder = TrayIconBuilder::new()
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
    // Design 2b's chart-line glyph as a macOS template image (black + alpha;
    // rasterized from the mock's mark into icons/tray.png). Not the app icon:
    // that is still the stock Tauri logo, which reads as mush when templated.
    if let Ok(icon) = Image::from_bytes(include_bytes!("../icons/tray.png")) {
        builder = builder.icon(icon).icon_as_template(true);
    }
    let tray = builder.build(app)?;
    app.manage(Tray { tray });
    refresh(app);
    Ok(())
}

/// Show the panel under the tray icon (right edges aligned), or hide it if
/// it's already up. The panel refetches on every show via the panel-shown
/// event; hide-on-blur lives in lib.rs's window-event handler.
fn toggle_panel(app: &AppHandle, rect: tauri::Rect) {
    let Some(w) = app.get_webview_window("traypanel") else {
        return;
    };
    if w.is_visible().unwrap_or(false) {
        let _ = w.hide();
        return;
    }
    let scale = w.scale_factor().unwrap_or(2.0);
    let pos = match rect.position {
        tauri::Position::Physical(p) => (f64::from(p.x), f64::from(p.y)),
        tauri::Position::Logical(l) => (l.x * scale, l.y * scale),
    };
    let size = match rect.size {
        tauri::Size::Physical(s) => (f64::from(s.width), f64::from(s.height)),
        tauri::Size::Logical(l) => (l.width * scale, l.height * scale),
    };
    let panel_w = w.outer_size().map(|s| f64::from(s.width)).unwrap_or(300.0 * scale);
    let x = pos.0 + size.0 - panel_w;
    let y = pos.1 + size.1 + 4.0 * scale;
    let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
    let _ = w.show();
    let _ = w.set_focus();
    let _ = app.emit_to("traypanel", "panel-shown", ());
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

/// Recomputes the bar title (Today's Summary + Settings → tray_title) and
/// rewrites it in place. Called after every scan and on settings save; no-op
/// until the tray exists. The db lock is released before set_title: it hops
/// to the main thread, and sync commands on the main thread take the same
/// lock — holding it here would deadlock.
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
    let _ = tray.tray.set_title(title.as_deref());
}

// --- Menu Bar Extra title (design 2b) ---
// Pure: constructed inputs in, strings out. The Tauri glue above stays thin.

/// The bar title for Today's Summary: "3.4M · $12.84". `None` on a no-usage
/// day — the icon stands alone rather than advertising `0 · $0.00`. Cost
/// follows the glossary: "≥ " marker when Partial (priced total over a set
/// with Unpriced Models or Unattributed Usage), and a day with no available
/// Cost shows tokens alone — never $0. ponytail: the bar drops the missing-Cost
/// wording for space; the menu's
/// per-Source rows (#24) spell it out.
fn tray_title(today: &crate::queries::Summary, settings: &crate::settings::Settings) -> Option<String> {
    if today.total_tokens == 0 {
        return None;
    }
    let toks = fmt_tokens(today.total_tokens);
    Some(match today.cost {
        None => toks,
        Some(c) => {
            let marker = if today.has_unpriced || today.unattributed_tokens > 0 { "≥ " } else { "" };
            format!("{toks} · {marker}{cost}", cost = fmt_cost(c, settings))
        }
    })
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
        }
    }

    #[test]
    fn no_usage_day_has_no_title() {
        assert_eq!(tray_title(&sum(0, None, false), &Settings::default()), None);
    }

    #[test]
    fn plain_day_shows_tokens_and_cost() {
        assert_eq!(
            tray_title(&sum(3_400_000, Some(12.84), false), &Settings::default()),
            Some("3.4M · $12.84".to_string())
        );
    }

    #[test]
    fn partial_cost_carries_the_marker() {
        assert_eq!(
            tray_title(&sum(3_400_000, Some(12.8), true), &Settings::default()),
            Some("3.4M · ≥ $12.80".to_string())
        );
    }

    #[test]
    fn unattributed_usage_marks_priced_cost_partial() {
        let mut today = sum(3_400_000, Some(12.8), false);
        today.unattributed_tokens = 400;
        assert_eq!(
            tray_title(&today, &Settings::default()),
            Some("3.4M · ≥ $12.80".to_string())
        );
    }

    #[test]
    fn all_unattributed_day_shows_tokens_alone() {
        let mut today = sum(964_200, None, false);
        today.unattributed_tokens = 964_200;
        assert_eq!(
            tray_title(&today, &Settings::default()),
            Some("964.2K".to_string())
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
            Some("3.4M · HK$78.00".to_string())
        );
    }

    #[test]
    fn zero_decimal_currency_drops_the_cents() {
        assert_eq!(
            tray_title(&sum(3_400_000, Some(1.0), false), &currency("JPY", 155.0)),
            Some("3.4M · ¥155".to_string())
        );
    }

    #[test]
    fn large_amounts_group_thousands_like_the_frontend() {
        assert_eq!(
            tray_title(&sum(3_400_000, Some(200.0), false), &currency("HKD", 7.8)),
            Some("3.4M · HK$1,560.00".to_string())
        );
        assert_eq!(
            tray_title(&sum(3_400_000, Some(12345.6), false), &Settings::default()),
            Some("3.4M · $12,345.60".to_string())
        );
    }

    #[test]
    fn unmapped_currency_falls_back_to_amount_code() {
        assert_eq!(
            tray_title(&sum(3_400_000, Some(2.0), false), &currency("SEK", 10.5)),
            Some("3.4M · 21.00 SEK".to_string())
        );
    }

    #[test]
    fn all_unpriced_day_shows_tokens_alone_never_zero_dollars() {
        assert_eq!(
            tray_title(&sum(964_200, None, true), &Settings::default()),
            Some("964.2K".to_string())
        );
    }

    // Mirrors format.ts formatCompactTokenTotal: 0.999995 rollover, ≤2
    // decimals trimmed, plain digits under 1K.
    #[test]
    fn token_totals_use_the_frontend_compact_form() {
        let t = |n| tray_title(&sum(n, None, false), &Settings::default()).unwrap();
        assert_eq!(t(999_999), "1M");
        assert_eq!(t(847), "847");
        assert_eq!(t(1_912_345_678), "1.91B");
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
