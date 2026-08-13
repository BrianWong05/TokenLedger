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

## Follow-up — the provisional fan-out actually paints (2026-08-14)

The pass above moved the scan off the first paint, but the provisional window
reload it issued never reached the screen: the post-scan reconcile bumps the
reload epoch in the microtask that follows the paint, so those nine queries were
always issued and always discarded. The headline therefore sat zero-shaped, with
`…` for cost and `—` for every Context row, until the SECOND fan-out landed,
having already paid for the same figures once.

`prices-rebuilt` lands in the same stretch (~1.0–1.2 s, right after the launch
catalog refresh clears the write lock the scan holds) and is a third potential
bump, but measurement shows it never adds a third fan-out: it arrives either
before the first series has landed, where `scheduleReload` drops it outright, or
inside the provisional reload's own debounce, where it just re-arms that timer.
Two fan-outs per boot is what the launch actually runs.

`land()` now takes a superseded response while no window-scoped figures have
landed yet, so the first fan-out to answer is the one on screen and each later
pass reconciles in place behind it. A superseded response is painted but never
cached, so it cannot be replayed for its window later.

Measured on the real Ledger — 95,235 events, 80 MB + 18 MB WAL, 13 Sources.
Release build, launched twice back to back with only the frontend bundle
differing, page cache warm and the scan finding nothing new (258–332 ms), so the
scan is not a term in either column:

| Launch milestone | Before | After |
| --- | ---: | ---: |
| Source cards paint (unbounded series) | 1,140 ms | 1,002 ms |
| Total-tokens headline reveals | 2,678 ms | 1,380 ms |
| Cost line and Context Breakdown fill | 2,678 ms | 1,857 ms |
| Window figures settled (post-scan reconcile) | 2,678 ms | 2,334 ms |
| Window fan-outs run / discarded | 2 / 1 | 2 / 0 |

The cards-paint difference is run-to-run variance in the series query (218 ms vs
156 ms), not an effect of the change. The headline's 1,298 ms is the change.

What the launch was actually waiting on is visible in the per-read lock-wait
times: the nine reads of a fan-out are issued together from the frontend and
then serialize on the single `read_db` mutex, so the last one waits 400–840 ms
behind its siblings for ~390–440 ms of total query time. Discarding a whole
fan-out therefore cost about half a second of nothing. Parallelising the fan-out
across more than one read connection is the next lever if this needs to get
faster; it was not needed to fix the placeholder.

Painting pre-scan figures is a claim about the screen, not about everything
downstream of it, so the snapshot now carries `provisional` and two consumers
read it:

- The Total-tokens entrance reel (#14 wants the first *authoritative* nonzero
  figure) waits for it to clear. It clears on the post-scan **series** refetch —
  one query pair after the scan, not the ten-query window fan-out — so the
  headline reveals in roughly half a second instead of waiting on the Summary,
  and the reel never rolls a figure the reconcile would then correct with no
  motion (#12 story 9 holds a same-window change still).
- Export waits for it too, on the same reasoning its `reloading` gate already
  carried: the screen wears a pre-scan figure for half a second and corrects
  itself, where a file would state it forever.

Deliberate cost: boot now runs the series + Profile + window fan-out twice —
once provisionally before the scan settles, once as the post-scan reconcile,
because a pre-scan read must never be mistaken for post-scan truth
(zero-insert ≠ unchanged). The second pass is background work behind painted
figures, roughly 0.3–0.7 s of query time on the real Ledger, paid once per
launch. `Overview.test.tsx` pins the fan-out at exactly two window Summaries
per boot so a third pass cannot creep in unmeasured; the committed `npm run
perf` budgets are unaffected because the benchmark measures the queries, not
the orchestration.
