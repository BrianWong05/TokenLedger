use std::time::{Duration, Instant};

use crate::db;
use crate::queries::{self, Filters};
use crate::types::UsageEvent;
use rusqlite::OpenFlags;

const EVENT_COUNT: usize = 100_000;
const DAY_SECONDS: i64 = 86_400;
// 2025-01-01T00:00:00Z — the fixed anchor every synthetic timestamp hangs off.
const BASE_EPOCH: i64 = 1_735_689_600;
const SERIES_BUDGET: Duration = Duration::from_millis(1_000);
// Tight enough to catch a per-query full-table scan sneaking back into the
// range-switch path (ctx_buckets' old whole-Ledger ROW_NUMBER ran this reload
// at ~150ms); the fixed path measures ~25ms here.
const RANGE_RELOAD_BUDGET: Duration = Duration::from_millis(100);

fn synthetic_event(i: usize) -> UsageEvent {
    // Spread writes deterministically across two years and insert them out of
    // timestamp order. This resembles an all-history backfill without relying
    // on private Source Artifacts or the machine's real Ledger.
    let second = ((i as i64 * 7_919) % (730 * DAY_SECONDS)) + BASE_EPOCH;
    let source = format!("source-{}", i % 16);
    UsageEvent {
        dedup_key: format!("perf-{i}"),
        source: source.clone(),
        timestamp: second,
        model: Some(format!("model-{}", i % 64)),
        project: Some(format!("/synthetic/project-{}", i % 256)),
        api_calls: 1,
        input_tokens: 1_000 + (i % 2_000) as i64,
        output_tokens: 200 + (i % 800) as i64,
        cache_read_tokens: (i % 500) as i64,
        cache_write_5m_tokens: (i % 100) as i64,
        cache_write_1h_tokens: (i % 25) as i64,
        source_file: format!("/synthetic/{source}/{}.jsonl", i % 2_000),
        session_id: Some(format!("session-{}", i % 4_096)),
        reasoning_tokens: Some((i % 120) as i64),
        ctx: Default::default(),
    }
}

#[test]
#[ignore = "performance standard: run in release mode via `npm run perf`"]
fn performance_standard_large_ledger() {
    assert!(
        !cfg!(debug_assertions),
        "performance standards must run with --release (`npm run perf`)"
    );

    let dir = tempfile::tempdir().unwrap();
    let mut conn = db::open_db(&dir.path().join("performance.db")).unwrap();
    let events: Vec<_> = (0..EVENT_COUNT).map(synthetic_event).collect();
    db::insert_events(&mut conn, &events).unwrap();
    drop(events);

    // This is the Overview's unbounded Activity/Profile read. It exercises the
    // exact production query and local-calendar bucketing used on first load.
    let started = Instant::now();
    let points = queries::series(&conn, &Filters::default(), "day").unwrap();
    let elapsed = started.elapsed();

    assert!(!points.is_empty());
    eprintln!(
        "PERF daily_series events={EVENT_COUNT} points={} elapsed_ms={:.1} budget_ms={}",
        points.len(),
        elapsed.as_secs_f64() * 1_000.0,
        SERIES_BUDGET.as_millis()
    );
    assert!(
        elapsed <= SERIES_BUDGET,
        "100k-record daily series took {:.1} ms; budget is {} ms",
        elapsed.as_secs_f64() * 1_000.0,
        SERIES_BUDGET.as_millis()
    );

    // The eight serialized reads in OverviewStore.runReload(). The Tauri layer
    // protects this same connection with one mutex, so serial execution here is
    // the user-visible backend latency even though the frontend uses Promise.all.
    let filters = Filters {
        start_ts: Some(BASE_EPOCH + 700 * DAY_SECONDS),
        end_ts: Some(BASE_EPOCH + 731 * DAY_SECONDS),
        ..Filters::default()
    };
    let started = Instant::now();
    let summary = queries::summary(&conn, &filters).unwrap();
    let models = queries::breakdown(&conn, "model", &filters).unwrap();
    let projects = queries::breakdown(&conn, "project", &filters).unwrap();
    let resources = queries::ctx_resources(&conn, &filters).unwrap();
    let buckets = queries::ctx_buckets(&conn, &filters).unwrap();
    let tools = queries::ctx_tools(&conn, &filters).unwrap();
    let skills = queries::ctx_skills(&conn, &filters).unwrap();
    let exec = queries::ctx_exec(&conn, &filters).unwrap();
    let elapsed = started.elapsed();
    assert!(summary.total_tokens > 0 && !models.is_empty() && !projects.is_empty());
    eprintln!(
        "PERF range_reload events={EVENT_COUNT} result_rows={} elapsed_ms={:.1} budget_ms={}",
        models.len()
            + projects.len()
            + resources.len()
            + buckets.len()
            + tools.len()
            + skills.len()
            + exec.len(),
        elapsed.as_secs_f64() * 1_000.0,
        RANGE_RELOAD_BUDGET.as_millis()
    );
    assert!(
        elapsed <= RANGE_RELOAD_BUDGET,
        "100k-record 30-day range reload took {:.1} ms; budget is {} ms",
        elapsed.as_secs_f64() * 1_000.0,
        RANGE_RELOAD_BUDGET.as_millis()
    );

    // Optional private-data check: callers may point at their own Ledger. Open
    // read-only, report only cardinality and timing, and never print row data.
    if let Some(path) = std::env::var_os("TOKENLEDGER_PERF_DB") {
        let real =
            rusqlite::Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        db::register_query_functions(&real).unwrap();
        let count: i64 = real
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .unwrap();
        let started = Instant::now();
        let points = queries::series(&real, &Filters::default(), "day").unwrap();
        let elapsed = started.elapsed();
        eprintln!(
            "PERF real_daily_series events={count} points={} elapsed_ms={:.1}",
            points.len(),
            elapsed.as_secs_f64() * 1_000.0,
        );
    }
}

// Manual comparison bench: per-query timings against a private Ledger
// snapshot (TOKENLEDGER_PERF_DB). Prints cardinality and timing only. Kept
// permanently — this is the measurement the queries.rs one-scan-vs-two
// comments rest on, and rebuilding it ad hoc is how it kept getting lost.
#[test]
#[ignore = "manual: TOKENLEDGER_PERF_DB=<snapshot> cargo test --release real_range_timings -- --ignored --nocapture"]
fn real_range_timings() {
    let path = std::env::var_os("TOKENLEDGER_PERF_DB").expect("set TOKENLEDGER_PERF_DB");
    let conn =
        rusqlite::Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    db::register_query_functions(&conn).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let windows: [(&str, Option<i64>); 4] = [
        ("day", Some(now - DAY_SECONDS)),
        ("week", Some(now - 7 * DAY_SECONDS)),
        ("month", Some(now - 30 * DAY_SECONDS)),
        ("total", None),
    ];
    for (name, start_ts) in windows {
        let filters = Filters { start_ts, ..Filters::default() };
        for _ in 0..2 {
            // warm-up pass first; the second line is the one to read
            let started = Instant::now();
            let s = queries::summary(&conn, &filters).unwrap();
            let t_summary = started.elapsed();
            let started = Instant::now();
            let m = queries::breakdown(&conn, "model", &filters).unwrap();
            let t_model = started.elapsed();
            let started = Instant::now();
            let p = queries::breakdown(&conn, "project", &filters).unwrap();
            let t_project = started.elapsed();
            let started = Instant::now();
            let t = queries::breakdown(&conn, "tool", &filters).unwrap();
            let t_tool = started.elapsed();
            eprintln!(
                "PERF {name}: summary={:.1}ms model={:.1}ms project={:.1}ms source={:.1}ms rows={}/{}/{} convs={}",
                t_summary.as_secs_f64() * 1e3,
                t_model.as_secs_f64() * 1e3,
                t_project.as_secs_f64() * 1e3,
                t_tool.as_secs_f64() * 1e3,
                m.len(), p.len(), t.len(), s.convs,
            );
        }
    }
}

#[test]
fn cached_local_bucketing_matches_sqlite_reference() {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_db(&dir.path().join("bucket-equivalence.db")).unwrap();
    // Includes epoch, contemporary summer/winter dates, and instants around
    // common northern/southern-hemisphere daylight-saving transition months.
    for timestamp in [
        0_i64,
        1_710_054_000,
        1_728_144_000,
        1_782_907_200,
        1_799_000_000,
    ] {
        let (cached_day, sqlite_day, cached_hour, sqlite_hour): (String, String, String, String) =
            conn.query_row(
                "SELECT tokenledger_local_bucket(?1, 0), \
                        strftime('%Y-%m-%d', ?1, 'unixepoch', 'localtime'), \
                        tokenledger_local_bucket(?1, 1), \
                        strftime('%Y-%m-%d %H:00', ?1, 'unixepoch', 'localtime')",
                [timestamp],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(cached_day, sqlite_day);
        assert_eq!(cached_hour, sqlite_hour);
    }
}

// ── the Limits page's estimate read ──
//
// `limit_readings` grows by a row per observation for as long as the app runs,
// and three statements in `queries::limits` touch it. The specification's posture
// (Evaluation timing, final paragraph) is "start with direct indexed range
// queries … do not scan unrelated Ledger history per row … add no cache until
// profiling demonstrates a need", so this measures the paths a person pays for
// and asserts the access shape of the statements the code actually issues.
const LIMIT_SOURCES: usize = 5;
const WINDOW_MINUTES: [i64; 2] = [300, 10_080];
// Past today's real table (about 2,200 rows) and past a year of heavy use: the
// question is whether an old, large table slows a page whose answer depends only
// on its recent tail.
const READING_COUNT: usize = 200_000;
// Epochs seeded with Usage behind them, the newest of which is still active and
// so never trains. Twelve completed ones leave the newest five representatives a
// comfortable core and give Stale reconstruction history to walk back through.
const RECENT_EPOCHS: i64 = 13;
// Nine intervals of this across a 9→90 rise: 10,000 tokens per point, identical
// in every epoch, so the epochs cohere and the page reaches Ready. Noisy amounts
// would time the withheld path instead, which is measured separately below.
const INTERVAL_TOKENS: i64 = 90_000;
// A plausible heavy Ledger inside one evidence horizon: about 240 Records a day
// for 84 days.
const BULK_RECORDS: usize = 20_000;
// Measured 62-69 ms across every path at 201,300 Readings and 21,170 selectable
// Records. Tight enough that the per-interval Record scan #167 removed (963 ms on
// this same fixture) cannot come back unnoticed.
const LIMITS_BUDGET: Duration = Duration::from_millis(150);
// The withheld page is different since #186: its five-hour windows are genuinely
// Gathering — no provable core anywhere in their history — and proving that now
// requires paging every stored Reading backwards to exhaustion, as the
// specification demands ("Stop at the first Ready proof or when history is
// exhausted"). Measured 1,447 ms over 202,600 Readings; the walk is linear in
// the table, so this budget is about the whole table's size, not the tail's.
const EXHAUSTION_BUDGET: Duration = Duration::from_millis(3_000);

/// Readings that form eligible evidence, in the two shapes production actually
/// writes (#183): proven `live` observations — the Companion's, the only shape
/// that carries an account — ten rising ones per epoch, with an account-less
/// `logs` Reading interleaved after each, the way Codex's rollouts land between
/// Companion checks. The mixed timeline is what the walk's pass-through has to
/// survive at scale; a fixture of provenance-bearing `logs` rows measured a
/// timeline production never produces. `epochs_back` places epochs relative to
/// `now`, so a caller can seed both the recent tail the estimator reads and the
/// ancient bulk it must skip.
fn seed_readings(
    conn: &mut rusqlite::Connection,
    now: i64,
    epochs_back: std::ops::Range<i64>,
) -> u64 {
    use crate::types::{LimitReading, ModelScope, ReadingProvenance};

    let mut readings = Vec::new();
    for s in 0..LIMIT_SOURCES {
        let source = format!("limsrc-{s}");
        for minutes in WINDOW_MINUTES {
            let span = minutes * 60;
            for age in epochs_back.clone() {
                let resets_at = now - age * span + span;
                for step in 0..10i64 {
                    let proven = LimitReading {
                        source: source.clone(),
                        window_key: format!("w{minutes}"),
                        window_minutes: Some(minutes),
                        used_pct: (step + 1) as f64 * 9.0,
                        resets_at,
                        observed_at: resets_at - span + step * (span / 10),
                        via: "live".to_string(),
                        plan: Some("perf".to_string()),
                        provenance: ReadingProvenance {
                            account_id: Some(format!("acct-{s}")),
                            metering_regime: Some("perf:regime".to_string()),
                            limit_id: Some(format!("perf:w{minutes}")),
                            model_scope: Some(ModelScope::All),
                            source_order: Some(step),
                            // Reaching back before the epoch, so coverage is
                            // proven and the intervals are complete.
                            covered_from: Some(resets_at - 4 * span),
                            external_activity: None,
                        },
                    };
                    // The rollout between two Companion checks: same figure,
                    // no account, no coverage. Benign, so it must pass through
                    // rather than end the run it sits inside.
                    let mut rollout = proven.clone();
                    rollout.via = "logs".to_string();
                    rollout.observed_at += span / 20;
                    rollout.provenance.account_id = None;
                    rollout.provenance.covered_from = None;
                    readings.push(proven);
                    readings.push(rollout);
                }
            }
        }
    }
    db::insert_limit_readings(conn, &readings).unwrap()
}

/// The access plan of a statement, joined into one line.
///
/// The parameters are bound to NULL only so the statement can be stepped: SQLite
/// fixes the plan when it prepares, before any value arrives, so what is bound
/// cannot change what this reports.
fn plan_of(conn: &rusqlite::Connection, sql: &str) -> String {
    let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
    let unbound: Vec<Option<i64>> = vec![None; stmt.parameter_count()];
    let rows = stmt
        .query_map(rusqlite::params_from_iter(unbound), |r| {
            r.get::<_, String>(3)
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    rows.join(" | ")
}

fn state_of(outcome: &crate::queries::LimitEstimateOutcome) -> String {
    use crate::queries::LimitEstimateOutcome as O;
    match outcome {
        O::Ready { .. } => "Ready",
        O::Gathering => "Gathering",
        O::Unstable => "Unstable",
        O::Stale => "Stale",
        O::Blocked => "Blocked",
    }
    .to_string()
}

fn ready_windows(cards: &[crate::queries::SourceLimits]) -> usize {
    cards
        .iter()
        .flat_map(|c| &c.windows)
        .filter(|w| {
            matches!(w.estimate.outcome, crate::queries::LimitEstimateOutcome::Ready { .. })
        })
        .count()
}

#[test]
#[ignore = "performance standard: run in release mode via `npm run perf:limits`"]
fn performance_standard_limits_estimate() {
    assert!(
        !cfg!(debug_assertions),
        "performance standards must run with --release (`npm run perf:limits`)"
    );

    let dir = tempfile::tempdir().unwrap();
    let mut conn = db::open_db(&dir.path().join("performance-limits.db")).unwrap();
    let now = BASE_EPOCH + 730 * DAY_SECONDS;

    // Unrelated Ledger history the estimate must not walk.
    let events: Vec<_> = (0..EVENT_COUNT).map(synthetic_event).collect();
    db::insert_events(&mut conn, &events).unwrap();
    drop(events);

    // Usage Records that can participate, one inside each interval the recent
    // Readings bound, carrying a fixed token amount.
    let mut participating = Vec::new();
    for s in 0..LIMIT_SOURCES {
        for minutes in WINDOW_MINUTES {
            let span = minutes * 60;
            for age in 0..RECENT_EPOCHS {
                let resets_at = now - age * span + span;
                for step in 0..9i64 {
                    let mut event = synthetic_event(0);
                    event.dedup_key = format!("limperf-{s}-{minutes}-{age}-{step}");
                    event.source = format!("limsrc-{s}");
                    event.model = None;
                    // One second into `(t0, t1]`, so membership is unambiguous.
                    event.timestamp = resets_at - span + step * (span / 10) + 1;
                    event.input_tokens = INTERVAL_TOKENS;
                    event.output_tokens = 0;
                    event.cache_read_tokens = 0;
                    event.cache_write_5m_tokens = 0;
                    event.cache_write_1h_tokens = 0;
                    participating.push(event);
                }
            }
        }
    }
    // Selectable but idle: inside the span `matching_usage` covers, so they are
    // read and carried, but in the 42-84 day band where no candidate epoch sits,
    // so the answer is unchanged and the two page timings differ only in volume.
    for s in 0..LIMIT_SOURCES {
        for i in 0..(BULK_RECORDS / LIMIT_SOURCES) {
            let mut event = synthetic_event(i);
            event.dedup_key = format!("limbulk-{s}-{i}");
            event.source = format!("limsrc-{s}");
            event.timestamp = now - 42 * DAY_SECONDS - (i as i64 * 90) % (42 * DAY_SECONDS);
            participating.push(event);
        }
    }
    db::insert_events(&mut conn, &participating).unwrap();
    // `account_id` sits outside db::COLS, so no scan can write it yet (#171).
    conn.execute(
        "UPDATE events SET account_id = 'acct-' || substr(source, 8) \
         WHERE source LIKE 'limsrc-%'",
        [],
    )
    .unwrap();
    drop(participating);

    // The recent tail the estimator reads, then the ancient bulk it must skip.
    let mut total = seed_readings(&mut conn, now, 0..RECENT_EPOCHS);
    let mut age = RECENT_EPOCHS;
    while total < READING_COUNT as u64 {
        total += seed_readings(&mut conn, now, age..age + 40);
        age += 40;
    }
    let ledger: i64 = conn
        .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
        .unwrap();

    // ── access shape, on the statements the code issues ──
    //
    // EXPLAINing the exported constants rather than a copy typed here: a copy
    // reports whatever it says regardless of what production does, which this
    // very assertion did until #167's review caught it passing with `account_id`
    // deleted from the real clause.
    let usage_plan = plan_of(&conn, crate::limits_evidence::MATCHING_USAGE_SQL);
    eprintln!("PERF limits_usage_plan {usage_plan}");
    assert!(
        usage_plan.contains("idx_events_evidence")
            && usage_plan.contains("source=?")
            && usage_plan.contains("account_id=?")
            && usage_plan.contains("timestamp>?"),
        "matching_usage must seek idx_events_evidence on all three fields, got: {usage_plan}"
    );
    assert!(
        !usage_plan.contains("SCAN events"),
        "matching_usage must not scan the Ledger, got: {usage_plan}"
    );

    // There is no seek to assert here, and that is the finding: `observed_at` is
    // the fourth column of the primary key, so the horizon filter cannot seek and
    // SQLite walks the index end to end. Not covering either — the statement
    // selects fifteen columns, so every row is fetched from the table as well.
    // Pinned as it stands so that adding an `observed_at` index fails this and
    // forces the numbers in docs/performance.md to be taken again, rather than
    // improving silently and leaving the doc wrong.
    let readings_plan = plan_of(&conn, crate::limits_evidence::STORED_READINGS_SQL);
    eprintln!("PERF limits_readings_plan {readings_plan}");
    assert!(
        readings_plan.starts_with("SCAN limit_readings USING INDEX"),
        "stored_readings' plan changed — re-measure and update docs/performance.md; got: {readings_plan}"
    );

    // No time bound on this one by design (see its doc), so it aggregates the
    // whole table. Reported so the cost of that is visible rather than implied.
    let displayed_plan = plan_of(&conn, queries::DISPLAYED_WINDOWS_SQL);
    eprintln!("PERF limits_displayed_plan {displayed_plan}");
    let plan_label_plan = plan_of(&conn, queries::PLAN_LABEL_SQL);
    eprintln!("PERF limits_plan_label_plan {plan_label_plan}");

    // ── 1. page open: the first read of this data in this process ──
    let started = Instant::now();
    let cards = queries::limits(&conn, now, &std::env::temp_dir()).unwrap();
    let page_open = started.elapsed();
    let ready = ready_windows(&cards);
    let windows: usize = cards.iter().map(|c| c.windows.len()).sum();
    eprintln!(
        "PERF limits_page_open readings={total} ledger={ledger} cards={} windows={windows} ready={ready} elapsed_ms={:.1} budget_ms={}",
        cards.len(),
        page_open.as_secs_f64() * 1e3,
        LIMITS_BUDGET.as_millis(),
    );
    // Every window Ready, not merely one: a Blocked or Gathering window measures
    // a fast path and proves nothing about the estimator, and a fixture that
    // half-worked would hide behind `> 0`.
    assert_eq!(
        ready,
        LIMIT_SOURCES * WINDOW_MINUTES.len(),
        "fixture must reach Ready on every window to time the estimator"
    );

    // ── 2. after a scan writes Readings, and 3. on the nextEvaluationAt timer ──
    seed_readings(&mut conn, now + 3_600, 0..1);
    let started = Instant::now();
    queries::limits(&conn, now + 3_600, &std::env::temp_dir()).unwrap();
    let after_scan = started.elapsed();
    let started = Instant::now();
    queries::limits(&conn, now + 7_200, &std::env::temp_dir()).unwrap();
    let on_timer = started.elapsed();
    eprintln!(
        "PERF limits_reevaluation after_scan_ms={:.1} on_timer_ms={:.1}",
        after_scan.as_secs_f64() * 1e3,
        on_timer.as_secs_f64() * 1e3,
    );

    // ── 4. the withheld path: Stale reconstruction and Gathering exhaustion ──
    //
    // Withdrawing the newest epochs' coverage drops each Series below three
    // recent candidates, which is the only branch that reaches `aged_out_core` —
    // one full estimator replay per completed epoch, newest-first, stopping at
    // the first that proves Ready. The weekly windows find their proof in the
    // bounded read and reconstruct Stale; the five-hour ones have no provable
    // core anywhere (their old epochs carry no Usage), so they page the whole
    // table backwards to exhaustion and stay Gathering — both spec-mandated
    // shapes, and a Ready page enters neither.
    // 42 days is the weekly window's own recency horizon, so this empties the
    // candidate set of both Series rather than only the short one's — the weekly
    // Series keeps twelve completed epochs inside that horizon and stays Ready if
    // only the newest few are withdrawn.
    conn.execute(
        "UPDATE limit_readings SET external_activity = 'perf:withdrawn' \
         WHERE resets_at > ?1",
        [now - 42 * DAY_SECONDS],
    )
    .unwrap();
    let started = Instant::now();
    let withheld_cards = queries::limits(&conn, now, &std::env::temp_dir()).unwrap();
    let withheld = started.elapsed();
    let mut states = std::collections::BTreeMap::new();
    for w in withheld_cards.iter().flat_map(|c| &c.windows) {
        *states.entry(state_of(&w.estimate.outcome)).or_insert(0) += 1;
    }
    eprintln!(
        "PERF limits_withheld states={states:?} elapsed_ms={:.1}",
        withheld.as_secs_f64() * 1e3,
    );
    assert_eq!(
        ready_windows(&withheld_cards),
        0,
        "withheld fixture still reads Ready"
    );
    // Stale is the proof that `aged_out_core` ran and found an older clock that
    // would have been Ready. Gathering would mean it walked every clock and found
    // nothing, which times the same loop but does not prove it reached a verdict.
    assert!(
        states.contains_key("Stale"),
        "withheld fixture never reached Stale, so the replay was not exercised: {states:?}"
    );

    // ── where the time goes ──
    //
    // Reported, not asserted: the budget belongs on the whole page read, and a
    // stage breakdown is what says which statement to look at when it fails.
    // Runs last so it cannot warm the caches the page-open number is measured
    // against, and re-derives against the Readings as they now stand.
    let since = now - 2 * crate::limits_estimator::recency_horizon(Some(10_080));
    let started = Instant::now();
    let staged = crate::limits_evidence::stored_readings(&conn, since).unwrap();
    let t_readings = started.elapsed();
    let started = Instant::now();
    let staged_usage = crate::limits_evidence::matching_usage(&conn, &staged).unwrap();
    let t_usage = started.elapsed();
    let started = Instant::now();
    let staged_evidence = crate::limits_evidence::derive(&staged, &staged_usage).unwrap();
    let t_derive = started.elapsed();
    // The two statements the stage trio does not cover, both named in
    // queries::limits: the newest-epoch self-join that decides which windows the
    // page draws, and the per-Source plan lookup.
    let started = Instant::now();
    let displayed: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM ({})", queries::DISPLAYED_WINDOWS_SQL),
            [600],
            |r| r.get(0),
        )
        .unwrap();
    let t_displayed = started.elapsed();
    assert_eq!(displayed as usize, LIMIT_SOURCES * WINDOW_MINUTES.len());
    // What the plan-label statement costs when it has to run. It is the fallback
    // now — the page takes the label from the Readings it already holds — so this
    // is the cost avoided rather than a cost paid, and it is measured to keep that
    // decision honest if anyone reverts it.
    let started = Instant::now();
    for s in 0..LIMIT_SOURCES {
        let _: Option<String> = conn
            .query_row(queries::PLAN_LABEL_SQL, [format!("limsrc-{s}")], |r| r.get(0))
            .unwrap();
    }
    let t_plan_label = started.elapsed();
    eprintln!(
        "PERF limits_stages in_horizon_readings={} matching_records={} partitions={} \
         read_readings_ms={:.1} read_usage_ms={:.1} derive_ms={:.1} displayed_windows_ms={:.1} \
         plan_label_fallback_ms={:.1}",
        staged.len(),
        staged_usage.len(),
        staged_evidence.partitions.len(),
        t_readings.as_secs_f64() * 1e3,
        t_usage.as_secs_f64() * 1e3,
        t_derive.as_secs_f64() * 1e3,
        t_displayed.as_secs_f64() * 1e3,
        t_plan_label.as_secs_f64() * 1e3,
    );

    for (name, elapsed, budget) in [
        ("page open", page_open, LIMITS_BUDGET),
        ("after scan", after_scan, LIMITS_BUDGET),
        ("on timer", on_timer, LIMITS_BUDGET),
        // The one path allowed to cost more: a Gathering window pages all of
        // history to prove nothing older would change its answer (#186).
        ("withheld", withheld, EXHAUSTION_BUDGET),
    ] {
        assert!(
            elapsed <= budget,
            "{name} took {:.1} ms with {total} Readings; budget is {} ms",
            elapsed.as_secs_f64() * 1e3,
            budget.as_millis(),
        );
    }

    // Optional private-data check, the same recipe the Overview gate uses: open
    // read-only, report cardinality and timing, never row data. A real Ledger
    // today has no proven account identity (#171), so this measures the Blocked
    // path — which is the one every real user is on until that ticket lands.
    if let Some(path) = std::env::var_os("TOKENLEDGER_PERF_DB") {
        let real =
            rusqlite::Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        db::register_query_functions(&real).unwrap();
        let readings: i64 = real
            .query_row("SELECT COUNT(*) FROM limit_readings", [], |r| r.get(0))
            .unwrap();
        let started = Instant::now();
        let real_cards = queries::limits(&real, now, &std::env::temp_dir()).unwrap();
        let elapsed = started.elapsed();
        eprintln!(
            "PERF real_limits readings={readings} cards={} windows={} ready={} elapsed_ms={:.1}",
            real_cards.len(),
            real_cards.iter().map(|c| c.windows.len()).sum::<usize>(),
            ready_windows(&real_cards),
            elapsed.as_secs_f64() * 1e3,
        );
    }
}
