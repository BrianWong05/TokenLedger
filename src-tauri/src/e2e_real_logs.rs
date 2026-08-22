// End-to-end verification against the REAL logs on this machine (Task 16).
// Not run by default (touches ~1GB of real user data and can take several
// seconds): `cargo test --release e2e_real_logs -- --ignored --nocapture`
use crate::{db, pricing, queries, scan, source_catalog};

#[test]
#[ignore]
fn e2e_real_logs() {
    let roots = scan::SourceRoots::default_roots();

    let dir = tempfile::tempdir().unwrap();
    let mut conn = db::open_db(&dir.path().join("tokenledger.db")).unwrap();

    let status = scan::run_scan(&mut conn, &roots);

    // Prices AFTER the scan, deliberately: publisher rates are fetched per Model
    // in the Ledger, so refreshing first would find an empty Ledger, fetch
    // nothing, and let this test pass with that whole tier dark (ADR-0009).
    let n = pricing::refresh_prices(&mut conn, dir.path()).expect("refresh_prices failed");
    println!("\n=== prices loaded: {n} rows ===");
    let published: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM prices WHERE catalog NOT IN ('litellm', 'openrouter')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    println!("=== of which publisher rates: {published} ===");
    assert!(published > 0, "no Model resolved to its publisher's rate");

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
            row.key.as_deref().unwrap_or("Unattributed usage"),
            row.total_tokens,
            row.requests,
            row.cost
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

    // Taken from the catalog rather than written down: the guard worth keeping
    // is that no source is silently dropped from the scan, and a literal count
    // only measures how long ago someone added one — this said 14 against a
    // catalog of 15, and `main` had to bump it by hand to say 15. Comparing
    // keys also names the offender instead of reporting arithmetic.
    let expected: Vec<&str> =
        source_catalog::catalog().sources.iter().map(|s| s.key.as_str()).collect();
    let reported: Vec<&str> = status.sources.iter().map(|s| s.source.as_str()).collect();
    assert_eq!(reported, expected, "every catalogued source must report a status");
    assert!(
        summary.total_tokens > 0,
        "expected non-zero tokens scanning real logs"
    );

    // Context attribution invariants (spec 2026-07-10-context-breakdown).
    // The universal cross-Source invariants live in crate::invariants, shared
    // with the hermetic twelve-Source test so both exercise identical SQL.
    crate::invariants::assert_partition_exact(&conn);
    crate::invariants::assert_secondary_subset(&conn);
    crate::invariants::assert_hermes_ctx_null(&conn);

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
        let it = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap();
        it.collect::<rusqlite::Result<Vec<_>>>().unwrap()
    };
    println!("=== ctx resources ===");
    for (s, k, n) in &resources {
        println!("  {s:<8} {k:<12} {n}");
    }

    // Claude-only category totals.
    let claude_only = queries::Filters {
        tools: vec!["claude".to_string()],
        ..Default::default()
    };
    let claude_summary = queries::summary(&conn, &claude_only).unwrap();
    println!("\n=== claude-only summary ===");
    println!("  input_tokens        {}", claude_summary.input_tokens);
    println!("  output_tokens       {}", claude_summary.output_tokens);
    println!("  cache_read_tokens   {}", claude_summary.cache_read_tokens);
    println!(
        "  cache_write_tokens  {}",
        claude_summary.cache_write_tokens
    );
    println!("  total_tokens        {}", claude_summary.total_tokens);
    println!("  requests            {}", claude_summary.requests);
    println!("  cost                {:?}", claude_summary.cost);

    // Exact-bucket partition (spec 2026-07-10-context-drilldown): per source,
    // history + new_input + system + response + reasoning == total usage.
    crate::invariants::assert_bucket_partition_exact(&conn);
    let buckets = queries::ctx_buckets(&conn, &all).unwrap();
    for b in &buckets {
        println!(
            "  {:<8} history={} new_input={} system={:?} response={} reasoning={:?}",
            b.source, b.history, b.new_input, b.system, b.response, b.reasoning
        );
    }

    // Tool weights: print top rows per source for an eyeball check.
    let tools = queries::ctx_tools(&conn, &all).unwrap();
    println!("=== top ctx_tools ===");
    for t in tools.iter().take(12) {
        println!(
            "  {:<8} {:<28} est={:<10} calls={}",
            t.source, t.name, t.est_tokens, t.calls
        );
    }
    assert!(
        tools
            .iter()
            .any(|t| t.source == "claude" && t.est_tokens > 0),
        "expected claude tool weights on real logs"
    );

    // Bash exec facets (spec 2026-07-10-bash-exec-drilldown): rows must exist
    // for claude on real logs; print the top kinds and executables.
    let exec = queries::ctx_exec(&conn, &all).unwrap();
    assert!(
        exec.iter()
            .any(|r| r.source == "claude" && r.est_tokens > 0),
        "expected claude ctx_exec rows on real logs"
    );
    let mut by_kind: std::collections::HashMap<&str, (i64, i64)> = std::collections::HashMap::new();
    let mut by_exe: std::collections::HashMap<&str, (i64, i64)> = std::collections::HashMap::new();
    for r in exec.iter().filter(|r| r.source == "claude") {
        let k = by_kind.entry(r.kind.as_str()).or_insert((0, 0));
        k.0 += r.est_tokens;
        k.1 += r.calls;
        let e = by_exe.entry(r.exe.as_str()).or_insert((0, 0));
        e.0 += r.est_tokens;
        e.1 += r.calls;
    }
    let mut kinds: Vec<_> = by_kind.into_iter().collect();
    kinds.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    println!("=== top exec kinds (claude) ===");
    for (k, (est, calls)) in kinds.iter().take(8) {
        println!("  {:<16} est={:<12} calls={}", k, est, calls);
    }
    let mut exes: Vec<_> = by_exe.into_iter().collect();
    exes.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    println!("=== top exec executables (claude) ===");
    for (e, (est, calls)) in exes.iter().take(8) {
        println!("  {:<16} est={:<12} calls={}", e, est, calls);
    }
}

// Per-skill weights against the real transcripts (#84). Separate from the
// sweep above so it can be run alone:
//   cargo test --no-default-features e2e_real_skills -- --ignored --nocapture
#[test]
#[ignore]
fn e2e_real_skills() {
    let roots = scan::SourceRoots::default_roots();
    let dir = tempfile::tempdir().unwrap();
    let mut conn = db::open_db(&dir.path().join("tokenledger.db")).unwrap();
    scan::run_scan(&mut conn, &roots);

    let rows = queries::ctx_skills(&conn, &queries::Filters::default()).unwrap();
    println!("\n=== heaviest skills across the real Ledger ===");
    for r in rows.iter().take(10) {
        println!("  {:<44} {:>9} est. tok  ({} uses)", r.name, r.est_tokens, r.uses);
    }
    assert!(!rows.is_empty(), "real transcripts invoke skills; none were weighed");
    assert!(
        rows.windows(2).all(|w| w[0].est_tokens >= w[1].est_tokens),
        "heaviest first",
    );
    // The whole point of the change: a skill's body dwarfs its invocation
    // overhead, so the top skill must be far above the ~24 tokens a Skill
    // tool call costs on its own.
    assert!(rows[0].est_tokens > 1_000, "bodies are not being attributed");

    // A plugin skill keeps its namespace, so it never merges with a local
    // skill of the same name.
    if let Some(ns) = rows.iter().find(|r| r.name.contains(':')) {
        println!("  namespaced example: {}", ns.name);
    }

    // Skills stay a subset of messages — the invariant the reclassification
    // had to preserve.
    crate::invariants::assert_secondary_subset(&conn);
}

// TOKL-26 against the real transcripts: Claude's `usage.iterations` books one
// Usage Record per API call, so a model fallback stops reporting one call of two
// and each call's tokens land under the Model that served them.
//   cargo test --release e2e_real_iterations -- --ignored --nocapture
//
// Every expectation here is derived, never written down: a literal would drift
// (see the source-count comment above for what that costs), and the figures move
// whenever a new fallback happens — the 10th turn appeared mid-implementation.
// But asserting only shape is not enough either, because a parser that stamped
// every call with the top-level rollup would keep the counts, the turns and the
// partition all correct while getting every figure wrong. So the per-call
// figures are checked against the Artifact itself.
#[test]
#[ignore]
fn e2e_real_iterations() {
    let roots = scan::SourceRoots::default_roots();
    let dir = tempfile::tempdir().unwrap();
    let mut conn = db::open_db(&dir.path().join("tokenledger.db")).unwrap();
    scan::run_scan(&mut conn, &roots);

    let mut stmt = conn
        .prepare(
            "SELECT model, COUNT(*), \
                    SUM(input_tokens + output_tokens + cache_read_tokens \
                        + cache_write_5m_tokens + cache_write_1h_tokens) \
             FROM events \
             WHERE source = 'claude' AND dedup_key GLOB '*#it[0-9]*' \
             GROUP BY model ORDER BY model",
        )
        .unwrap();
    let by_model: Vec<(String, i64, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();

    println!("\n=== Claude per-iteration Records ===");
    for (model, records, tokens) in &by_model {
        println!("  {model:<22} {records:>3} records  {tokens:>12} tokens");
    }

    let records: i64 = by_model.iter().map(|r| r.1).sum();
    let tokens: i64 = by_model.iter().map(|r| r.2).sum();
    let one = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
    let calls = one(
        "SELECT COALESCE(SUM(api_calls), 0) FROM events \
         WHERE source = 'claude' AND dedup_key GLOB '*#it[0-9]*'",
    );
    let turns = one(
        "SELECT COUNT(DISTINCT substr(dedup_key, 1, instr(dedup_key, '#it') - 1)) FROM events \
         WHERE source = 'claude' AND dedup_key GLOB '*#it[0-9]*'",
    );
    println!("  ---\n  {records} records, {calls} calls, {tokens} tokens, {turns} turns");

    // Non-vacuous: every assertion below is trivially true of an empty set, so
    // the population has to exist first. Real transcripts hold model fallbacks;
    // finding none means the parser stopped reading the field.
    assert!(records > 0, "no per-iteration Records: usage.iterations is not being read");
    assert!(tokens > 0, "per-iteration Records carry no tokens");

    // Each Record is exactly one API call, so Requests — which sums api_calls,
    // and per CONTEXT.md is never a Ledger row count — is exact rather than a
    // floor for a fallback message.
    assert_eq!(calls, records, "every per-iteration Record is exactly one call");

    // Requests moved: one Record per turn is what the old parser booked. The
    // `thin` check below is strictly stronger and implies this one — every
    // Record carries non-zero tokens by construction, so a turn outweighing its
    // heaviest call means it holds two of them. This weaker assertion is kept
    // only because it fires first and names the regression directly.
    assert!(records > turns, "Requests did not move: still one Record per turn");
    let thin = one(
        "SELECT COUNT(*) FROM ( \
           SELECT substr(dedup_key, 1, instr(dedup_key, '#it') - 1) AS turn, \
                  SUM(input_tokens + output_tokens + cache_read_tokens \
                      + cache_write_5m_tokens + cache_write_1h_tokens) AS total, \
                  MAX(input_tokens + output_tokens + cache_read_tokens \
                      + cache_write_5m_tokens + cache_write_1h_tokens) AS heaviest \
           FROM events WHERE source = 'claude' AND dedup_key GLOB '*#it[0-9]*' \
           GROUP BY turn HAVING total <= heaviest)",
    );
    assert_eq!(thin, 0, "a turn booked no more than its heaviest single call: tokens did not move");

    // The plain key each multi-call turn used to book under is gone. A surviving
    // one would double-count the turn — once as the old mixed-Model row, once as
    // its per-iteration Records.
    let orphans = one(
        "SELECT COUNT(*) FROM events o \
         WHERE o.source = 'claude' AND o.dedup_key NOT GLOB '*#it[0-9]*' \
           AND EXISTS (SELECT 1 FROM events i \
                       WHERE i.dedup_key GLOB o.dedup_key || '#it[0-9]*')",
    );
    assert_eq!(orphans, 0, "a superseded plain-key Record survived the re-parse");

    // Not asserted, deliberately: the ticket puts telling a retry apart from a
    // genuine extra call out of scope, so a same-Model multi-call turn must not
    // fail the build. Every turn observed so far is a fallback across two
    // Models, and that is worth seeing if it ever stops being true, because the
    // Cost argument for per-Model Records rests on it.
    let single_model_turns = one(
        "SELECT COUNT(*) FROM ( \
           SELECT substr(dedup_key, 1, instr(dedup_key, '#it') - 1) AS turn \
           FROM events WHERE source = 'claude' AND dedup_key GLOB '*#it[0-9]*' \
           GROUP BY turn HAVING COUNT(DISTINCT model) = 1)",
    );
    if single_model_turns > 0 {
        println!("  note: {single_model_turns} multi-call turn(s) span a single Model (a retry, not a fallback)");
    }

    // The figures themselves, against the Artifact rather than a constant. Only
    // two fields are re-read — the per-call `output_tokens` and `model` — which
    // is enough to catch the two regressions the shape checks cannot see: a
    // Record stamped with the top-level rollup instead of its own iteration's
    // (the top level equals the LAST iteration, so slot 0 mismatches), and a
    // Model taken from the message instead of the call that ran it. Deliberately
    // NOT a second implementation of the bucket split: that would only restate
    // the parser and drift away from it.
    let mut want: std::collections::HashMap<String, (String, i64)> = std::collections::HashMap::new();
    let mut stack = vec![dirs::home_dir().unwrap().join(".claude/projects")];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|x| x != "jsonl") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            for line in text.lines() {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
                if v["type"].as_str() != Some("assistant") {
                    continue;
                }
                let msg = &v["message"];
                let Some(id) = msg["id"].as_str().filter(|i| !i.is_empty()) else { continue };
                let Some(iterations) =
                    msg["usage"]["iterations"].as_array().filter(|a| a.len() > 1)
                else {
                    continue;
                };
                let base = match v["requestId"].as_str() {
                    Some(r) => format!("claude:{id}:{r}"),
                    None => format!("claude:{id}"),
                };
                for (i, it) in iterations.iter().enumerate() {
                    let out = it["output_tokens"].as_i64().unwrap_or(0);
                    let model = it["model"].as_str().unwrap_or_default().to_string();
                    // Duplicate content-block lines repeat the array; the Ledger
                    // keeps the greatest output per slot, so expect that too.
                    want
                        .entry(format!("{base}#it{i}"))
                        .and_modify(|w| {
                            if out > w.1 {
                                *w = (model.clone(), out)
                            }
                        })
                        .or_insert((model, out));
                }
            }
        }
    }
    assert_eq!(want.len() as i64, records, "the Artifact and the Ledger disagree on how many per-call Records exist");
    for (key, (model, out)) in &want {
        let got: (String, i64) = conn
            .query_row(
                "SELECT model, output_tokens FROM events WHERE dedup_key = ?1",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap_or_else(|e| panic!("{key} is in the Artifact but not the Ledger: {e}"));
        assert_eq!(&got.0, model, "{key} booked the wrong Model");
        assert_eq!(got.1, *out, "{key} booked the wrong output_tokens");
    }

    // The partition still holds over the Records this fix introduced.
    crate::invariants::assert_partition_exact(&conn);
    crate::invariants::assert_secondary_subset(&conn);
}
