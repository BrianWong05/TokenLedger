# Context Breakdown — real attribution (v3)

Date: 2026-07-10
Status: approved (brainstormed with user; all forks user-decided)

## Problem

The v2 overview rewire deliberately dropped the Context Breakdown panel
(Messages / System prompt / Reasoning / Tool calls / Custom agents / MCP
servers / Skills) because its numbers were mock fractions, and real
content attribution was deferred as "a separate future project"
(2026-07-10-overview-real-data-design.md). This is that project: restore
the panel fed by real, estimated-from-logs attribution.

## Decisions (user-selected)

1. **Real attribution**, not a re-mounted mock and not a stripped-down
   partial panel.
2. **Billed-context semantics**: category numbers decompose cumulative
   billed context (`input + cache_read + cache_write_5m + cache_write_1h`
   — cache writes are context being transmitted; on a session's first
   call the system prompt arrives as `cache_creation` tokens, so
   excluding writes would blind the system-prompt heuristic), i.e. "what
   am I paying to re-send every turn". Messages will legitimately
   dominate (~99%).
3. **All four sources, best effort**: categories a source's logs cannot
   support are NULL and render "—" (never 0 — same honesty rule as
   Reasoning/Unpriced in v2).
4. **Layout**: Overview8b right column stacks Context Breakdown (top,
   its original 8b slot), then Token Breakdown, then Models.
5. **Storage**: nullable `ctx_*` columns on the events table (option A);
   rejected: a 1:1 side table (join for nothing) and parse-on-demand
   (re-reads GBs per range change, duplicates adapter logic).

## Schema (migration v2 → v3)

Seven nullable INTEGER columns on events; each is the event's attributed
share of billed context (`input_tokens + cache_read_tokens +
cache_write_5m_tokens + cache_write_1h_tokens` — cache writes are context
being transmitted; on a session's first call the system prompt arrives as
`cache_creation` tokens, so excluding writes would blind the system-prompt
heuristic), computed at scan time:

- **Primary — a partition**: `ctx_messages`, `ctx_system`,
  `ctx_reasoning`. When attribution exists they sum to billed context
  exactly (messages takes the rounding remainder).
- **Secondary — overlapping subsets of `ctx_messages`**:
  `ctx_toolcalls`, `ctx_agents`, `ctx_mcp`, `ctx_skills`. They do not
  sum to anything by design (matches the mock's muted sub-rows).
- NULL = source cannot attribute that category.

New tables:

- `ctx_resources(source, kind, name, day)` — PK (source, kind, name,
  day). Distinct resources observed per local day; kinds: `skill`,
  `mcp_server`, `agent`, `memory_file`. Powers the meta line
  ("32 skills · 2 MCP servers · 1 agent · 1 memory file") for any range
  via `COUNT(DISTINCT name)`.
- `session_ctx(session_id PRIMARY KEY, …running per-category char
  counters…, system_baseline_chars)` — **Claude only**; running
  composition must survive byte-offset resume. Codex and Gemini
  re-parse whole files and rebuild state in memory.

## Attribution algorithm

**Estimator:** tokens ≈ chars ÷ 4 of content text (tool inputs/results
JSON-serialized). Reported numbers always beat estimates (Gemini
`tokens.tool`). All panel values are estimates and are labeled so.

### Claude

Per session, maintain running char counters by category. Every line's
content (user text, assistant text, tool_use input, tool_result content,
thinking) is added to its category counter as lines stream by. At each
assistant API call, attribute: share of each category in the running
composition × that call's billed context (`input + cache_read +
cache_write_5m + cache_write_1h`).

- **System prompt** (not present in transcripts): heuristic — the
  session's first call's billed context minus the estimated chars of the
  content seen so far = system baseline, held constant in the composition
  thereafter. (The system prompt lands largely in `cache_creation` on
  that first call, which is why billed context includes cache writes.) UI
  keeps the ⓘ affordance: "estimated from each session's first call".
- **Reasoning: NULL.** Transcripts store thinking blocks as signature-only
  (the `thinking` text is always empty — verified across old and new logs),
  so reasoning-in-context is unobservable for Claude. `ctx_reasoning` is
  NULL, matching v2's `reasoning_tokens` convention, and renders "—". The
  engine still counts thinking text within its tool-use turn (reset at each
  user-turn boundary; all counters reset on `compact_boundary` lines) in
  case a future log format carries it.
- **Custom agents**: events in transcripts under `subagents/` (or lines
  with `isSidechain: true`) attribute their entire billed context to
  `ctx_agents`. In the parent session, agent output arriving as
  tool_results counts as tool calls only — no double bookkeeping.
- `ctx_mcp` = tool i/o where the tool name starts `mcp__` (server name =
  second `__`-delimited segment, recorded in ctx_resources);
  `ctx_skills` = the `Skill` tool (skill name from its args). Both are
  subsets of `ctx_toolcalls`. Memory files: presence of the memory
  directory read marker in system-reminder content is NOT parseable
  reliably — `memory_file` resources are recorded only when a
  tool_result path matches `**/memory/MEMORY.md` (best effort).

### Codex

Full re-parse per file (existing v1 behavior). Walk `response_item`
lines in order: `message`, `function_call`, and `function_call_output`
chars all feed the messages counter, with `function_call` /
`function_call_output` additionally feeding the toolcalls counter (so
`ctx_toolcalls` stays a subset of `ctx_messages`); `reasoning` →
reasoning (same per-turn reset rule). At each `token_count` event,
attribute the Δ(input + cached) by current shares. Shares normalize
over known content only, so the unattributable system prompt is
absorbed proportionally and the primary partition sums to billed input
by construction. `ctx_system`, `ctx_agents`, `ctx_mcp`, `ctx_skills` =
NULL.

### Gemini

Per token-bearing message: `ctx_messages` = `input + cached` (the whole
billed context — message history including tool content);
`ctx_toolcalls` = reported `tokens.tool`, a subset of `ctx_messages`
consistent with the schema's subset rule; `ctx_reasoning` = NULL
(`thoughts` are output-side, never re-sent as input); `ctx_system`,
`ctx_agents`, `ctx_mcp`, `ctx_skills` = NULL.

### Hermes

No content in `state.db` rows: all seven NULL.

## Queries & IPC

- `series` per-bucket per-source payload gains the seven summed `ctx_*`
  fields. SQLite `SUM` over all-NULL input yields NULL, which flows to
  the frontend as "—" with no extra logic. Frontend slices client-side
  as today; all ranges work unchanged.
- New small command `ctx_resources(range)` → `[{source, kind, count}]`
  (`COUNT(DISTINCT name)` per kind over days in range).

## Frontend

- `src/overview/ContextBreakdown.tsx` repurposed to real props (same
  pattern as the v2 TokenBreakdown/ModelsList refactors): header dot +
  "{Tool} Context Breakdown"; subtitle "Cache hit rate X% · reused /
  input (est.)" — cache figures are real (from token categories),
  attribution rows are estimates; three primary rows with faint
  proportion bars + percentages; divider; four muted secondary rows (no
  percentages); meta line from `ctx_resources`.
- NULL → "—". Hermes renders the header (cache hit is real) plus all-dash
  rows and a tooltip: "Hermes logs don't record content".
- Mounted in Overview8b right column: Context Breakdown, Token
  Breakdown, Models (in that order).
- `mock.ts` gains a tiny `mockCtxTotals()` adapter (mock fractions
  reshaped into the real `CtxTotals` prop) so the unmounted 8a FocusPanel
  keeps compiling against the repurposed ContextBreakdown — the same
  pattern v2 used for ModelsList (`mockModelBars`).

## Backfill & edge cases

- Migration v3 clears scan resume state (file offsets / mtimes and
  `session_ctx`) so the next scan re-parses all history and fills
  `ctx_*` on existing rows. Adapters' event insert becomes an upsert:
  on dedup-key conflict, update the `ctx_*` columns only. One-time full
  re-scan, same cost as first launch.
- Attribution is strictly best-effort on top of v1 parsing: a line whose
  content defies extraction contributes nothing to counters and never
  blocks the usage event.
- A Claude file resumed mid-session with no `session_ctx` row (state
  cleared out-of-band) gets NULL attribution for the remainder of that
  session rather than guessing from a partial window.
- A full re-parse from byte 0 always starts a fresh composition (persisted state is only for byte-offset resumes), so clearing scan state self-heals tainted or stale attribution.

## Testing

- `e2e_real_logs.rs` invariants over this machine's real logs:
  - attributed Claude events: `ctx_messages + ctx_system +
    ctx_reasoning == input + cache_read + cache_write_5m +
    cache_write_1h` (exact — messages absorbs the rounding remainder);
  - each secondary column ≤ `ctx_messages`;
  - Gemini `ctx_toolcalls` totals equal reported `tokens.tool` sums;
  - Hermes events: all seven columns NULL.
- `data.test.ts`: NULL → "—", percentage math, meta-line assembly,
  Hermes all-dash view-model.

## Out of scope

- Per-name drill-down (the mock's "›" expanders on rows — e.g. tokens
  per individual MCP server or skill). `ctx_resources` records names,
  so a future iteration can add per-name token sums without another
  migration of the events table.
- Context attribution for the old dashboard / 8a variant.
- Live "current context window" inspection; this panel reports
  historical billed context only.
