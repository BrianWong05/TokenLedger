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
// `limit_readings` is the one table that grows by a row per observation for as
// long as the app runs, and three statements in `queries::limits` touch it. The
// spec's posture (Evaluation timing, final paragraph) is "start with direct
// indexed range queries … do not scan unrelated Ledger history per row … add no
// cache until profiling demonstrates a need", so this measures the three paths a
// person actually pays for and asserts the access shape rather than only the
// clock.
const LIMIT_SOURCES: usize = 5;
// Two windows per Source — a session and a weekly — which is the shipped shape.
const WINDOW_MINUTES: [i64; 2] = [300, 10_080];
// Deliberately far past today's real table (about 2,200 rows) and past a year of
// heavy use: the question is whether an old, large table slows a page whose
// answer only ever depends on its recent tail.
const READING_COUNT: usize = 200_000;
// Completed epochs with Usage behind them. Twelve, so the newest five
// representatives are a comfortable core and Stale reconstruction has history to
// walk back through.
const RECENT_EPOCHS: i64 = 13;
// Nine intervals of this each, over a 9→90 percentage rise: 810,000 tokens across
// 81 points is 10,000 per point, identical in every epoch.
const INTERVAL_TOKENS: i64 = 90_000;
// A plausible heavy Ledger inside the evidence horizon: about 240 Records a day
// for 84 days. Nothing about this is extreme.
const BULK_RECORDS: usize = 20_000;
// Measured 62-69 ms across all three paths at 201,300 Readings and 21,170
// participating Records. The headroom matches the range-reload gate's: tight
// enough that the per-interval Record scan this ticket removed (963 ms on the
// same fixture) cannot come back unnoticed.
const LIMITS_BUDGET: Duration = Duration::from_millis(150);

/// Readings that form eligible evidence: full provenance, ten rising
/// observations per epoch, epochs back to back. `age_epochs` places the epoch
/// relative to `now`, so a caller can seed both the recent tail the estimator
/// reads and the ancient bulk it must skip.
fn seed_readings(
    conn: &mut rusqlite::Connection,
    now: i64,
    sources: usize,
    epochs_back: std::ops::Range<i64>,
) -> u64 {
    use crate::types::{LimitReading, ModelScope, ReadingProvenance};

    let mut readings = Vec::new();
    for s in 0..sources {
        let source = format!("limsrc-{s}");
        for minutes in WINDOW_MINUTES {
            let span = minutes * 60;
            for age in epochs_back.clone() {
                let resets_at = now - age * span + span;
                for step in 0..10i64 {
                    readings.push(LimitReading {
                        source: source.clone(),
                        window_key: format!("w{minutes}"),
                        window_minutes: Some(minutes),
                        // Ten rising points: enough for a qualifying run, and the
                        // same ratio in every epoch so a stable core exists.
                        used_pct: (step + 1) as f64 * 9.0,
                        resets_at,
                        observed_at: resets_at - span + step * (span / 10),
                        via: "logs".to_string(),
                        plan: Some("perf".to_string()),
                        provenance: ReadingProvenance {
                            account_id: Some(format!("acct-{s}")),
                            metering_regime: Some("perf:regime".to_string()),
                            limit_id: Some(format!("perf:w{minutes}")),
                            model_scope: Some(ModelScope::All),
                            source_order: Some(step),
                            // Coverage reaching back before the epoch, so every
                            // interval is complete rather than refused.
                            covered_from: Some(resets_at - 4 * span),
                            external_activity: None,
                        },
                    });
                }
            }
        }
    }
    db::insert_limit_readings(conn, &readings).unwrap()
}

/// The one access-shape assertion that matters: the Usage side must find its
/// Records through `idx_events_evidence`, not by walking the Ledger.
fn plan_of(conn: &rusqlite::Connection, sql: &str) -> String {
    let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(3))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    rows.join(" | ")
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

    // Usage Records that CAN participate, one inside each interval the recent
    // Readings bound, carrying a fixed token amount. Fixed on purpose: a noisy
    // amount gives each epoch a different run ratio, the cores stop cohering, and
    // the page reads Unstable — which would time the withheld path rather than
    // the estimator, and prove nothing.
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
    db::insert_events(&mut conn, &participating).unwrap();
    // `account_id` sits outside db::COLS, so the scan cannot write it yet (#171).
    conn.execute(
        "UPDATE events SET account_id = 'acct-' || substr(source, 8) \
         WHERE source LIKE 'limsrc-%'",
        [],
    )
    .unwrap();
    drop(participating);

    // The recent tail the estimator actually reads, then the ancient bulk it must
    // skip past. Epoch 0 is active (never trains); 1..13 are completed.
    let recent = seed_readings(&mut conn, now, LIMIT_SOURCES, 0..RECENT_EPOCHS);
    let mut ancient = 0u64;
    let mut age = RECENT_EPOCHS;
    while recent + ancient < READING_COUNT as u64 {
        ancient += seed_readings(&mut conn, now, LIMIT_SOURCES, age..age + 40);
        age += 40;
    }
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM limit_readings", [], |r| r.get(0))
        .unwrap();
    let ledger: i64 = conn
        .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
        .unwrap();

    // Access shape, before any timing: the Usage side must seek.
    let usage_plan = plan_of(
        &conn,
        "SELECT timestamp, model, input_tokens FROM events \
         WHERE source = 'limsrc-0' AND account_id = 'acct-0' \
           AND timestamp > 0 AND timestamp <= 1",
    );
    eprintln!("PERF limits_usage_plan {usage_plan}");
    assert!(
        usage_plan.contains("idx_events_evidence"),
        "matching_usage must seek idx_events_evidence, got: {usage_plan}"
    );
    assert!(
        !usage_plan.contains("SCAN events"),
        "matching_usage must not scan the Ledger, got: {usage_plan}"
    );

    let readings_plan = plan_of(
        &conn,
        "SELECT source, observed_at FROM limit_readings WHERE observed_at >= 0 \
         ORDER BY source, window_key, resets_at, observed_at",
    );
    eprintln!("PERF limits_readings_plan {readings_plan}");

    // Where the time actually goes, measured on the same statements the query
    // runs. Reported rather than asserted: the budget belongs on the whole page
    // read, and a stage breakdown is what tells anyone which of the three to look
    // at when it fails.
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
    eprintln!(
        "PERF limits_stages in_horizon_readings={} matching_records={} partitions={}          read_readings_ms={:.1} read_usage_ms={:.1} derive_ms={:.1}",
        staged.len(),
        staged_usage.len(),
        staged_evidence.partitions.len(),
        t_readings.as_secs_f64() * 1e3,
        t_usage.as_secs_f64() * 1e3,
        t_derive.as_secs_f64() * 1e3,
    );

    // 1. Page open — the cold read a person pays for on every visit to the tab.
    let started = Instant::now();
    let cards = queries::limits(&conn, now).unwrap();
    let page_open = started.elapsed();
    let ready = cards
        .iter()
        .flat_map(|c| &c.windows)
        .filter(|w| w.estimate.state == crate::limits_readiness::ReadinessState::Ready)
        .count();
    eprintln!(
        "PERF limits_page_open readings={total} ledger={ledger} cards={} windows={} ready={ready} elapsed_ms={:.1} budget_ms={}",
        cards.len(),
        cards.iter().map(|c| c.windows.len()).sum::<usize>(),
        page_open.as_secs_f64() * 1e3,
        LIMITS_BUDGET.as_millis(),
    );
    // A Blocked page would measure the fast path and prove nothing about the
    // estimator, so the fixture has to actually reach Ready.
    assert!(ready > 0, "fixture produced no Ready estimate to time");

    // 2. Scan-triggered reevaluation: a scan wrote new Readings, the page reruns
    //    the same query.
    seed_readings(&mut conn, now + 3_600, LIMIT_SOURCES, 0..1);
    let started = Instant::now();
    queries::limits(&conn, now + 3_600).unwrap();
    let after_scan = started.elapsed();

    // 3. The nextEvaluationAt timer: same data, later instant, no vendor call.
    let started = Instant::now();
    queries::limits(&conn, now + 7_200).unwrap();
    let on_timer = started.elapsed();

    eprintln!(
        "PERF limits_reevaluation after_scan_ms={:.1} on_timer_ms={:.1}",
        after_scan.as_secs_f64() * 1e3,
        on_timer.as_secs_f64() * 1e3,
    );

    // A heavy Ledger. These Records sit in the 42-84 day band: inside the span
    // `matching_usage` selects, so they are carried and cost what they cost, but
    // outside every candidate epoch, so the Ready answer above is unchanged and
    // the two numbers differ only in Record volume.
    let mut bulk = Vec::new();
    for s in 0..LIMIT_SOURCES {
        for i in 0..(BULK_RECORDS / LIMIT_SOURCES) {
            let mut event = synthetic_event(i);
            event.dedup_key = format!("limbulk-{s}-{i}");
            event.source = format!("limsrc-{s}");
            event.timestamp =
                now - 42 * DAY_SECONDS - (i as i64 * 90) % (42 * DAY_SECONDS);
            bulk.push(event);
        }
    }
    db::insert_events(&mut conn, &bulk).unwrap();
    conn.execute(
        "UPDATE events SET account_id = 'acct-' || substr(source, 8)          WHERE source LIKE 'limsrc-%' AND account_id IS NULL",
        [],
    )
    .unwrap();
    drop(bulk);

    let started = Instant::now();
    let heavy_cards = queries::limits(&conn, now).unwrap();
    let heavy = started.elapsed();
    let heavy_ready = heavy_cards
        .iter()
        .flat_map(|c| &c.windows)
        .filter(|w| w.estimate.state == crate::limits_readiness::ReadinessState::Ready)
        .count();
    let heavy_records =
        crate::limits_evidence::matching_usage(&conn, &staged).unwrap().len();
    eprintln!(
        "PERF limits_heavy_ledger matching_records={heavy_records} ready={heavy_ready}          elapsed_ms={:.1}",
        heavy.as_secs_f64() * 1e3,
    );
    assert_eq!(heavy_ready, ready, "bulk Records must not change the answer");
    assert!(
        heavy <= LIMITS_BUDGET,
        "page open with {heavy_records} matching Records took {:.1} ms; budget is {} ms",
        heavy.as_secs_f64() * 1e3,
        LIMITS_BUDGET.as_millis(),
    );

    for (name, elapsed) in [
        ("page open", page_open),
        ("after scan", after_scan),
        ("on timer", on_timer),
    ] {
        assert!(
            elapsed <= LIMITS_BUDGET,
            "{name} took {:.1} ms with {total} Readings; budget is {} ms",
            elapsed.as_secs_f64() * 1e3,
            LIMITS_BUDGET.as_millis(),
        );
    }
}
