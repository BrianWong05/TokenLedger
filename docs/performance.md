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
| 30-day range reload | ≤ 100 ms | The eight serialized panel queries after a range change |

The range-reload budget is deliberately an order of magnitude tighter than the
series one: it is paid on every period switch, an interaction a person repeats,
and it is the budget a per-query whole-table scan breaks first.

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
| Synthetic 30-day range reload (8 serialized queries) | — | 131.0 ms | Passes the 1,000 ms standard of the day |

The real-ledger comparison uses the old raw SQL aggregation as its diagnostic
baseline and the new production `series` query as the result, so the synthetic
same-harness comparison is the canonical regression number.

The root cause was SQLite's built-in `strftime(..., 'localtime')` conversion
being invoked for every row on macOS. Replacing it with a custom SQLite
scalar function backed by Chrono's cached local time-zone data preserves the
existing calendar buckets while removing repeated operating-system time-zone
lookups.

## Validated result — range switches (2026-08-10)

Same machine and release profile as above. This pass targeted the period-tab
switch (Day / Week / Month / Custom), which pays the 30-day range reload.

| Workload | Before | After | Improvement |
| --- | ---: | ---: | ---: |
| Synthetic 100,000-event 30-day range reload (same harness) | 153.5 ms | 23.4 ms | 84.8% lower, 6.6x faster |
| Real 82,133-event reload, Day window | 177.8 ms | 73.3 ms | 58.8% lower |
| Real 82,133-event reload, Week window | 172.4 ms | 70.6 ms | 59.1% lower |
| Real 82,133-event reload, Month window | 305.4 ms | 197.7 ms | 35.3% lower |
| Real 82,133-event reload, Total window | 396.1 ms | 267.6 ms | 32.4% lower |
| Real 82,133-event reload, Custom 30-day window | 310.3 ms | 207.6 ms | 33.1% lower |

The synthetic same-harness row is the canonical regression number; the
real-ledger rows come from a read-only snapshot and are reported per window
because the dominant cost differs between them. They were measured with a
temporary per-query harness, not the committed benchmark, which reports the
synthetic figures only.

Three causes, in order of size:

1. `queries::ctx_buckets` ranked the whole `events` table with a window
   function before the range filter applied, so it cost 110–155 ms whichever
   window was selected. It now finds each Session's first cache write through
   the `idx_events_first_cw` partial index (schema v13) and joins that against
   the windowed aggregate.
2. The Ledger read commands were synchronous Tauri commands, which execute on
   the main thread, so each reload stalled the event loop for its whole
   duration. They are now declared `#[tauri::command(async)]`.
3. Debug builds compiled bundled SQLite at `opt-level = 0`, multiplying every
   query's cost under `tauri dev`. A `[profile.dev.package.libsqlite3-sys]`
   override compiles it optimized.

The remaining cost is honest aggregation: `summary` and each `breakdown` run
two full-window passes, the second only to count distinct Sessions. Merging
them into one pass at `(group, source, session)` grain measured about 45%
cheaper and is the next lever if these windows need to get faster.

## Validated result — startup first paint (2026-08-13)

Same machine and release profile as above. This pass targeted the launch: the
Overview used to render zeros until the launch scan, the unbounded series, and
the window reload had completed **in sequence**, with every read queued behind
the one connection mutex the scan holds for its whole pass.

This change is structural rather than a per-query win: the scan term is
removed from the first paint entirely. The store paints the persisted Ledger
immediately (series + Profile + window reload — the range-switch figures
above) while the scan runs on the write connection; reads moved to a second
WAL connection (`read_db`), so they cannot queue behind it.

| Workload | Measured |
| --- | ---: |
| Launch-shaped scan, real roots + 94,412-event live snapshot, nothing new accrued | 274 ms cold file cache / 60 ms warm |
| First paint (unbounded series + Profile + window reload, real snapshot) | the range-switch figures above; scan no longer a term |

The scan term the paint no longer waits for grows with accrued logs and cold
caches (a full-history cold scan is in the tens of seconds in a debug build),
which is exactly why it was the wrong thing to put in front of the paint.

Deliberate cost: boot now runs the series + Profile + window fan-out twice —
once provisionally before the scan settles, once as the post-scan reconcile,
because a pre-scan read must never be mistaken for post-scan truth
(zero-insert ≠ unchanged). The second pass is background work behind painted
figures, roughly 0.3–0.7 s of query time on the real Ledger, paid once per
launch. `Overview.test.tsx` pins the fan-out at exactly two window Summaries
per boot so a third pass cannot creep in unmeasured; the committed `npm run
perf` budgets are unaffected because the benchmark measures the queries, not
the orchestration.
