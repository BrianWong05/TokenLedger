// End-to-end verification against the REAL logs on this machine (Task 16).
// Not run by default (touches ~1GB of real user data and can take several
// seconds): `cargo test --release e2e_real_logs -- --ignored --nocapture`
use crate::{db, pricing, queries, scan};

#[test]
#[ignore]
fn e2e_real_logs() {
    let roots = scan::SourceRoots::default_roots();

    let dir = tempfile::tempdir().unwrap();
    let mut conn = db::open_db(&dir.path().join("tokenledger.db")).unwrap();

    let n = pricing::refresh_prices(&mut conn, dir.path()).expect("refresh_prices failed");
    println!("\n=== prices loaded: {n} rows ===");

    let status = scan::run_scan(&mut conn, &roots);
    println!("\n=== per-source scan results ===");
    for s in &status.sources {
        println!(
            "  {:<8} inserted={:<8} skipped={:<8} error={:?}",
            s.source, s.events_inserted, s.lines_skipped, s.error
        );
    }

    let all = queries::Filters::default();
    let by_tool = queries::breakdown(&conn, "tool", &all).unwrap();
    println!("\n=== breakdown by tool ===");
    for row in &by_tool {
        println!(
            "  {:<8} tokens={:<12} requests={:<8} cost={:?}",
            row.key, row.total_tokens, row.requests, row.cost
        );
    }

    let summary = queries::summary(&conn, &all).unwrap();
    println!("\n=== overall summary ===");
    println!("  input_tokens        {}", summary.input_tokens);
    println!("  output_tokens       {}", summary.output_tokens);
    println!("  cache_read_tokens   {}", summary.cache_read_tokens);
    println!("  cache_write_tokens  {}", summary.cache_write_tokens);
    println!("  total_tokens        {}", summary.total_tokens);
    println!("  requests            {}", summary.requests);
    println!("  cost                {:?}", summary.cost);
    println!("  has_unpriced        {}", summary.has_unpriced);
    println!("  unpriced_models     {:?}", summary.unpriced_models);
    println!("  cache_hit_rate      {:.4}", summary.cache_hit_rate);

    assert_eq!(status.sources.len(), 4, "expected all 4 sources to report");
    assert!(
        summary.total_tokens > 0,
        "expected non-zero tokens scanning real logs"
    );

    // Claude-only category totals, for a direct ccusage cross-check (Task 16 step 3).
    let claude_only = queries::Filters {
        tools: vec!["claude".to_string()],
        ..Default::default()
    };
    let claude_summary = queries::summary(&conn, &claude_only).unwrap();
    println!("\n=== claude-only summary (for ccusage cross-check) ===");
    println!("  input_tokens        {}", claude_summary.input_tokens);
    println!("  output_tokens       {}", claude_summary.output_tokens);
    println!("  cache_read_tokens   {}", claude_summary.cache_read_tokens);
    println!("  cache_write_tokens  {}", claude_summary.cache_write_tokens);
    println!("  total_tokens        {}", claude_summary.total_tokens);
    println!("  requests            {}", claude_summary.requests);
    println!("  cost                {:?}", claude_summary.cost);
}
