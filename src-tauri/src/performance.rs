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
