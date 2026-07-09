# Overview real-data wiring — design

Date: 2026-07-10
Status: approved for planning

## Goal

Replace the mock data behind the mounted "TokenTracker · Overview" view
(`src/overview/Overview8b.tsx`) with real Ledger data, and add the backend
capabilities the design needs but v1 lacks: per-source time series,
conversation counts, reasoning tokens, and the Cache-Estimated cost marker.

## Non-goals (deferred)

- **Insights / Models / Settings tabs** stay as dead nav buttons.
- **Status footer and price-override UI**: deferred to a future Settings-page
  design. The old dashboard (`src/App.tsx`, `src/components/`) and the 8a
  variant (`src/overview/Overview.tsx`) stay in the repo, unmounted.
- **Context-content attribution** (Messages / System prompt / Tool calls /
  MCP / Skills): would require parsing transcript content — a separate future
  project. The panel is repurposed (see Frontend).

## Domain notes

Terms per CONTEXT.md. Two additions to its rules, established here:

- **Reasoning tokens** are a *display subset of Output Tokens*. They are never
  priced separately and never added to totals. NULL means "this source does
  not report reasoning" (Claude); 0 means "reported zero". NULL is surfaced as
  "—", never as 0 — same honesty rule as Unpriced.
- **Convs** = conversation count = `COUNT(DISTINCT session_id)` within a
  bucket/range. Shown per table row only; never summed across days in the UI,
  because a Session spanning two days would double-count.
- **Cache-Estimated** (CONTEXT.md): a Model priced for input and output whose
  cache tokens have no rate. Flagged per-Model; does not turn the view total
  into a ≥ Partial Cost.

## Backend (Rust)

### Schema migration v1 → v2

Two new nullable columns on `usage_events`:

- `session_id TEXT` — NULL when unknown (pre-migration rows whose logs were
  pruned).
- `reasoning_tokens INTEGER` — NULL = source doesn't report; subset of
  `output_tokens` when present.

Backfill: bump the schema version, reset scan state (file offsets / mtimes /
sizes) so the next scan re-parses every log in full, and switch event inserts
to `ON CONFLICT DO UPDATE` on the two new columns so existing rows are filled
in by dedup key. Token counts must be provably unchanged by migration +
re-scan (the app is 3 days old, so backfill should be near-total; unreachable
rows keep NULL).

### Adapter additions

| Source | session_id | reasoning_tokens |
|---|---|---|
| Claude Code | per-line `sessionId` | NULL (not reported separately) |
| Codex CLI | rollout file stem | Δ`reasoning_output_tokens` between snapshots, same delta rule as other fields; NULL if the field is absent in real logs |
| Gemini CLI | `sessionId` | `thoughts` |
| Hermes | `session_id` | `reasoning_tokens` |

### New IPC command: `series`

`series(filters, bucket: 'day' | 'hour')` → rows grouped by (bucket, source),
local-time bucketing reusing the existing trend logic:

```
{ bucket, source, inputTokens, outputTokens, cacheReadTokens,
  cacheWriteTokens, totalTokens, reasoningTokens (nullable), cost,
  requests, convs }
```

`cost` is priced-tokens-only per (bucket, source), same per-model rate
resolution as `trend`. This command is the real-data twin of the mock's
`DAYS` array: one unbounded `series(day)` call powers the heatmap, stacked
trend, sparklines, tool cards, token breakdown, and daily table via
client-side slicing.

### Extensions to v1 queries

- `summary` gains `cacheEstimatedModels: string[]` — models in range that are
  priced for input+output but have no cache rate *and* nonzero cache tokens.
- `breakdown(by='model')` rows gain `source`, `cacheEstimated`,
  `reasoningTokens`, `convs`. Grouping becomes (model, source). One call
  powers the models list, per-tool model counts on tool cards, and
  cache-estimated flags.
- `breakdown(by='project')` rows gain `reasoningTokens`, `convs` for the
  Project Usage table.

## Frontend

### Data layer

New `src/overview/data.ts`. On mount: auto-`scan()` (as the old App did),
then one unbounded `series(day)` fetch. Re-fetched on range/tool change only:
`series(hour)` for the Day view, `summary` for cost, `breakdown` (model,
project) for the right column and table.

Range → Filters mapping: Day = today (local), Week = last 7 days, Month =
last 30 days, Total = all time, Custom = date pickers bounded by the
Ledger's first/last event dates.

`Overview8b.tsx` stops importing `mock.ts`; `mock.ts` remains only as the 8a
variant's dependency. Shared formatters (`fmtTok`, `fmtUSD`, `fmtPct`,
`fmtDate`) move to one shared module used by both.

### Per-panel mapping

- **Head**: real range total and `summary.cost`. When `hasUnpriced`, the cost
  renders as `≥ $X` with the unpriced-model count (Partial Cost rule). When
  `cost` is null (zero priced tokens in range), render "unpriced" — never
  $0.00.
- **Context panel → "Token breakdown"**: per selected tool — Input / Output /
  Cache read / Cache write bars plus Cache Hit Rate, from the series slice.
  The speculative context-content rows are removed.
- **Models list**: real models for the selected tool with token share; a
  small "cache est." tag on Cache-Estimated models.
- **Table (Daily / Project tabs)**: real rows with Total · Input · Output ·
  Cached · Reasoning · Convs. Reasoning sums available values; renders "—"
  when every contributing source is NULL, with a header tooltip noting Claude
  doesn't report reasoning separately. Convs never totalled across rows.
- **Heatmap**: trailing 365 days ending today, for every range. Intensity
  levels from the real distribution (quartiles of nonzero days), not fixed
  thresholds.
- **Tool cards / split bar / stacked trend / sparklines**: real per-source
  slices of `series(day)` (or `series(hour)` on Day).

### Edge cases and errors

- Empty Ledger or empty range → zeroed panels; components already guard
  empty arrays.
- Scan failure → keep rendering last-fetched data; one small inline error
  line (per-source failures are already isolated in the backend).
- First load → skeleton/dimmed state until the initial series arrives.

## Testing

- Rust units: migration (v1 DB → v2, token counts identical), `series`
  aggregation and distinct-convs fixtures, cache-estimated detection,
  adapter fixtures asserting session_id/reasoning and reasoning ≤ output.
- Invariant: `series(day)` summed over a range equals `summary` for the same
  filters.
- Re-run the e2e real-log harness: Claude totals still match ccusage <0.5%.
- Frontend: vitest for range→filter mapping and data reshaping; final visual
  sign-off in `npm run tauri dev` against the real Ledger.

## Decisions log

1. Scope: wire Overview to real data **and** close backend gaps (series,
   convs, reasoning, Cache-Estimated).
2. Context Breakdown panel → replaced by a real per-tool token breakdown;
   transcript-content parsing is a future project.
3. Status footer / override UI → deferred to a future Settings design; old
   components left in place, unmounted.
4. Data flow: new `series` command + extended v1 queries; frontend slices
   client-side like the mock did. Rejected: monolithic `overview` command
   (bespoke, duplicates SQL), whole-ledger dump (pricing/convs still need
   backend, payload grows forever).
