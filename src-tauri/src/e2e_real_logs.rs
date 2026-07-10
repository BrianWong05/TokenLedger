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

    // Context attribution invariants (spec 2026-07-10-context-breakdown).
    // Partition exact where attributed:
    let bad_partition: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE ctx_messages IS NOT NULL AND \
             ctx_messages + COALESCE(ctx_system, 0) + COALESCE(ctx_reasoning, 0) != \
             input_tokens + cache_read_tokens + cache_write_5m_tokens + cache_write_1h_tokens",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bad_partition, 0, "primary partition must equal billed context exactly");

    // Secondary ⊆ messages:
    let bad_subset: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE \
             COALESCE(ctx_toolcalls, 0) > COALESCE(ctx_messages, 0) OR \
             COALESCE(ctx_mcp, 0) > COALESCE(ctx_messages, 0) OR \
             COALESCE(ctx_skills, 0) > COALESCE(ctx_messages, 0)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bad_subset, 0, "secondary categories are subsets of messages");

    // Hermes: no content, everything NULL:
    let hermes_ctx: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE source='hermes' AND ctx_messages IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(hermes_ctx, 0);

    // Claude attributed the bulk of its events (real transcripts on this machine):
    let (claude_total, claude_attr): (i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COUNT(ctx_messages) FROM events WHERE source='claude'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    println!("\n=== claude ctx coverage: {claude_attr}/{claude_total} events attributed ===");
    assert!(
        claude_attr * 10 >= claude_total * 5,
        "expected ≥50% of claude events attributed (got {claude_attr}/{claude_total})"
    );

    let resources: Vec<(String, String, i64)> = {
        let mut stmt = conn
            .prepare("SELECT source, kind, COUNT(DISTINCT name) FROM ctx_resources GROUP BY source, kind")
            .unwrap();
        let it = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
        it.collect::<rusqlite::Result<Vec<_>>>().unwrap()
    };
    println!("=== ctx resources ===");
    for (s, k, n) in &resources {
        println!("  {s:<8} {k:<12} {n}");
    }

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

    // Exact-bucket partition (spec 2026-07-10-context-drilldown): per source,
    // history + new_input + system + response + reasoning == total usage.
    let buckets = queries::ctx_buckets(&conn, &all).unwrap();
    for b in &buckets {
        let (tot_in, tot_out, tot_cr, tot_cw): (i64, i64, i64, i64) = conn
            .query_row(
                "SELECT SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens), \
                 SUM(cache_write_5m_tokens + cache_write_1h_tokens) FROM events WHERE source = ?1",
                [&b.source],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        let total = tot_in + tot_out + tot_cr + tot_cw;
        let sum = b.history + b.new_input + b.system.unwrap_or(0) + b.response
            + b.reasoning.unwrap_or(0);
        assert_eq!(sum, total, "bucket partition exact for {}", b.source);
        println!(
            "  {:<8} history={} new_input={} system={:?} response={} reasoning={:?}",
            b.source, b.history, b.new_input, b.system, b.response, b.reasoning
        );
    }

    // Tool weights: print top rows per source for an eyeball check.
    let tools = queries::ctx_tools(&conn, &all).unwrap();
    println!("=== top ctx_tools ===");
    for t in tools.iter().take(12) {
        println!("  {:<8} {:<28} est={:<10} calls={}", t.source, t.name, t.est_tokens, t.calls);
    }
    assert!(
        tools.iter().any(|t| t.source == "claude" && t.est_tokens > 0),
        "expected claude tool weights on real logs"
    );
}
