//! A report over a window of the Ledger, written to CSV for use outside the
//! app. It runs the production queries against the real Ledger rather than
//! reimplementing them, so every figure — Cost, its completeness, Unpriced and
//! Unattributed Usage, the Unreadable Artifact floor — is the one the Overview
//! would show for the same window. A hand-written SQL report cannot say that:
//! Cost needs `RateMap::resolve`'s Override → exact → normalized ladder, which
//! is not expressible in a query.
//!
//! Ignored by default: it reads the machine's real Ledger, like the other
//! local-data workflows in this crate. Nothing is written to the Ledger — the
//! connection is opened read-only.
//!
//! ```text
//! cargo test --manifest-path src-tauri/Cargo.toml report::tests::ledger_report -- --ignored --nocapture
//! ```
//!
//! | Variable | Default |
//! |---|---|
//! | `TOKENLEDGER_REPORT_DAYS` | `30` — trailing local days, today included |
//! | `TOKENLEDGER_REPORT_DB`   | the installed app's Ledger for this platform |
//! | `TOKENLEDGER_REPORT_OUT`  | `tokenledger-report-<from>_<to>/` in the repo root |

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{Duration, Local, LocalResult, NaiveDate, TimeZone};
use rusqlite::{Connection, OpenFlags};

use crate::queries::{BreakdownRow, Filters, Summary};
use crate::{db, queries, types::SourceUnreadable};

const DAYS_ENV: &str = "TOKENLEDGER_REPORT_DAYS";
const DB_ENV: &str = "TOKENLEDGER_REPORT_DB";
const OUT_ENV: &str = "TOKENLEDGER_REPORT_OUT";

/// Where the installed app keeps its Ledger, per README. Derived here rather
/// than asked of Tauri because a report is not an app run: there is no
/// AppHandle, and a mock one would resolve somewhere else entirely.
fn default_db() -> Option<PathBuf> {
    const APP_ID: &str = "com.brianwong.tokenledger";
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        home()?.join("Library").join("Application Support")
    } else if cfg!(target_os = "windows") {
        PathBuf::from(std::env::var_os("APPDATA")?)
    } else {
        match std::env::var_os("XDG_DATA_HOME") {
            Some(dir) => PathBuf::from(dir),
            None => home()?.join(".local").join("share"),
        }
    };
    Some(base.join(APP_ID).join("tokenledger.db"))
}

/// Local midnight as an epoch second — the same instant `rangeWindow` in
/// src/overview/data.ts sends as `startTs`, so a report window and the
/// Overview's own preset cover identical ground. A zone that skips midnight
/// outright starts the day at the first instant that exists.
fn local_midnight(date: NaiveDate) -> i64 {
    let naive = date.and_hms_opt(0, 0, 0).expect("midnight is a valid time");
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(dt) => dt.timestamp(),
        LocalResult::Ambiguous(dt, _) => dt.timestamp(),
        LocalResult::None => Local
            .from_local_datetime(&date.and_hms_opt(1, 0, 0).expect("01:00 is a valid time"))
            .earliest()
            .expect("the hour after a skipped midnight exists")
            .timestamp(),
    }
}

/// How much of a set of Records' Cost the Ledger could compute — carried as a
/// column of its own so a partial figure is never mistaken for a total and an
/// Unpriced Model never reads as `$0` (glossary: Partial Cost, Unpriced).
/// readout::is_partial_cost's rule, worded for a CSV column.
fn cost_basis(cost: Option<f64>, has_unpriced: bool, unattributed: i64) -> &'static str {
    if cost.is_none() {
        "unavailable"
    } else if crate::readout::is_partial_cost(cost, has_unpriced, unattributed) {
        "partial"
    } else {
        "exact"
    }
}

/// `Some(cost)` as a number, or empty — an unavailable Cost leaves the cell
/// blank rather than writing a zero a spreadsheet would happily sum.
fn cost_cell(cost: Option<f64>) -> String {
    cost.map_or(String::new(), |c| format!("{c:.6}"))
}

/// A figure is a floor when a Source holds an Unreadable Artifact whose content
/// could fall in the window, or Unbooked Requests the window could contain —
/// readout::figures_are_floor's rule, worded for a CSV column. Both the token
/// totals and the Requests figure read it: the same gap bounds both.
///
/// The report always runs to the present, so there is no window end to test.
fn tokens_basis(
    unreadable: &[SourceUnreadable],
    unbooked: &[crate::types::SourceUnbooked],
    window_start: i64,
) -> &'static str {
    if crate::readout::figures_are_floor(unreadable, unbooked, window_start, None) {
        "floor"
    } else {
        "exact"
    }
}

fn esc(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

fn write_csv(dir: &Path, name: &str, header: &str, rows: Vec<String>) -> std::io::Result<PathBuf> {
    let path = dir.join(name);
    let mut body = String::from(header);
    for row in rows {
        body.push('\n');
        body.push_str(&row);
    }
    body.push('\n');
    fs::write(&path, body)?;
    Ok(path)
}

/// A breakdown section. `keyed_by_source` is the Model breakdown's extra
/// column: only Model rows carry the Source that scoped them.
fn breakdown_rows(rows: &[BreakdownRow], keyed_by_source: bool) -> Vec<String> {
    rows.iter()
        .map(|r| {
            // None is the Model breakdown's Unattributed Usage row; every other
            // breakdown names its key (`breakdown` substitutes "unknown").
            let key = r.key.as_deref().unwrap_or("Unattributed usage");
            let mut cells = vec![esc(key)];
            if keyed_by_source {
                cells.push(esc(r.source.as_deref().unwrap_or("")));
            }
            cells.extend([
                r.input_tokens.to_string(),
                r.output_tokens.to_string(),
                r.cache_read_tokens.to_string(),
                r.cache_write_tokens.to_string(),
                r.total_tokens.to_string(),
                r.requests.to_string(),
                r.convs.to_string(),
                cost_cell(r.cost),
                cost_basis(r.cost, r.has_unpriced, r.unattributed_tokens).to_string(),
                r.unattributed_tokens.to_string(),
                r.cache_estimated.to_string(),
            ]);
            cells.join(",")
        })
        .collect()
}

const BREAKDOWN_COLS: &str =
    "input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,total_tokens,\
     requests,sessions,cost_usd,cost_basis,unattributed_tokens,cache_estimated";

fn summary_row(label: &str, s: &Summary) -> String {
    [
        esc(label),
        s.input_tokens.to_string(),
        s.output_tokens.to_string(),
        s.cache_read_tokens.to_string(),
        s.cache_write_tokens.to_string(),
        s.total_tokens.to_string(),
        s.requests.to_string(),
        s.convs.to_string(),
        format!("{:.6}", s.cache_hit_rate),
        cost_cell(s.cost),
        cost_basis(s.cost, s.has_unpriced, s.unattributed_tokens).to_string(),
        s.unattributed_tokens.to_string(),
        esc(&s.unpriced_models.join(" ")),
        esc(&s.cache_estimated_models.join(" ")),
    ]
    .join(",")
}

const SUMMARY_COLS: &str =
    "input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,total_tokens,\
     requests,sessions,cache_hit_rate,cost_usd,cost_basis,unattributed_tokens,\
     unpriced_models,cache_estimated_models";

/// Writes the whole report and returns the lines to read out on stdout.
fn run(conn: &Connection, days: i64, today: NaiveDate, out: &Path) -> std::io::Result<Vec<String>> {
    let from = today - Duration::days(days - 1);
    let start_ts = local_midnight(from);
    // The upper bound stays open, exactly as the Overview's presets leave it
    // (data.ts: "presets leave endTs open"). A Record timestamped ahead of the
    // clock therefore lands in the report rather than vanishing between the
    // window and the last day.
    let filters = Filters { start_ts: Some(start_ts), ..Default::default() };

    let summary = queries::summary(conn, &filters).expect("summary over the report window");
    let unreadable = db::load_unreadable(conn);
    let unbooked = db::load_unbooked(conn);
    fs::create_dir_all(out)?;

    let mut written = Vec::new();
    written.push(write_csv(
        out,
        "summary.csv",
        &format!("window,{SUMMARY_COLS},tokens_basis"),
        vec![format!(
            "{},{}",
            summary_row(&format!("{from} .. {today}"), &summary),
            tokens_basis(&unreadable, &unbooked, start_ts)
        )],
    )?);

    // One row per local day, each its own window so the day carries the same
    // figures the Overview would show if that day were the selection — Cost
    // included, which the trend's plain f64 cannot express when a whole day is
    // Unpriced. The last day's end stays open for the reason above.
    let mut day_rows = Vec::new();
    for offset in 0..days {
        let day = from + Duration::days(offset);
        let last = offset == days - 1;
        let day_filters = Filters {
            start_ts: Some(local_midnight(day)),
            end_ts: (!last).then(|| local_midnight(day + Duration::days(1))),
            ..Default::default()
        };
        let day_summary = queries::summary(conn, &day_filters).expect("summary over one day");
        day_rows.push(summary_row(&day.to_string(), &day_summary));
    }
    written.push(write_csv(out, "by-day.csv", &format!("day,{SUMMARY_COLS}"), day_rows)?);

    for (name, by, key) in [
        ("by-source.csv", "tool", "source"),
        ("by-model.csv", "model", "model"),
        ("by-project.csv", "project", "project"),
    ] {
        let rows = queries::breakdown(conn, by, &filters).expect("breakdown over the report window");
        let model_scoped = by == "model";
        let header = if model_scoped {
            format!("{key},source,{BREAKDOWN_COLS}")
        } else {
            format!("{key},{BREAKDOWN_COLS}")
        };
        written.push(write_csv(out, name, &header, breakdown_rows(&rows, model_scoped))?);
    }

    let mut lines = vec![
        format!("window            {from} .. {today}  ({days} local days)"),
        format!(
            "tokens            {}{}  (in {} · out {} · cache read {} · cache write {})",
            if tokens_basis(&unreadable, &unbooked, start_ts) == "floor" { "≥ " } else { "" },
            summary.total_tokens,
            summary.input_tokens,
            summary.output_tokens,
            summary.cache_read_tokens,
            summary.cache_write_tokens,
        ),
        match summary.cost {
            None => "est. cost         unavailable — every Model in the window is Unpriced".to_string(),
            Some(c) => format!(
                "est. cost         {}${c:.2}  at API list prices, not billed",
                if summary.has_unpriced || summary.unattributed_tokens > 0 { "≥ " } else { "" }
            ),
        },
        format!(
            "requests          {}{}   sessions {}   cache hit rate {:.1}%",
            // The same gap that floors the tokens floors this: an unreadable
            // Session hides the Requests in it, and an Unbooked Request is one
            // the Source made and the Ledger cannot count.
            if tokens_basis(&unreadable, &unbooked, start_ts) == "floor" { "≥ " } else { "" },
            summary.requests, summary.convs, summary.cache_hit_rate * 100.0
        ),
    ];
    if summary.unattributed_tokens > 0 {
        lines.push(format!(
            "unattributed      {} tokens carry no Model and so no Cost",
            summary.unattributed_tokens
        ));
    }
    if !summary.unpriced_models.is_empty() {
        lines.push(format!("unpriced Models   {}", summary.unpriced_models.join(", ")));
    }
    for u in &unreadable {
        lines.push(format!(
            "unreadable        {}: {} Artifact(s) the scan cannot read — token totals are a floor",
            u.source, u.artifacts_unreadable
        ));
    }
    for u in &unbooked {
        lines.push(format!(
            "unbooked          {}: {} Request(s) the Source reports no tokens for — no Usage Record exists for them",
            u.source, u.requests
        ));
    }
    lines.push(String::new());
    for path in written {
        lines.push(format!("wrote             {}", path.display()));
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_db;

    #[test]
    #[ignore = "reads this machine's real Ledger; run it deliberately"]
    fn ledger_report() {
        let days: i64 = std::env::var(DAYS_ENV)
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|d| *d > 0)
            .unwrap_or(30);
        let db_path = std::env::var_os(DB_ENV)
            .map(PathBuf::from)
            .or_else(default_db)
            .expect("no home directory to resolve the Ledger from — set TOKENLEDGER_REPORT_DB");
        assert!(
            db_path.exists(),
            "no Ledger at {} — run TokenLedger once, or set {DB_ENV}",
            db_path.display()
        );

        // Read-only, so a report can never migrate or write the Ledger the app
        // is holding open; WAL lets the two connections coexist.
        let conn = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open the Ledger read-only");
        db::register_query_functions(&conn).expect("register the local-bucket function");

        let today = Local::now().date_naive();
        let from = today - Duration::days(days - 1);
        // Anchored to the repository, not the working directory: cargo runs a
        // test binary from the package root, so a bare relative default would
        // put the report under src-tauri/ and read as though it were lost.
        let out = std::env::var_os(OUT_ENV).map(PathBuf::from).unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("the crate directory has a parent")
                .join(format!("tokenledger-report-{from}_{today}"))
        });

        let lines = run(&conn, days, today, &out).expect("write the report");
        println!("\n{}\n", lines.join("\n"));
    }

    /// A window with no usage costs zero — the one zero that is a figure rather
    /// than a gap — while Unpriced and Unattributed keep their own basis.
    #[test]
    fn cost_basis_separates_a_real_zero_from_a_gap() {
        assert_eq!(cost_basis(Some(0.0), false, 0), "exact");
        assert_eq!(cost_basis(Some(12.0), true, 0), "partial");
        assert_eq!(cost_basis(Some(12.0), false, 400), "partial");
        assert_eq!(cost_basis(None, true, 0), "unavailable");
        assert_eq!(cost_cell(None), "");
    }

    /// Same mtime-against-window-start rule as the tray and the frontend.
    #[test]
    fn tokens_basis_marks_a_window_an_unreadable_artifact_could_reach() {
        let u = |count, mtime| SourceUnreadable {
            source: "antigravity".to_string(),
            artifacts_unreadable: count,
            unreadable_max_mtime: mtime,
        };
        assert_eq!(tokens_basis(&[u(6, Some(1_000))], &[], 1_000), "floor");
        assert_eq!(tokens_basis(&[u(6, Some(999))], &[], 1_000), "exact");
        assert_eq!(tokens_basis(&[u(6, None)], &[], 1_000), "floor");
        assert_eq!(tokens_basis(&[u(0, None)], &[], 0), "exact");
        assert_eq!(tokens_basis(&[], &[], 0), "exact");

        // TOKL-25: Unbooked Requests floor the same figures, bounded by when
        // they actually happened rather than by a file mtime.
        let b = |requests, first, last| crate::types::SourceUnbooked {
            source: "qoder".into(),
            requests,
            first_at: first,
            last_at: last,
        };
        assert_eq!(tokens_basis(&[], &[b(628, Some(500), Some(1_500))], 1_000), "floor");
        // Every unbooked Request predates the window: nothing to admit.
        assert_eq!(tokens_basis(&[], &[b(628, Some(100), Some(999))], 1_000), "exact");
        assert_eq!(tokens_basis(&[], &[b(0, Some(100), Some(9_999))], 1_000), "exact");
        // A pre-v21 row knows no span and marks conservatively.
        assert_eq!(tokens_basis(&[], &[b(628, None, None)], 1_000), "floor");
    }

    #[test]
    fn csv_fields_quote_only_what_needs_it() {
        assert_eq!(esc("claude-opus-5"), "claude-opus-5");
        assert_eq!(esc("/Users/me/a,b"), "\"/Users/me/a,b\"");
        assert_eq!(esc("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    /// The report is a view of the Ledger like any other: over a window it
    /// holds no usage in, it reports zero rather than refusing.
    #[test]
    fn an_empty_ledger_reports_an_empty_window() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db(&dir.path().join("tokenledger.db")).unwrap();
        let out = dir.path().join("report");
        let lines = run(&conn, 30, NaiveDate::from_ymd_opt(2026, 8, 10).unwrap(), &out).unwrap();

        assert!(lines.iter().any(|l| l.contains("est. cost         $0.00")), "{lines:?}");
        let by_day = fs::read_to_string(out.join("by-day.csv")).unwrap();
        assert_eq!(by_day.lines().count(), 31, "a header and one row per day");
        assert!(by_day.lines().next().unwrap().starts_with("day,input_tokens"));
        assert!(out.join("by-model.csv").exists());
    }

    /// TOKL-25: the text report's Requests figure carries the ≥ too. The gap
    /// that floors the tokens floors this — an unreadable Session hides the
    /// Requests inside it, and an Unbooked Request is one the Source made that
    /// no Usage Record could count.
    #[test]
    fn unbooked_requests_floor_the_reported_requests_figure() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db(&dir.path().join("tokenledger.db")).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 8, 10).unwrap();
        let out = dir.path().join("report");

        // Clean Ledger first: neither figure is marked, so the assertions below
        // cannot pass on a report that marks everything.
        let clean = run(&conn, 30, today, &out).unwrap();
        assert!(
            clean.iter().any(|l| l.starts_with("requests          0 ")),
            "{clean:?}"
        );
        assert!(clean.iter().all(|l| !l.contains("unbooked  ")), "{clean:?}");

        // A window the Unbooked Requests fall inside.
        let at = crate::time::iso_to_epoch("2026-08-09T12:00:00Z").unwrap();
        crate::db::set_unbooked_requests(&conn, "/t/s.jsonl", "qoder", 628, Some(at), Some(at))
            .unwrap();
        let lines = run(&conn, 30, today, &out).unwrap();
        assert!(
            lines.iter().any(|l| l.starts_with("requests          ≥ ")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("tokens            ≥ ")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("unbooked") && l.contains("628 Request(s)")),
            "{lines:?}"
        );

        // A window that closes before the Requests happened is left exact: the
        // span is what an Unreadable Artifact cannot offer.
        let old_at = crate::time::iso_to_epoch("2020-01-01T00:00:00Z").unwrap();
        crate::db::set_unbooked_requests(
            &conn,
            "/t/s.jsonl",
            "qoder",
            628,
            Some(old_at),
            Some(old_at),
        )
        .unwrap();
        let exact = run(&conn, 30, today, &out).unwrap();
        assert!(
            exact.iter().any(|l| l.starts_with("requests          0 ")),
            "{exact:?}"
        );
    }
}
