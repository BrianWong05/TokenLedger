// The Menu Bar Extra (CONTEXT.md): TokenLedger's resident menu-bar presence
// per ADR-0005 — the app lives here, not in a window you keep open. Renders
// design 2b ("Menu Bar - Options") as a native menu per ADR-0006: bar title
// with Today's tokens + Cost, then the menu build_menu lays out (see its doc
// for the row order). Accelerators are menu-local hints, not global hotkeys.
// "Rescan now" reuses the exact scan the `scan` command runs
// (crate::scan_now), so there is no second scan code path.
use std::sync::Mutex;

use tauri::image::Image;
use tauri::menu::{IconMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Emitter, Manager, Wry};

use crate::scan_now;

// Held so each scan can rewrite the bar title, header rows, and per-Source
// rows in place without rebuilding the whole tray (an open menu must not be
// disturbed); the menu is rebuilt only when Source membership changes.
pub struct TrayMenu {
    tray: TrayIcon<Wry>,
    handles: Mutex<Handles>,
}

// The mutable menu items refresh() re-texts, plus the (icon key, item) pair
// per Today-used Source — the key list is the membership identity.
struct Handles {
    hdr_cost: MenuItem<Wry>,
    hdr_usage: MenuItem<Wry>,
    sources: Vec<(String, IconMenuItem<Wry>)>,
}

/// Builds the whole 2b menu for a given header + per-Source row set:
/// header rows / sep / source rows / sep / Open, Rescan / sep / Settings,
/// Quit. Stats rows are enabled-but-inert (no on_menu_event arm): disabled
/// grey proved unreadable at sign-off, so full-brightness text won over 2b's
/// "no fake hover" — clicking a stat row just closes the menu. The trailing
/// source separator is skipped when there are no rows, so an empty day never
/// shows a double rule.
fn build_menu(
    app: &AppHandle,
    header: &(String, String),
    rows: &[(String, String)],
) -> tauri::Result<(Menu<Wry>, Handles)> {
    let hdr_cost = MenuItem::with_id(app, "hdr_cost", &header.0, true, None::<&str>)?;
    let hdr_usage = MenuItem::with_id(app, "hdr_usage", &header.1, true, None::<&str>)?;
    let menu = Menu::new(app)?;
    menu.append(&hdr_cost)?;
    menu.append(&hdr_usage)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    let mut sources = Vec::with_capacity(rows.len());
    for (i, (key, text)) in rows.iter().enumerate() {
        let item =
            IconMenuItem::with_id(app, format!("src_{i}"), text, true, source_icon(key), None::<&str>)?;
        menu.append(&item)?;
        sources.push((key.clone(), item));
    }
    if !rows.is_empty() {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }
    menu.append(&MenuItem::with_id(app, "open", "Open TokenLedger", true, None::<&str>)?)?;
    menu.append(&MenuItem::with_id(app, "scan", "Rescan now", true, Some("Shift+CmdOrCtrl+R"))?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "settings", "Settings…", true, Some("CmdOrCtrl+,"))?)?;
    menu.append(&MenuItem::with_id(app, "quit", "Quit TokenLedger", true, Some("CmdOrCtrl+Q"))?)?;
    Ok((menu, Handles { hdr_cost, hdr_usage, sources }))
}

/// The committed 18pt-rendered PNGs (rasterized from src/overview/icons SVGs,
/// full color). ponytail: decoded per rebuild — six tiny files, rare rebuilds.
fn source_icon(key: &str) -> Option<Image<'static>> {
    let bytes: &[u8] = match key {
        "claude" => include_bytes!("../icons/sources/claude.png"),
        "codex" => include_bytes!("../icons/sources/codex.png"),
        "gemini" => include_bytes!("../icons/sources/gemini.png"),
        "hermes" => include_bytes!("../icons/sources/hermes.png"),
        "grok" => include_bytes!("../icons/sources/grok.png"),
        "antigravity" => include_bytes!("../icons/sources/antigravity.png"),
        _ => return None,
    };
    Image::from_bytes(bytes).ok()
}

/// Builds the tray once, from setup. Uses the app's own icon as a macOS template
/// image so it inverts with the menu bar.
pub fn build(app: &AppHandle) -> tauri::Result<()> {
    // Placeholder header + no source rows; the refresh() below seeds both
    // from the existing Ledger (rebuilding the menu if today has usage).
    let placeholder = ("Today".to_string(), "…".to_string());
    let (menu, handles) = build_menu(app, &placeholder, &[])?;

    let mut builder = TrayIconBuilder::new()
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(on_menu_event);
    // Design 2b's chart-line glyph as a macOS template image (black + alpha;
    // rasterized from the mock's mark into icons/tray.png). Not the app icon:
    // that is still the stock Tauri logo, which reads as mush when templated.
    if let Ok(icon) = Image::from_bytes(include_bytes!("../icons/tray.png")) {
        builder = builder.icon(icon).icon_as_template(true);
    }
    let tray = builder.build(app)?;
    app.manage(TrayMenu {
        tray,
        handles: Mutex::new(handles),
    });
    refresh(app);
    Ok(())
}

/// Recomputes the bar title, Today-header rows, and per-Source rows and
/// applies them: set_text in place when Source membership is unchanged (an
/// open menu is never disturbed), a full build_menu + set_menu when it
/// changed (rare: first usage of a tool today, midnight rollover). Called
/// after every scan and on settings save; no-op until the tray exists.
/// The db lock is released before any menu call: those hop to the main
/// thread, and sync commands on the main thread take the same lock — holding
/// it here would deadlock.
pub fn refresh(app: &AppHandle) {
    let Some(tray) = app.try_state::<TrayMenu>() else {
        return;
    };
    let state = app.state::<crate::AppState>();
    let (title, header, rows) = {
        let Ok(db) = state.db.lock() else { return };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let Ok((start, end)) = day_window(&db, now) else {
            return;
        };
        // Same time yesterday = now − 24h. Its window is clamped at that
        // instant so the pace comparison is so-far vs so-far. ponytail: a DST
        // change skews the comparison an hour twice a year; civil-time
        // arithmetic is the upgrade path if that hour ever matters.
        let y_now = now - 86_400;
        let Ok((y_start, _)) = day_window(&db, y_now) else {
            return;
        };
        let today_f = crate::queries::Filters {
            start_ts: Some(start),
            end_ts: Some(end),
            ..Default::default()
        };
        let y_f = crate::queries::Filters {
            start_ts: Some(y_start),
            end_ts: Some(y_now),
            ..Default::default()
        };
        let (Ok(today), Ok(yesterday), Ok(tool_rows), Ok(settings)) = (
            crate::queries::summary(&db, &today_f),
            crate::queries::summary(&db, &y_f),
            crate::queries::breakdown(&db, "tool", &today_f),
            crate::settings::get_settings(&db),
        ) else {
            return;
        };
        (
            tray_title(&today, &settings),
            header_lines(&today, &yesterday, &settings),
            source_rows(&tool_rows, &settings),
        )
    };
    let _ = tray.tray.set_title(title.as_deref());
    // try_lock, never lock: the main thread must not block here, or it could
    // deadlock against a background refresh mid-hop to the main thread for a
    // menu call. A lost contended refresh is fine — the next scan tick
    // (≤30s) redoes it.
    let Ok(mut h) = tray.handles.try_lock() else {
        return;
    };
    let same_membership = h.sources.len() == rows.len()
        && h.sources.iter().zip(&rows).all(|((k, _), (rk, _))| k == rk);
    if same_membership {
        let _ = h.hdr_cost.set_text(&header.0);
        let _ = h.hdr_usage.set_text(&header.1);
        for ((_, item), (_, text)) in h.sources.iter().zip(&rows) {
            let _ = item.set_text(text);
        }
    } else if let Ok((menu, handles)) = build_menu(app, &header, &rows) {
        if tray.tray.set_menu(Some(menu)).is_ok() {
            *h = handles;
        }
    }
}

fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        "open" => show_main(app),
        "settings" => {
            // Show first, then ask the shell to land on the Settings tab; the
            // frontend's onOpenSettings listener does the switch.
            show_main(app);
            let _ = app.emit("open-settings", ());
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

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

// --- Menu Bar Extra title (design 2b) ---
// Pure: constructed inputs in, strings out. The Tauri glue above stays thin.

/// The bar title for Today's Summary: "3.4M · $12.84". `None` on a no-usage
/// day — the icon stands alone rather than advertising `0 · $0.00`. Cost
/// follows the glossary: "≥ " marker when Partial (priced total over a set
/// with Unpriced Models), and an all-Unpriced day shows tokens alone — never
/// $0. ponytail: the bar drops the word "unpriced" for space; the menu's
/// per-Source rows (#24) spell it out.
fn tray_title(today: &crate::queries::Summary, settings: &crate::settings::Settings) -> Option<String> {
    if today.total_tokens == 0 {
        return None;
    }
    let toks = fmt_tokens(today.total_tokens);
    Some(match today.cost {
        None => toks,
        Some(c) => {
            let marker = if today.has_unpriced { "≥ " } else { "" };
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

/// The menu's two inert Today-header rows (design 2b's header, flattened
/// to native text): a cost line and a usage line. Both rows always exist so
/// the menu structure never changes under an open menu — only set_text runs.
///
/// Cost line: "Today: $12.84 · +12.4% vs yesterday". The pace delta compares
/// today-so-far Cost against yesterday up to the same time and is folded into
/// the text; it disappears when either side has no Cost (yesterday empty or
/// Unpriced — no divide-by-zero artifact). "≥ " marks a Partial Cost; an
/// all-Unpriced day reads "Today: unpriced" — never $0; a no-usage day reads
/// "Today: no usage yet". ponytail: the glossary's Partial-Cost count of
/// Unpriced Models is dropped like the title drops it — the per-Source rows
/// (#24) surface which tools are unpriced.
/// Usage line: "3.4M tok · 1,912 req" — Requests is the glossary's sum of
/// calls, straight from Summary.
fn header_lines(
    today: &crate::queries::Summary,
    yesterday_so_far: &crate::queries::Summary,
    settings: &crate::settings::Settings,
) -> (String, String) {
    let usage = format!(
        "{} tok · {} req",
        fmt_tokens(today.total_tokens),
        fmt_amount(today.requests.max(0) as f64, 0)
    );
    let cost_line = if today.total_tokens == 0 {
        "Today: no usage yet".to_string()
    } else {
        match today.cost {
            None => "Today: unpriced".to_string(),
            Some(c) => {
                let marker = if today.has_unpriced { "≥ " } else { "" };
                let delta = match yesterday_so_far.cost {
                    Some(y) if y > 0.0 => {
                        format!(" · {:+.1}% vs yesterday", (c / y - 1.0) * 100.0)
                    }
                    _ => String::new(),
                };
                format!("Today: {marker}{}{delta}", fmt_cost(c, settings))
            }
        }
    };
    (cost_line, usage)
}

/// Display label for a DB source key (breakdown's tool key, e.g. "claude" —
/// the adapters write short keys, never display names). Labels are the
/// frontend's (meta.ts TOOLS); the key itself doubles as the icon filename.
/// Unknown keys fall through to None — shown by raw key, never dropped.
fn source_label(key: &str) -> Option<&'static str> {
    match key {
        "claude" => Some("Claude"),
        "codex" => Some("Codex"),
        "gemini" => Some("Gemini"),
        "hermes" => Some("Hermes"),
        "grok" => Some("Grok"),
        "antigravity" => Some("Antigravity"),
        _ => None,
    }
}

/// The menu's per-Source rows for Today, as (source key, row text): one row
/// per Source with usage — "Claude — 1.8M · $6.12" — Cost descending, Sources
/// whose Models are all Unpriced last (by tokens) reading "unpriced", never
/// $0.00; a mixed row's priced sum is a Partial Cost and carries "≥ ". The
/// key list is also the membership identity the glue diffs to decide re-text
/// vs rebuild.
fn source_rows(
    rows: &[crate::queries::BreakdownRow],
    settings: &crate::settings::Settings,
) -> Vec<(String, String)> {
    let mut used: Vec<&crate::queries::BreakdownRow> =
        rows.iter().filter(|r| r.total_tokens > 0).collect();
    // Cost desc; None (all-Unpriced) sorts after every Some, then tokens desc.
    used.sort_by(|a, b| match (a.cost, b.cost) {
        (Some(x), Some(y)) => y.total_cmp(&x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => b.total_tokens.cmp(&a.total_tokens),
    });
    used.iter()
        .map(|r| {
            let cost = match r.cost {
                Some(c) => {
                    let marker = if r.has_unpriced { "≥ " } else { "" };
                    format!("{marker}{}", fmt_cost(c, settings))
                }
                None => "unpriced".to_string(),
            };
            let text = format!(
                "{} — {} · {cost}",
                source_label(&r.key).unwrap_or(&r.key),
                fmt_tokens(r.total_tokens)
            );
            (r.key.clone(), text)
        })
        .collect()
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

    #[test]
    fn header_shows_cost_then_tokens_and_requests() {
        let mut today = sum(3_400_000, Some(12.84), false);
        today.requests = 1912;
        let lines = header_lines(&today, &sum(0, None, false), &Settings::default());
        assert_eq!(lines.0, "Today: $12.84"); // yesterday empty → no delta
        assert_eq!(lines.1, "3.4M tok · 1,912 req");
    }

    #[test]
    fn delta_compares_against_yesterday_up_to_now() {
        let lines = header_lines(
            &sum(3_400_000, Some(12.84), false),
            &sum(1_000_000, Some(10.0), false),
            &Settings::default(),
        );
        // 12.84 / 10.00 → +28.4%, one decimal like the design's +12.4%.
        assert_eq!(lines.0, "Today: $12.84 · +28.4% vs yesterday");
    }

    #[test]
    fn falling_pace_reads_negative() {
        let lines = header_lines(
            &sum(3_400_000, Some(9.0), false),
            &sum(1_000_000, Some(10.0), false),
            &Settings::default(),
        );
        assert_eq!(lines.0, "Today: $9.00 · -10.0% vs yesterday");
    }

    #[test]
    fn delta_hidden_when_yesterday_had_no_cost_by_now() {
        // Some(0.0) and None both suppress — no divide-by-zero artifact.
        let today = sum(3_400_000, Some(12.84), false);
        let zero = header_lines(&today, &sum(0, Some(0.0), false), &Settings::default());
        assert_eq!(zero.0, "Today: $12.84");
        let unpriced = header_lines(&today, &sum(500, None, true), &Settings::default());
        assert_eq!(unpriced.0, "Today: $12.84");
    }

    #[test]
    fn header_partial_cost_carries_the_marker() {
        let lines = header_lines(
            &sum(3_400_000, Some(12.84), true),
            &sum(0, None, false),
            &Settings::default(),
        );
        assert_eq!(lines.0, "Today: ≥ $12.84");
    }

    #[test]
    fn header_all_unpriced_day_says_unpriced_never_zero() {
        let mut today = sum(964_200, None, true);
        today.requests = 41;
        let lines = header_lines(&today, &sum(0, None, false), &Settings::default());
        assert_eq!(lines.0, "Today: unpriced");
        assert_eq!(lines.1, "964.2K tok · 41 req");
    }

    #[test]
    fn header_empty_day_says_no_usage_yet() {
        let lines = header_lines(
            &sum(0, Some(0.0), false),
            &sum(0, None, false),
            &Settings::default(),
        );
        assert_eq!(lines.0, "Today: no usage yet");
        assert_eq!(lines.1, "0 tok · 0 req");
    }

    fn brow(source: &str, total_tokens: i64, cost: Option<f64>) -> crate::queries::BreakdownRow {
        crate::queries::BreakdownRow {
            key: source.to_string(),
            source: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            total_tokens,
            requests: 0,
            cost,
            reasoning_tokens: None,
            convs: 0,
            cache_estimated: false,
            has_unpriced: false,
        }
    }

    // Keys are the DB source values (short keys, e.g. "claude" — what
    // breakdown(by "tool") actually returns), never display names.
    #[test]
    fn source_rows_drop_zero_usage_and_sort_by_cost_then_unpriced_by_tokens() {
        let rows = source_rows(
            &[
                brow("codex", 238_100, Some(1.11)),
                brow("gemini", 0, None), // no usage today → absent
                brow("grok", 964_200, None), // Unpriced → last
                brow("hermes", 500_000, Some(2.0)),
                brow("claude", 1_800_000, Some(6.12)),
            ],
            &Settings::default(),
        );
        let texts: Vec<&str> = rows.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "Claude — 1.8M · $6.12",
                "Hermes — 500K · $2.00",
                "Codex — 238.1K · $1.11",
                "Grok — 964.2K · unpriced",
            ]
        );
        let keys: Vec<&str> = rows.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["claude", "hermes", "codex", "grok"]);
    }

    #[test]
    fn source_rows_order_multiple_unpriced_by_tokens() {
        let rows = source_rows(
            &[brow("grok", 100, None), brow("hermes", 900, None)],
            &Settings::default(),
        );
        let texts: Vec<&str> = rows.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(texts, vec!["Hermes — 900 · unpriced", "Grok — 100 · unpriced"]);
    }

    #[test]
    fn source_rows_keep_unknown_sources_by_raw_key_without_icon() {
        let rows = source_rows(&[brow("weirdtool", 1_000, Some(1.0))], &Settings::default());
        assert_eq!(
            rows,
            vec![("weirdtool".to_string(), "weirdtool — 1K · $1.00".to_string())]
        );
    }

    #[test]
    fn source_row_partial_cost_carries_the_marker() {
        // A Source mixing priced and Unpriced Models: breakdown returns the
        // priced-only sum — the row must not read as a complete total.
        let mut row = brow("claude", 1_800_000, Some(6.12));
        row.has_unpriced = true;
        let rows = source_rows(&[row], &Settings::default());
        assert_eq!(rows[0].1, "Claude — 1.8M · ≥ $6.12");
    }

    // Asset guard: the bundled PNGs must decode through the same strict
    // decoder the app uses (tauri's png feature) — sips/CoreImage accepting
    // a file proves nothing, and a corrupt asset fails silently at runtime
    // (icon-less tray, icon-less rows).
    #[test]
    fn bundled_tray_icons_decode() {
        for key in ["claude", "codex", "gemini", "hermes", "grok", "antigravity"] {
            assert!(source_icon(key).is_some(), "menu icon for {key} must decode");
        }
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
