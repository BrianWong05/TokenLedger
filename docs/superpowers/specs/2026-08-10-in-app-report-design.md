# In-App Window Report Design

## Goal

Export the Overview's current window as one CSV report, so the figures the app
holds — tokens by category, Cost, per-Source, per-Model and per-Project
breakdowns, and the Context breakdown — can leave the app without leaving the
app's honesty behind.

Today the only file the app writes is one Trend bucket's per-Model split
(`TrendModal.tsx:518`, `save_csv` in `lib.rs:419`). A month of usage is not
reachable from the UI at all.

## Scope

One Export control in the Overview toolbar, reporting whatever window the range
picker is on. "The last 30 days" is the existing **Month** preset followed by
Export; no second date picker is introduced, because a second place to choose a
range is a second place for the two to disagree.

Out of scope: a report screen, scheduling, PDF, a preview before saving, and any
change to the Trend bucket export. The `report::ledger_report` cargo workflow
stays as the headless path (see *Relationship to report.rs*).

## Approaches considered

1. **Frontend assembles, existing save seam (selected).** A pure serializer over
   the state the Overview already renders, handed to the existing
   `ExportPort.saveCsv` and `save_csv` command. No Rust changes. Nothing is
   refetched, so the file cannot disagree with the screen it was taken from, and
   the serializer is testable without Tauri exactly as `bucketCsv` is.
2. **Rust assembles, a new `save_report` command.** One producer for both the
   app and the cargo workflow, but it re-runs queries the frontend has already
   run, and it cannot reach the frontend-only derivations — `toolTree` and
   `mcpBars` allocate a Context category total proportionally across raw
   `ctx_tools` rows. Rust holds the rows, not the allocation, so this exports
   Context figures that differ from the panel unless the allocation is ported.
3. **Rust returns rows, frontend serializes.** Approach 2's refetch plus
   approach 1's serializer, for neither's benefit: the rows are already in the
   store.

Approach 1 duplicates no estimate and adds no command.

## Components

**`src/overview/reportCsv.ts` (new).** `windowReportCsv(input: ReportInput):
string` and `reportFilename(from, to): string`. Pure — a plain input object in,
a string out, with no React, Tauri or store import. It is a new file rather than
an addition to `data.ts` because `data.ts` is already 1067 lines and owns
range/bucket/derivation math; serializing is a separate job with a separate
consumer.

**`src/overview/overviewStore.ts`.** Two changes:

- Add `breakdown('tool', filters)` as a ninth call to the window load
  (currently `:344-352`), kept as `sourceRows`. Per-Source usage figures are the
  only thing the store does not already hold.
- Add a `reportInput()` selector. Its one piece of real work: the Context
  derivations (`ctxTotals`, `toolTree`, `mcpBars`, `skillBars`) run once for
  `s.selected` at `:433-466`; the selector runs the same functions over each
  Source **present in the window** that reports Context, tagging every row with
  its Source. A Source with usage in the window but no Context capability
  contributes no rows; a Source absent from the window is not iterated at all.

**`src/overview/Overview.tsx`.** An Export button in `tt-toolbar` beside
Rescan, following the Decrypt button's busy pattern (`:226-238`).

**`src/lib/strings/overview.ts`.** `overview.export`, `overview.exporting` and
`overview.exportFailed` in `en` and `zh-Hant`.

**`README.md`.** The `report::ledger_report` section is narrowed to name it the
headless path (see *Relationship to report.rs*). This is the only file changed
outside `src/`.

**No Rust changes.** `save_csv` and `ExportPort` are used as they stand.

## Data flow

The user selects a window; the store has already loaded it. Export snapshots the
rendered state through `reportInput()`, serializes it with `windowReportCsv()`,
and passes the result to `saveCsv(filename, contents)`, which opens the native
dialog and writes. The promise resolves `true` written or `false` cancelled.

Nothing is refetched. A scan landing mid-export cannot produce a file matching
neither the screen before it nor the screen after.

## Report shape

One rule governs every cell: **an unknown is empty, never `0`.** This is the CSV
form of the rule the UI already follows — Unpriced is never `$0`, an
unattributable Context category is "—" and not zero — and it keeps a
spreadsheet's `SUM()` from absorbing a gap.

Blocks are separated by a blank line and identified by their first column; there
is no separate section marker.

### Header block

`key,value` lines, so the file explains itself:

```
tokenledger_report,1
generated,2026-08-10T23:35:12+10:00
window,2026-07-12,2026-08-10
window_grain,day
tokens_basis,exact
currency,USD
display_currency,AUD
display_rate,1.52
```

`tokens_basis` is `exact` or `floor`, by the ADR-0017 rule already implemented in
`unreadableSourcesIn` (`src/lib/tokenCompleteness.ts`), which this reuses rather
than restates. `display_currency` and `display_rate` appear only when a Display
Currency is set; every figure below stays USD regardless, because a file outlives
the user-editable rate that would otherwise define it. `window_grain` names the
grain of the file's own rows — the same value as the time block's first column,
by construction — not the Trend's display bucket. A report carries the finest
honest grain it holds and lets the spreadsheet roll it up, so the chart's
aggregation to weeks or months is never exported: pre-rolling here would destroy
detail the file otherwise keeps. For a `Total` window the dates come from the
Ledger's own extent (`firstIso`, `lastIso`).

### Usage blocks

| Block | First column | Notes |
|---|---|---|
| summary | `window` | plus `unpriced_models`, `cache_estimated_models`, space-joined |
| time | `hour` \| `day` | ascending; `hour` only for a single-day window whose hours have landed, `day` otherwise |
| by Source | `source` | total tokens descending |
| by Model | `model` | plus a `source` column: a Model is scoped to the tool that ran it |
| by Project | `project` | quoted; paths contain commas |

All five share: `input_tokens`, `output_tokens`, `cache_read_tokens`,
`cache_write_tokens`, `total_tokens`, `requests`, `sessions`, `cache_hit_rate`,
`cost_usd`, `cost_basis`, `unattributed_tokens`, `cache_estimated` — except the
time block, which omits `sessions`. See below.

`cache_hit_rate` is not on `BreakdownRow`, so the serializer computes it per row
as `cache_read / (input + cache_read + cache_write)` — well defined in every
block precisely because Input excludes cache reads (ADR-0001).

`cost_basis` is `exact`, `partial` or `unavailable`, folding `has_unpriced` and
`unattributed_tokens` into one word so a Partial Cost is never read as a total.

`sessions` is distinct Sessions at that row's own grain, as
`queries::breakdown` counts them; a Session spanning two Models counts in both,
so the column does not sum to the summary's figure. This matches what the app
displays.

**The time block omits it.** The non-additivity above holds in every block, but
only the time block is a sequence the file explicitly invites the reader to roll
up — that is why it carries the finest grain it holds rather than the chart's
aggregate. A column that must not be summed has no place in the one block whose
stated contract is that summing it is correct: a Session open across midnight
would be counted in each day it touches, and the total would silently exceed the
summary's. The four whole-window blocks keep the column, because there the rows
are categories and no roll-up is implied.

#### The time block's cost is a weaker signal than the others

Every other block reads Cost from a `Summary` or `BreakdownRow`, where
`cost: Option<f64>` distinguishes "no priced tokens" from "priced tokens worth
zero". The time block reads `SeriesPoint`, whose `cost` is a plain number with a
separate `hasUnpriced` flag — the distinction is not in the data.

Rather than refetch a `summary` per bucket, which would break the property this
design is built on, the serializer resolves it conservatively: when `hasUnpriced`
is set and `cost` is `0`, the row writes an **empty** `cost_usd` and a
`cost_basis` of `unavailable`.

The cost of that choice, stated so it is not discovered later: a bucket whose
priced Models genuinely total `$0` *and* which also holds an Unpriced Model reads
`unavailable` where the other blocks would say `partial`. The error runs toward
admitting ignorance rather than reporting a `0` that means "unknown", which is
the rule the rest of the file follows.

The block aggregates `SeriesPoint`s across Sources per bucket, as the Trend chart
already does.

### Context blocks

Each carries a `source` column, with every reporting Source stacked, so the file
does not depend on which Source card happened to be selected.

| Block | First column | Columns |
|---|---|---|
| categories | `context` | `source, est_tokens, basis` — the first column holds the category |
| tools | `tool` | `source, category, est_tokens, calls` |
| MCP | `mcp_server` | `source, est_tokens, calls` |
| skills | `skill` | `source, est_tokens, uses` |
| Bash | `bash` | `source, exe, est_tokens, calls` — the first column holds `cmd`, already the two-word signature |

`category` takes one of seven values, from `ctxTotals`: `messages`, `system` and
`reasoning`, then `toolcalls`, `agents`, `mcp` and `skills`. `basis` is `exact`
for the first three, which partition the billed total, and `estimated` for the
last four, which are overlapping subsets of messages.

**No Context block emits a total row.** Those four subsets overlap and do not sum
to a whole; a total would invite exactly the reading the app declines to offer.
A category a Source cannot attribute is omitted, not zeroed. `uses` counts
injections rather than distinct skills — a re-invoked skill reloads its whole
body, which is what makes `est_tokens` grow.

**`est_tokens` is the allocated figure the panel shows, never the raw stored
weight.** `ctx_tools` and `ctx_exec` store content sizes; `toolTree`, `mcpBars`
and the Bash drill-down each spread a Context total across those sizes, so a
block written from the raw rows would sit on a different scale from the panel it
was taken from — the failure this design cites to rule out assembling in Rust.
The tools and Bash blocks therefore resolve the same Bash leaf the drill-down
expands, and a Source that attributes no Tool-calls total contributes no rows,
exactly as it renders no drill-down.

**The Bash block merges rows sharing a signature.** `ctx_exec` groups by `kind`
as well, and `kind` reads the raw command line while `cmd` is the two-word
signature — `npm run build` and `npm run dev` are two stored rows that both
reduce to `npm run`. The block carries no `kind` column, so unmerged they would
be two rows under one key with nothing to tell them apart.

Only Claude, Codex and pi report Context; the blocks are absent when no Source in
the window does.

### Naming

Filename `usage-<from>_<to>.csv`, extending the existing `usage-<key>.csv`
convention.

Column and block names stay English and untranslated, following the stated
policy in `src/lib/strings/overview.ts` that puts Model names, ISO codes and file
paths outside translation. They are machine-facing identifiers, and a report
whose columns rename themselves with the UI language cannot be scripted against.
Only the button label, its busy label and its error message are localized.

## Error handling

Cancelling is not a failure: `saveCsv` resolving `false` is silent.

A rejected write means the dialog or the filesystem failed. The Overview already
renders `scanError || fetchError` in its `tt-error` band (`:209-213`);
`exportError` joins them there, cleared on the next export or window change. No
toast infrastructure is introduced.

`exporting` disables the button with `aria-busy`. The button is also disabled
while the window is loading, since there is nothing to serialize. It is **not**
disabled on an empty window: a window with no usage is a legitimate report
costing `$0.00`, the one zero that is a figure rather than a gap.

Assembly cannot fail — it is a pure function over state that has already
rendered.

The Trend bucket export's swallowed write error (`TrendModal.tsx:520`) is left
as it stands. It sits in a modal with no error band, and it is not code this
change touches.

## Testing

`reportCsv.test.ts` pins the honesty rules, pure and Tauri-free:

- an Unpriced Model leaves `cost_usd` empty with `cost_basis` `unavailable`
- Unattributed Usage makes `cost_basis` `partial` while its tokens still count
- a Source that cannot attribute a Context category yields no row, not a zero
- `cache_hit_rate` derives correctly per row, including an all-cache-read row
- Project paths containing commas and quotes survive a round-trip
- no Context block emits a total row, and `basis` is `exact` for messages,
  system and reasoning and `estimated` for the four overlapping subsets
- `display_currency` and `display_rate` appear only when set, and `cost_usd`
  stays USD either way
- `tokens_basis` is `floor` when an unreadable Artifact's mtime is at or after
  the window start
- the grain line reads `hour` or `day` — `hour` only for a single-day window
  whose hours have landed, `day` for every other window, however long
- a `Total` window takes its filename dates from the Ledger's extent
- a time-block bucket with `hasUnpriced` and `cost === 0` writes an empty
  `cost_usd` with `cost_basis` `unavailable`, not `0`

`Overview.test.tsx`, which already injects `ports.export`: clicking Export calls
the port with `usage-<from>_<to>.csv`; a `false` resolution shows no error; a
rejection shows the error band; the button is disabled while loading.

`overviewStore.test.ts`: the window load issues the ninth `breakdown('tool')`
call, and `reportInput()` derives Context for every reporting Source rather than
only `selected`.

One test carries the architecture: on the same fake Ledger, the CSV's summary
`total_tokens` and `cost_usd` equal what the rendered headline shows. That makes
"the file cannot disagree with the screen" fail the build when it stops being
true, rather than remaining a claim about the design.

## Relationship to report.rs

`report::ledger_report` stays. It becomes the headless path — cron, CI, no GUI —
and its README section is narrowed to say so, since the way to get a report is
now the button.

The two produce different layouts on purpose: one shareable file, one folder of
five per-table CSVs. Their figures cannot drift, because both resolve Cost
through `queries::summary` and `queries::breakdown`. Only layout can, and the
layouts differ by design.

Making the shapes match was rejected: it would require porting `toolTree` and
`mcpBars` into Rust, duplicating an estimate — the worst kind to keep two of.
