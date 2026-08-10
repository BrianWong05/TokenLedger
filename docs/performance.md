# Performance standard

TokenLedger's performance gate targets the Overview data path because it is the
largest synchronous workload behind the app's main interaction. Run it with:

```bash
npm run perf
```

The command builds an optimized Rust test, creates a deterministic 100,000-record
synthetic Ledger, and checks two cold reads on the production query code:

| Workload | Budget | Why it matters |
| --- | ---: | --- |
| Unbounded daily series | ≤ 1,000 ms | First-load Activity, Profile, and trend data |
| 30-day range reload | ≤ 1,000 ms | The eight serialized panel queries after a range change |

The benchmark is ignored by the normal test suite because seeding 100,000 rows
is intentionally heavier than a unit test. It contains no private Source
Artifacts and prints only record counts, result-row counts, and elapsed time.

For a local, read-only check against an existing Ledger, provide its path:

```bash
TOKENLEDGER_PERF_DB=/path/to/tokenledger.db npm run perf
```

This optional check prints only the number of Usage Records, number of daily
series points, and elapsed time. It does not print models, projects, Sessions,
paths, or content.

## Baseline protocol

- Use the same machine, release profile, dataset, command, and time zone for
  before/after comparisons.
- Close unrelated CPU-heavy work when collecting reportable numbers.
- Record the first measured read after the synthetic Ledger is seeded; this is
  deliberately a cold user-facing load, not a warmed microbenchmark median.
- Treat a budget failure as a regression even if functional tests still pass.

## Validated result (2026-08-10)

Measured on Apple Silicon macOS in a release build. The optional real-ledger
benchmark opens the database read-only and prints aggregate timings only.

| Workload | Before | After | Improvement |
| --- | ---: | ---: | ---: |
| Synthetic 100,000-event daily series (same harness) | 13,823.4 ms | 215.0 ms | 98.4% lower, 64.3x faster |
| Real 81,896-event daily series read path | about 11.0 s | 232.9 ms | about 97.9% lower, 47.1x faster |
| Synthetic 30-day range reload (8 serialized queries) | — | 131.0 ms | Passes the 1,000 ms standard |

The real-ledger comparison uses the old raw SQL aggregation as its diagnostic
baseline and the new production `series` query as the result, so the synthetic
same-harness comparison is the canonical regression number.

The root cause was SQLite's built-in `strftime(..., 'localtime')` conversion
being invoked for every row on macOS. Replacing it with a custom SQLite
scalar function backed by Chrono's cached local time-zone data preserves the
existing calendar buckets while removing repeated operating-system time-zone
lookups.
