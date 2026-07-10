# Context Breakdown v2 — exact buckets + drill-downs (schema v4)

Date: 2026-07-10
Status: approved (brainstormed with user; all forks user-decided)
Reference: TokenTracker (github.com/mm7894215/TokenTracker), whose
`src/lib/claude-categorizer.js` was studied as the example. We adopt its
usage-field bucketing and largest-remainder allocation; we do NOT adopt
its parse-on-demand architecture or its `|| 1` empty-thinking trick.

## Problem

The v3 Context Breakdown estimates its primary rows (Messages / System
prompt) from content bytes, cannot split history from new input, and has
no drill-down. TokenTracker demonstrates a strictly more honest primary
split derived purely from usage fields, plus expandable per-category /
per-tool detail. This project upgrades the panel to that shape.

## Decisions (user-selected)

1. **Hybrid**: exact usage-field buckets become the primary rows; the v3
   estimated composition survives as (a) the estimated secondary section
   (Tool calls / Custom agents / MCP servers / Skills, unchanged
   semantics) and (b) a ⓘ tooltip on the Messages group row showing the
   old estimated content split. All v3 `ctx_*` columns and scan logic
   stay.
2. **Drill-down depth matches the example**: Messages → 3 sub-rows;
   Tool calls → categories → individual tools (two expansion levels).
3. **Architecture A**: exact buckets computed at query time from
   existing token columns (no new event columns); one new `ctx_tools`
   table filled during the existing scan; categories are a frontend map.
   Rejected: parse-on-demand (duplicates adapter parsing, re-reads GBs);
   categories-only (cannot answer "which MCP server is eating context").

## Exact primary buckets

Per source over the selected range, from existing token columns:

| Row | Definition | Exactness |
|---|---|---|
| Messages (group) | history + new input + response | exact |
| — Conversation history | `cache_read` + all cache writes EXCEPT each session's first cache-writing event | exact |
| — New input | fresh `input_tokens` | exact (label "New input", tooltip: "uncached input for the newest turn — user text and fresh tool results") |
| — Assistant response | `output − COALESCE(reasoning, 0)`, floored at 0 | exact |
| System prompt ⓘ | each session's FIRST cache-writing event's cache-write amount | exact usage-field version of the v3 heuristic; tooltip "first cache write of each session" |
| Reasoning | `reasoning_tokens` column | exact, REPORTED — real for Codex/Gemini/Hermes; NULL → "—" for Claude (thinking text absent from logs; we do not fabricate via TokenTracker's `\|\|1` trick) |

- **Partition invariant (exact):** history + newInput + system +
  response + reasoning == `input + cache_read + cache_write_5m +
  cache_write_1h + output` (total usage) per source, where NULL terms
  coalesce to 0 only in the invariant, never in display.
- **Percentages** on primary rows use total usage as denominator. The
  header keeps the existing cache-hit line (reused / billed input).
- **Per-source:** Codex/Gemini report no cache writes → System "—".
  Hermes stores one aggregated event per session, so first-vs-rest is
  meaningless → System "—", ALL its cache writes count as history;
  Hermes now shows real sub-rows instead of an all-dash panel.

## ctx_tools (schema v4)

- `ctx_tools(source TEXT, name TEXT, day TEXT, est_tokens INTEGER,
  calls INTEGER, PRIMARY KEY (source, name, day))`, day = local day.
- Filled during the existing scan: Claude accumulates est (bytes/4)
  content of each `tool_use` input and, via the existing
  tool_use_id→name map, each `tool_result`, keyed by tool name; Codex
  accumulates `function_call` / `function_call_output` payload bytes by
  function name; Gemini logs no tool names → no rows; Hermes none.
  `calls` counts tool_use / function_call occurrences.
- `est_tokens` is a WEIGHT (estimated unique content), not a display
  value.
- **Display allocation:** the estimated Tool calls row total is
  allocated to categories, and each category to its tools, proportional
  to these weights via largest-remainder integer allocation — children
  always sum exactly to their parent at both levels.
- Migration v4: `CREATE TABLE ctx_tools`, clear `scanned_files` +
  `session_ctx` (one-time full re-scan populates history; safe and
  self-healing since the full-reparse fix).

## Queries & IPC

- `ctx_buckets(filters)` → per-source `{ source, history, new_input,
  system, response, reasoning }` (all `Option<i64>`, camelCase over
  IPC). One pass over `events`: window-rank each session's
  cache-writing events by timestamp (rank 1 → system, rest → history);
  range/tool filters apply AFTER the window so sessions straddling the
  range attribute correctly (a first-cw event before the range means
  in-range cache writes are history — conservative, never inflates
  System). Hermes special-cased in the same query.
- `ctx_tools(filters)` → `[{ source, name, est_tokens, calls }]` summed
  over in-range days (same day-bounds convention as the existing
  `ctx_resources` query: `day >= strftime(start)`, `day <=
  strftime(end − 1s)`).
- `series` and the existing `ctx_resources` command are untouched.

## Frontend

- `ContextBreakdown.tsx` gains expandable rows: local
  `expanded: Set<string>` state, chevron affordance (mock's `›` style),
  indented sub-rows reusing existing row classNames (one new indent
  class at most). Structure:
  Messages ▸ (Conversation history / New input / Assistant response) ·
  System prompt ⓘ · Reasoning — divider — Tool calls ▸ (categories ▸
  tools) · Custom agents · MCP servers · Skills — meta line.
  Percentages only on exact primaries; estimated section keeps values
  only, labeled "est.".
- `data.ts` pure helpers: `categorizeTool(name)` — exact names `Task`
  and `Agent` → Agent; `TaskCreate|TaskUpdate|TaskGet|TaskList|
  TaskOutput|TaskStop` and `Todo*` → Task Mgmt; Read/Write/Edit/Glob →
  File Ops; Grep → Search; Bash → Execution; WebFetch/WebSearch → Web;
  `mcp__<server>__*` → "MCP: <server>"; Skill → Skill; else Other —
  plus `allocateByWeight(total, weights)` (largest-remainder),
  `toolTree(rows, toolcallsTotal)`, `bucketRows(...)`.
- Overview8b: two more fetches inside the existing per-range effect with
  the same stale guard. `mock.ts` adapter extended so the unmounted 8a
  FocusPanel keeps compiling (v3 precedent).

## Edge cases

- First cache-write predates local history (file deleted): surviving
  cache writes count as history; System under- rather than over-counts.
- `response = output − reasoning` floored at 0 (defensive).
- No in-range tool weights → Tool calls row shows its estimate without
  expansion; Gemini shows reported `tokens.tool` with no drill-down.
- NULL discipline unchanged: "—" everywhere a source cannot say.

## Testing

- `queries.rs`: first-cw window across sessions; session straddling the
  range; Hermes special case; exact bucket partition vs total usage;
  ctx_tools range sums.
- Adapter tests: per-name accumulation (Claude tool_result via id map;
  Codex function names); day bucketing.
- `data.test.ts`: categorizeTool map; largest-remainder exactness
  (children sum to parent, both levels); toolTree shape; NULL
  propagation.
- `e2e_real_logs.rs`: bucket-partition invariant on real logs; print top
  tools per category for an eyeball check.

## Out of scope

- Per-skill-name token drill-down (Skill row stays a single tool;
  `ctx_resources` already records skill names for the meta line).
- Output-side content split (assistant response vs tool-call JSON within
  output) — TokenTracker does this by char ratios; we keep response
  exact instead.
- Command-level Bash drill-down (TokenTracker's exec ledger).
