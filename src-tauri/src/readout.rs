//! How a Summary is read out to a person — the compact token total, the Cost
//! in the Display Currency, the ADR-0017 token floor, and the Partial-Cost
//! marker — one home, consumed by the Menu Bar Extra's bar title, the Linux
//! menu row, and the CSV report. The frontend renders the same figures with
//! Intl (src/lib/format.ts, src/lib/currency.ts), which ADR-0010's native
//! bar title cannot reach, so the two renderings are pinned to each other by
//! src/readout-cases.json: this module's tests and
//! src/lib/readoutCases.test.ts run the same table — rounding ties included,
//! one section per rule — and a divergence fails a build instead of shipping
//! two figures pixels apart in one dropdown.

/// The bar title for Today's Summary: "3.4M · $12.84", and "0 · $0.00" on a
/// day with no usage — a day that recorded nothing has a Cost of zero, not a
/// missing one (queries::summary), so it needs no case of its own here. Cost
/// follows the glossary: "≥ " marker when Partial (priced total over a set
/// with Unpriced Models or Unattributed Usage), and a day with usage but no
/// available Cost shows tokens alone — never $0. The token figure carries its
/// own "≥ " floor marker when an Unreadable Artifact could hold usage in the
/// window (ADR-0017) — the same rules as everywhere else, per the glossary's
/// Menu Bar Extra entry. ponytail: the bar drops the missing-Cost wording for
/// space; the menu's per-Source rows (#24) spell it out.
pub(crate) fn bar_title(
    today: &crate::queries::Summary,
    settings: &crate::settings::Settings,
    tokens_floor: bool,
) -> String {
    let floor_mark = if tokens_floor { "≥ " } else { "" };
    let toks = format!("{floor_mark}{}", fmt_tokens(today.total_tokens));
    match today.cost {
        None => toks,
        Some(c) => {
            let partial = is_partial_cost(today.cost, today.has_unpriced, today.unattributed_tokens);
            let marker = if partial { "≥ " } else { "" };
            format!("{toks} · {marker}{cost}", cost = fmt_cost(c, settings))
        }
    }
}

/// A Cost is Partial when it is a sum over only the priced tokens — a figure
/// exists, but Unpriced Models or Unattributed Usage kept tokens out of it
/// (glossary: Partial Cost). Mirror of isPartialCost in
/// src/lib/costCompleteness.ts, pinned by readout-cases.json's partialCosts
/// rows.
pub(crate) fn is_partial_cost(cost: Option<f64>, has_unpriced: bool, unattributed_tokens: i64) -> bool {
    cost.is_some() && (has_unpriced || unattributed_tokens > 0)
}

/// A token figure is a floor when any Source holds an Unreadable Artifact
/// whose content could fall in the window. Content is never newer than its
/// file, so `mtime >= window start` is the test — nothing bounds the content's
/// age downward (Antigravity's migration rewrote old Sessions as new files) —
/// and an unknown mtime marks conservatively. Mirror of unreadableSourcesIn
/// in src/lib/tokenCompleteness.ts, pinned by readout-cases.json's floors
/// rows.
pub(crate) fn tokens_are_floor(
    unreadable: &[crate::types::SourceUnreadable],
    window_start: i64,
) -> bool {
    unreadable.iter().any(|u| {
        u.artifacts_unreadable > 0
            && u.unreadable_max_mtime.is_none_or(|m| m >= window_start)
    })
}

/// A figure is also a floor when a Source holds Unbooked Requests the window
/// could contain (TOKL-25) — Requests it read and understood but booked no
/// Usage Record for, because the Source reported no tokens for them. Both the
/// Requests figure and every token total are bounded by it: the Requests
/// happened, and so did the tokens they spent, and neither is in the Ledger.
///
/// Unlike an Unreadable Artifact this has a real span, not just a file mtime —
/// these Requests carry their own timestamps — so a window strictly outside it
/// is left exact rather than marked. A null bound is a row written before the
/// span was recorded (schema v21) and marks conservatively, and a window with
/// no end is unbounded above. Mirror of unbookedSourcesIn in
/// src/lib/tokenCompleteness.ts, pinned by readout-cases.json's floors rows.
pub(crate) fn unbooked_are_floor(
    unbooked: &[crate::types::SourceUnbooked],
    window_start: i64,
    window_end: Option<i64>,
) -> bool {
    unbooked.iter().any(|u| {
        u.requests > 0
            && u.last_at.is_none_or(|last| last >= window_start)
            && match (u.first_at, window_end) {
                (Some(first), Some(end)) => first <= end,
                _ => true,
            }
    })
}

/// The ≥ marker's whole condition, for any figure a completeness gap bounds:
/// an Unreadable Artifact the window could draw content from, or an Unbooked
/// Request the window could contain. One function so a surface cannot mark
/// tokens for one cause and forget the other.
pub(crate) fn figures_are_floor(
    unreadable: &[crate::types::SourceUnreadable],
    unbooked: &[crate::types::SourceUnbooked],
    window_start: i64,
    window_end: Option<i64>,
) -> bool {
    tokens_are_floor(unreadable, window_start) || unbooked_are_floor(unbooked, window_start, window_end)
}

/// The Linux menu's Today row: the bar title, prefixed — a menu row, unlike a
/// title welded to the icon, has to name what it is counting.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn today_row(title: &str) -> String {
    format!("Today: {title}")
}

/// Token total in the frontend's compact form (format.ts
/// formatCompactTokenTotal): K/M/B suffix, up to 2 decimals with trailing
/// zeros trimmed, the same 0.999995 rollover so 999,999 reads "1M" — and the
/// same rounding. Intl rounds the decimal value half-up (1,005,000 is
/// "1.01M"), so the arithmetic here is integer throughout: rounding the f64
/// quotient instead ({:.2}) lands a hair below the tie and prints "1M".
fn fmt_tokens(n: i64) -> String {
    const UNITS: [(i128, &str); 3] = [(1_000_000_000, "B"), (1_000_000, "M"), (1_000, "K")];
    let n = n.max(0) as i128;
    for (div, suffix) in UNITS {
        // The unit holds until the value would round to a full 1000 at two
        // decimals: n/div >= 0.999995, in integers so the boundary cannot
        // drift through float error.
        if n * 1_000_000 >= div * 999_995 {
            // Half-up to hundredths of the unit; non-negative, so half-up is
            // Intl's halfExpand.
            let h = (n * 100 + div / 2) / div;
            let int = group_thousands(&(h / 100).to_string());
            return match (h % 100 / 10, h % 10) {
                (0, 0) => format!("{int}{suffix}"),
                (tenths, 0) => format!("{int}.{tenths}{suffix}"),
                _ => format!("{int}.{:02}{suffix}", h % 100),
            };
        }
    }
    n.to_string()
}

/// A USD Cost rendered in the Display Currency — the display-time
/// multiplication of ADR-0002: USD passes through, anything else multiplies
/// by the user's fixed usd_rate; stored figures never leave USD. The symbol
/// map is CLDR's for the two app languages (en-US / zh-Hant-HK, which differ
/// only on USD and AUD); the fallback is Intl's own generic form for a code
/// CLDR gives no symbol — the code first, joined to the amount by a no-break
/// space — which is also exactly how CLDR renders SGD ("SGD 21.00"), so SGD
/// needs no arm of its own.
fn fmt_cost(usd: f64, s: &crate::settings::Settings) -> String {
    let amount = if s.currency == "USD" { usd } else { usd * s.usd_rate };
    let zh = s.language == "zh-Hant";
    let (sym, dec): (&str, usize) = match s.currency.as_str() {
        "USD" => (if zh { "US$" } else { "$" }, 2),
        "EUR" => ("€", 2),
        "GBP" => ("£", 2),
        "HKD" => ("HK$", 2),
        "TWD" => ("NT$", 2),
        "CNY" => ("CN¥", 2),
        "AUD" => (if zh { "AU$" } else { "A$" }, 2),
        "CAD" => ("CA$", 2),
        "JPY" => ("¥", 0),
        "KRW" => ("₩", 0),
        code => return format!("{code}\u{a0}{}", fmt_amount(amount, 2)),
    };
    format!("{sym}{}", fmt_amount(amount, dec))
}

/// Rounds to `dec` places and comma-groups the integer part, matching the
/// frontend's Intl output ("1,560.00"). Intl rounds the amount's shortest
/// decimal form half-up — $2.675 renders "$2.68" even though the f64 sits
/// just below the tie, where binary rounding ({:.2}) would print "$2.67" —
/// so the rounding here works on the digits `{}` prints (Rust's shortest
/// round-trip form, the same digits JS's Number-to-string hands ICU), never
/// on the binary value. Costs are non-negative by construction (list rates ×
/// token counts), so no sign handling.
fn fmt_amount(amount: f64, dec: usize) -> String {
    // Clamp like fmt_tokens's n.max(0): a negative amount cannot occur (list
    // rates × token counts), but a hand-edited usd_rate must not feed a '-'
    // into the digit walk below.
    let shortest = format!("{}", amount.max(0.0));
    let (int, frac) = shortest.split_once('.').unwrap_or((shortest.as_str(), ""));
    let mut digits: Vec<u8> = int.bytes().chain(frac.bytes()).map(|b| b - b'0').collect();
    let mut int_len = int.len();
    let keep = int_len + dec;
    if digits.len() > keep {
        let round_up = digits[keep] >= 5;
        digits.truncate(keep);
        if round_up {
            let mut i = keep;
            let mut carry = true;
            while carry && i > 0 {
                i -= 1;
                carry = digits[i] == 9;
                digits[i] = if carry { 0 } else { digits[i] + 1 };
            }
            if carry {
                digits.insert(0, 1);
                int_len += 1;
            }
        }
    } else {
        digits.resize(keep, 0);
    }
    let int: String = digits[..int_len].iter().map(|d| char::from(d + b'0')).collect();
    let grouped = group_thousands(&int);
    if dec == 0 {
        grouped
    } else {
        let frac: String = digits[int_len..].iter().map(|d| char::from(d + b'0')).collect();
        format!("{grouped}.{frac}")
    }
}

/// Comma-groups a digit string ("1560" → "1,560") — Intl's en-US grouping,
/// which zh-Hant-HK shares.
fn group_thousands(int: &str) -> String {
    let mut grouped = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    grouped
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

    fn currency(code: &str, rate: f64) -> Settings {
        Settings {
            currency: code.to_string(),
            usd_rate: rate,
            ..Settings::default()
        }
    }

    // --- The shared pin (src/readout-cases.json) ---
    // The other half runs in src/lib/readoutCases.test.ts against Intl. The
    // tie rows are the point: Intl rounds the value's shortest decimal form
    // half-up (1,005,000 → "1.01M", $2.675 → "$2.68"), which is exactly where
    // binary f64 rounding drifts.

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Cases {
        tokens: Vec<TokenCase>,
        costs: Vec<CostCase>,
        floors: Vec<FloorCase>,
        unbooked_floors: Vec<UnbookedFloorCase>,
        partial_costs: Vec<PartialCase>,
    }
    #[derive(serde::Deserialize)]
    struct TokenCase {
        n: i64,
        text: String,
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct CostCase {
        usd: f64,
        usd_rate: f64,
        currency: String,
        lang: String,
        text: String,
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FloorCase {
        artifacts_unreadable: u64,
        unreadable_max_mtime: Option<i64>,
        window_start: i64,
        floor: bool,
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct UnbookedFloorCase {
        requests: u64,
        first_at: Option<i64>,
        last_at: Option<i64>,
        window_start: i64,
        window_end: Option<i64>,
        floor: bool,
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct PartialCase {
        cost: Option<f64>,
        has_unpriced: bool,
        unattributed_tokens: i64,
        partial: bool,
    }

    fn cases() -> Cases {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../src/readout-cases.json"
        )))
        .expect("readout-cases.json parses")
    }

    #[test]
    fn token_totals_match_the_shared_readout_cases() {
        for c in cases().tokens {
            assert_eq!(fmt_tokens(c.n), c.text, "fmt_tokens({})", c.n);
        }
    }

    #[test]
    fn costs_match_the_shared_readout_cases() {
        for c in cases().costs {
            let settings = Settings {
                language: c.lang.clone(),
                currency: c.currency.clone(),
                usd_rate: c.usd_rate,
                ..Settings::default()
            };
            assert_eq!(
                fmt_cost(c.usd, &settings),
                c.text,
                "fmt_cost({} USD × {} as {} in {})",
                c.usd,
                c.usd_rate,
                c.currency,
                c.lang
            );
        }
    }

    #[test]
    fn floor_rule_matches_the_shared_readout_cases() {
        for c in cases().floors {
            let u = crate::types::SourceUnreadable {
                source: "antigravity".to_string(),
                artifacts_unreadable: c.artifacts_unreadable,
                unreadable_max_mtime: c.unreadable_max_mtime,
            };
            assert_eq!(
                tokens_are_floor(&[u], c.window_start),
                c.floor,
                "tokens_are_floor({} unreadable, mtime {:?}, start {})",
                c.artifacts_unreadable,
                c.unreadable_max_mtime,
                c.window_start
            );
        }
    }

    #[test]
    fn unbooked_floor_rule_matches_the_shared_readout_cases() {
        for c in cases().unbooked_floors {
            let u = crate::types::SourceUnbooked {
                source: "qoder".to_string(),
                requests: c.requests,
                first_at: c.first_at,
                last_at: c.last_at,
            };
            assert_eq!(
                unbooked_are_floor(&[u], c.window_start, c.window_end),
                c.floor,
                "unbooked_are_floor({} requests in [{:?}, {:?}], window [{}, {:?}])",
                c.requests,
                c.first_at,
                c.last_at,
                c.window_start,
                c.window_end
            );
        }
    }

    #[test]
    fn partial_cost_markers_match_the_shared_readout_cases() {
        for c in cases().partial_costs {
            assert_eq!(
                is_partial_cost(c.cost, c.has_unpriced, c.unattributed_tokens),
                c.partial,
                "is_partial_cost({:?}, {}, {})",
                c.cost,
                c.has_unpriced,
                c.unattributed_tokens
            );
        }
    }

    // --- Bar title composition ---

    // Zero is a figure the day has, so the bar reads it out — the icon never
    // stands alone wondering whether the app is still counting. The Some(0.0)
    // is what queries::summary reports for a window holding no usage; the
    // no-usage-costs-zero rule lives there, not here.
    #[test]
    fn no_usage_day_reads_zero_in_the_display_currency() {
        assert_eq!(
            bar_title(&sum(0, Some(0.0), false), &Settings::default(), false),
            "0 · $0.00"
        );
        assert_eq!(
            bar_title(&sum(0, Some(0.0), false), &currency("JPY", 155.0), false),
            "0 · ¥0"
        );
    }

    #[test]
    fn plain_day_shows_tokens_and_cost() {
        assert_eq!(
            bar_title(&sum(3_400_000, Some(12.84), false), &Settings::default(), false),
            "3.4M · $12.84"
        );
    }

    #[test]
    fn partial_cost_carries_the_marker() {
        assert_eq!(
            bar_title(&sum(3_400_000, Some(12.8), true), &Settings::default(), false),
            "3.4M · ≥ $12.80"
        );
    }

    // The token figure carries its own ≥ (ADR-0017): an Unreadable Artifact
    // makes the count a floor, independently of whether the Cost is Partial.
    #[test]
    fn unreadable_artifacts_mark_the_token_figure() {
        assert_eq!(
            bar_title(&sum(3_400_000, Some(12.84), false), &Settings::default(), true),
            "≥ 3.4M · $12.84"
        );
        assert_eq!(
            bar_title(&sum(3_400_000, Some(12.8), true), &Settings::default(), true),
            "≥ 3.4M · ≥ $12.80"
        );
    }

    fn unreadable(count: u64, max_mtime: Option<i64>) -> crate::types::SourceUnreadable {
        crate::types::SourceUnreadable {
            source: "antigravity".to_string(),
            artifacts_unreadable: count,
            unreadable_max_mtime: max_mtime,
        }
    }

    // mtime is the only honest bound: content is never newer than its file,
    // and nothing bounds it downward (Antigravity's migration rewrote old
    // Sessions as new files). So only a window starting after every
    // Unreadable Artifact's last write is definitely complete — and an
    // unknown mtime marks conservatively.
    #[test]
    fn tokens_are_floor_tests_mtime_against_window_start() {
        assert!(tokens_are_floor(&[unreadable(100, Some(1_000))], 1_000));
        assert!(tokens_are_floor(&[unreadable(100, Some(1_001))], 1_000));
        assert!(!tokens_are_floor(&[unreadable(100, Some(999))], 1_000));
        assert!(tokens_are_floor(&[unreadable(100, None)], 1_000));
        assert!(!tokens_are_floor(&[unreadable(0, None)], 0));
        assert!(!tokens_are_floor(&[], 0));
    }

    #[test]
    fn unattributed_usage_marks_priced_cost_partial() {
        let mut today = sum(3_400_000, Some(12.8), false);
        today.unattributed_tokens = 400;
        assert_eq!(
            bar_title(&today, &Settings::default(), false),
            "3.4M · ≥ $12.80"
        );
    }

    #[test]
    fn all_unattributed_day_shows_tokens_alone() {
        let mut today = sum(964_200, None, false);
        today.unattributed_tokens = 964_200;
        assert_eq!(
            bar_title(&today, &Settings::default(), false),
            "964.2K"
        );
    }

    #[test]
    fn display_currency_multiplies_and_uses_its_symbol() {
        assert_eq!(
            bar_title(&sum(3_400_000, Some(10.0), false), &currency("HKD", 7.8), false),
            "3.4M · HK$78.00"
        );
    }

    #[test]
    fn all_unpriced_day_shows_tokens_alone_never_zero_dollars() {
        assert_eq!(
            bar_title(&sum(964_200, None, true), &Settings::default(), false),
            "964.2K"
        );
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
}
