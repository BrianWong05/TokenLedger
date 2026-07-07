# TokenLedger — AI Usage Tracker Design

**Date:** 2026-07-07
**Status:** Approved by user; revised after adversarial spec review (33-agent verification pass against real log data on this machine)
**Repo:** git@github.com:BrianWong05/TokenLedger.git

## Overview

TokenLedger is a macOS desktop app (Tauri v2) that tracks token usage and
estimated cost across the AI coding tools on this machine: Claude Code, Codex
CLI, Gemini CLI, and Hermes. It parses each tool's local logs into a normalized
SQLite ledger and shows a dark-themed dashboard: totals, estimated cost, cache
hit rate, a daily trend chart, and breakdowns by tool, model, project, and date
range.

## Goals

- Zero-effort tracking: parse local logs automatically; no manual entry.
- Token detail per event: input, output, cache write, cache read.
- Estimated cost from public API list prices (LiteLLM pricing database), with
  user-editable price overrides for self-hosted models.
- Fast: incremental scanning; instant queries after first scan.
- Works fully offline (pricing falls back to a bundled snapshot).
- Durable history: the database is a permanent ledger that outlives the source
  logs (Claude Code deletes transcripts after ~30 days by default).

## Non-goals (v1)

- Provider billing/usage API polling (no API keys, no real-billing overlay).
- Windows/Linux support.
- Tracking tools other than the four above.
- Live file watchers (a periodic incremental re-scan is sufficient).
- Gemini's >200k-context tiered pricing (measured ~$14 impact across 46 calls
  on this machine; computable later at query time from
  input + cache_read, so no schema change is needed to add it).
- Multi-select tool/model filters (IPC arrays keep the door open).

## Architecture

Tauri v2 app with two halves:

- **Rust core**: four source adapters parse logs incrementally into a SQLite
  database (via `rusqlite`) in the app data directory. Query commands aggregate
  it. Rust surface stays small: adapters + queries only.
- **React dashboard** (Vite + TypeScript + Recharts): one screen, calls the
  Rust core over Tauri IPC.

### IPC commands

| Command | Purpose |
|---|---|
| `scan()` | Run an incremental scan of all four sources; returns per-source status |
| `summary(filters)` | Totals: tokens by category, requests, est. cost, cache hit rate, unpriced-model list |
| `trend(filters, bucket)` | Time series (daily; hourly when range = today) of tokens + cost |
| `breakdown(by, filters)` | Grouped totals, `by` ∈ {tool, model, project} |
| `set_price_override(model, rates)` / `delete_price_override(model)` | Manage user price overrides |

`filters` = tools[], models[], project, date range. All aggregation happens in
SQL; the frontend only renders.

### Scan execution model

`scan()` is a Tauri async command. Concurrent calls (timer tick or manual
refresh while a scan runs) coalesce via a scan lock rather than starting a
second scan. The dashboard renders immediately from existing data and shows a
"scanning…" state in the status footer, re-querying when `scan()` resolves. The
app database is opened in WAL mode with a busy timeout. v1 uses a single
serialized connection, so a `summary`/`trend`/`breakdown` read briefly waits
behind an in-flight scan — acceptable because incremental scans are cheap
(milliseconds; a `stat()` sweep plus a few appended lines) and the only slow
scan is the first full ingest, when there is no prior data to display anyway.
(A dedicated read connection would let reads run fully concurrently under WAL;
deferred until a scan is ever slow enough to matter.)

### Refresh model

Scan on launch, then on a user-configurable timer (off / 30s / 60s; default
30s). Incremental scans are cheap, so no file watchers.

## Data model (SQLite)

The database is a **ledger (system of record), not a rebuildable cache**.
Verified on this machine: Claude Code prunes transcripts after ~30 days
(default `cleanupPeriodDays`), so once ingested, the DB is the only holder of
older history. Rules:

- Events are permanent. A source file disappearing never deletes its events
  (Gemini's replace-per-file fires only when a file *changes*, not vanishes).
- `scanned_files` rows for missing paths may be pruned; events may not.
- `PRAGMA user_version = 1` at creation; schema migrations are in-place
  (sequential `ALTER` blocks keyed on `user_version`) — never drop-and-rescan.
- The "All" date range means all ingested history.

> Retention note (informational): Claude Code history before ~2026-06-04 was
> already deleted before TokenLedger existed and is unrecoverable. Raising
> `cleanupPeriodDays` in `~/.claude/settings.json` extends retention forward.

```sql
events (
  dedup_key   TEXT PRIMARY KEY,   -- source-specific, see adapters
  source      TEXT NOT NULL,      -- claude | codex | gemini | hermes
  timestamp   INTEGER NOT NULL,   -- unix epoch seconds, UTC (truncate fractions)
  model       TEXT NOT NULL,      -- raw model string as logged
  project     TEXT,               -- ABSOLUTE PATH of the repo/directory
  api_calls   INTEGER NOT NULL DEFAULT 1,
  input_tokens          INTEGER NOT NULL DEFAULT 0,  -- EXCLUDES cache reads
  output_tokens         INTEGER NOT NULL DEFAULT 0,  -- includes reasoning/thoughts
  cache_read_tokens     INTEGER NOT NULL DEFAULT 0,
  cache_write_5m_tokens INTEGER NOT NULL DEFAULT 0,
  cache_write_1h_tokens INTEGER NOT NULL DEFAULT 0,
  source_file TEXT NOT NULL       -- for gemini replace-per-file and debugging
)

scanned_files (
  path        TEXT PRIMARY KEY,
  size        INTEGER,
  mtime       INTEGER,
  byte_offset INTEGER             -- resume point; used by the Claude adapter
)

prices (                          -- built from LiteLLM data
  model                 TEXT PRIMARY KEY,
  input_per_tok         REAL,
  output_per_tok        REAL,
  cache_read_per_tok    REAL,
  cache_write_5m_per_tok REAL,
  cache_write_1h_per_tok REAL
)

price_overrides (                 -- user-entered, wins over prices
  model                 TEXT PRIMARY KEY,  -- raw model name as logged
  input_per_tok         REAL,
  output_per_tok        REAL,
  cache_read_per_tok    REAL,
  cache_write_per_tok   REAL
)
```

**Token semantics invariant** (normalized across all sources):
`input_tokens`, `cache_read_tokens`, and cache-write tokens are **mutually
exclusive**; total prompt tokens = input + cache_read + cache_write_5m +
cache_write_1h. Sources with inclusive semantics (Codex, Gemini report cached
as a subset of input — verified on all 1,623 Codex and 1,385 Gemini events on
this machine) subtract cached from raw input in the adapter. Claude and Hermes
already report exclusive input.

**Requests** = `SUM(api_calls)` everywhere, meaning API calls uniformly.
Claude/Codex/Gemini adapters leave the default 1 per event; the Hermes adapter
writes the session row's `api_call_count` (verified: 26 Hermes sessions = 314
API calls — COUNT(*) would undercount 12×).

**Cost is never stored** on events; it is computed at query time by joining
`price_overrides` then `prices`, so pricing updates apply retroactively.

**Timezone**: timestamps are stored as UTC epoch seconds; all bucketing and
range filtering happen in the Mac's local timezone at query time
(`date(timestamp,'unixepoch','localtime')`). "Today" = local midnight → now;
"7d"/"30d" = today plus the previous 6/29 local calendar days (matching daily
buckets); a custom range is inclusive of both endpoint dates (end bound =
start of the following local day, exclusive). Verified: ~34% of this machine's
events fall in local 00:00–08:00, so UTC bucketing would misplace a third of
usage — and break the ccusage comparison, which buckets in local time.

## Source adapters

Each adapter emits normalized events. All four are independent; a failure in
one never blocks the others.

### Claude Code

- **Files:** `~/.claude/projects/**/*.jsonl` (append-only JSONL).
- **Extract:** assistant messages' `message.usage`: `input_tokens` (already
  excludes cache), `output_tokens`; cache writes are split by TTL from
  `usage.cache_creation.{ephemeral_5m_input_tokens, ephemeral_1h_input_tokens}`
  (verified: the split sums exactly to `cache_creation_input_tokens` in all
  13,944 deduped records; if the sub-object is ever absent, put the whole
  `cache_creation_input_tokens` in 5m); `cache_read_input_tokens` → cache_read.
  Model from `message.model`.
- **Skip rule:** assistant lines whose four usage fields are all zero — these
  are `<synthetic>` error placeholders (83 on this machine), not API calls.
  Verified lossless: no genuine call has all-zero usage. This keeps junk out
  of the model dropdown, request counts, and the unpriced-models footer.
- **Project:** the per-line `cwd` field (present on 100% of usage-bearing
  lines; the dash-encoded directory name is provably lossy — cannot
  distinguish `/` from `-` — and is only a fallback when `cwd` is absent).
  Worktree paths are rolled up to the parent repo by stripping everything
  from `/.claude/worktrees/` onward.
- **Dedup key:** `claude:{message.id}:{requestId}` — the same message can
  appear in multiple files after session resume. Fall back to
  `claude:{message.id}` when `requestId` is null (defensive; 0 of 32,355 real
  assistant lines lack it).
- **Incremental:** byte-offset resume. `byte_offset` always points to the end
  of the last **complete newline-terminated line**; a trailing partial line
  (mid-append race with the 30s timer) is left unconsumed for the next scan,
  not counted as malformed. Resume only when current size ≥ stored size; on
  shrink or parse inconsistency at the resume point, re-parse the whole file
  (idempotent via dedup keys).

### Codex CLI

- **Files:** `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.
- **Extract:** `token_count` events carry **cumulative snapshots**, and
  duplicate snapshot lines are common (10 of 28 token-bearing files on this
  machine; worst case sums to 2× the true total). Therefore: per-event tokens
  = `max(0, delta)` of each `total_token_usage` field between consecutive
  `token_count` events within a file; the first event contributes its full
  totals. Duplicate snapshots self-correct (delta = 0). Verified: this
  reproduces every file's final total exactly (28/28 files). Skip events with
  `info: null` (7 exist) and all-zero degenerate rows (9 exist).
- **Normalize:** input = Δinput − Δcached (cached is a subset of input, per
  OpenAI semantics); cache_read = Δcached; output = Δoutput (includes
  reasoning); cache writes 0 (not reported).
- **Model:** NOT in session_meta (verified: 0 of 47 metas have a model key).
  Track the most recent `turn_context.payload.model` while streaming and stamp
  each token_count event with it; fall back to `unknown` (→ unpriced) if none
  seen. Note: `codex-auto-review` (~1.08M input tokens here) has no LiteLLM
  entry and legitimately lands in the unpriced footer.
- **Project:** `cwd` from the most recent `session_meta` (resumes append
  additional meta lines; "top of file" is wrong).
- **Dedup key:** `codex:{file_stem}:{byte_offset_of_line}` — stable under
  append, known for free while streaming.
- **Incremental:** re-parse any changed file in full (the whole corpus is
  54MB, and full re-parse avoids persisting cumulative-totals/model resume
  state; idempotent via offset-based dedup keys).

### Gemini CLI

- **Files:** `~/.gemini/tmp/*/chats/session-*.json` (whole-file JSON, rewritten
  as the session grows). Verified: 30/39 files carry token data; the rest are
  trivial sessions and simply contribute no events.
- **Extract:** messages carry `tokens: {input, output, cached, thoughts, tool,
  total}` and a per-message `model`. Normalize: input = `input` − `cached`
  (cached is a subset of input — verified `total == input + output + thoughts`
  exactly across all 1,385 token-bearing messages); cache_read = `cached`;
  output = `output` + `thoughts`; cache writes 0. Messages without a `tokens`
  field contribute no event.
- **Project:** the `tmp/` subdirectory name reverse-mapped to the real
  absolute path via `~/.gemini/projects.json` (real path → friendly name);
  older hash-named dirs are shown as a shortened hash.
- **Dedup key:** `gemini:{sessionId}:{message.id}`.
- **Incremental:** on mtime/size change, `DELETE WHERE source_file = ?` then
  re-insert — replace-per-file, no offsets.

### Hermes

- **Source:** `~/.hermes/state.db` (SQLite), `sessions` table — has
  `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`
  (all mutually exclusive, Anthropic-style), `reasoning_tokens`, `model`,
  `started_at`, `api_call_count`, `cwd`.
- **Extract:** one event per session row; `reasoning_tokens` counted into
  output; `cache_write_tokens` → cache_write_5m. `api_calls` =
  `api_call_count`. Timestamp = `started_at` **truncated to whole seconds**
  (it is REAL epoch seconds with a fractional part, not ms).
- **Project:** `sessions.cwd` when non-null/non-empty (the column exists but
  is rarely populated), else NULL.
- **Dedup key:** `hermes:{session_id}`, upserted — live session rows mutate
  (verified: 14 rows with NULL ended_at and growing totals).
- **Access:** open read-only (`?mode=ro`) with a busy timeout; on lock
  failure, keep prior cached rows and report staleness.

## Pricing

- Bundle a snapshot of LiteLLM's `model_prices_and_context_window.json`; on
  launch, fetch the latest from GitHub and cache to the app data dir; any
  fetch failure silently falls back to the newest local copy.
- **Lookup order per raw model name:** user override → exact LiteLLM key →
  normalized fallback. Verified: every Claude/Codex/Gemini model string on
  this machine is already an exact LiteLLM key, so normalization is a
  fallback, not the primary path.
- **Normalized fallback build rules** (collisions are real: `chatgpt/gpt-5.4`
  has null costs; `replicate/.../gemini-2.5-flash` is 8× the correct rate):
  skip entries whose input AND output costs are null; prefer canonical
  providers (`anthropic`, `openai`, `gemini`, `vertex_ai-language-models`)
  over provider-prefixed resellers; never overwrite a non-null row with a
  null one. Never silently bind a log model to a *different* provider's
  prefixed entry.
- **Cache-write rates by TTL:** `cache_write_5m_per_tok` from LiteLLM's
  `cache_creation_input_token_cost` (1.25× input for Anthropic);
  `cache_write_1h_per_tok` from `cache_creation_input_token_cost_above_1hr`
  (2× input), falling back to the 5m rate when absent. Measured: 79% of this
  machine's Claude cache writes are 1h-TTL; flat pricing would understate
  Claude cost ~11.6% (~$353).
- **Price overrides** (for self-hosted models — all Hermes models here run on
  custom endpoints with no list price): user-editable per-model rates. In the
  UI, each model in the unpriced-models footer list gets an "set price…"
  action opening a small dialog with input/output/cache-read/cache-write
  fields entered as **$ per 1M tokens** (stored per-token). Overrides win over
  LiteLLM for any model they name and apply retroactively at query time.
- **Partial pricing display:** cost everywhere = sum over priced tokens
  (hero, trend line, breakdown totals). When the active filter's result
  contains any unpriced tokens, the hero cost shows an explicit marker
  ("≥ $X · N unpriced models", referencing the footer list). Literal
  "unpriced" replaces the number only when zero tokens in range are priced.
  A breakdown row for an unpriced model shows "unpriced" in its cost cell.
- **Cost labeling:** the hero cost is labeled "Est. cost" with a permanent
  sub-label "at API list prices — not billed" (all four sources on this
  machine are subscription, free-tier, or self-hosted).
- **Cache hit rate** = cache_read / (input + cache_read + cache_write) —
  well-defined because input excludes cache reads by the normalization
  invariant.

## Dashboard UI

Single screen, dark theme, English. Layout mirrors the reference screenshot:

1. **Filter bar:** tool picker — single-select segmented control
   (All | Claude | Codex | Gemini | Hermes); model dropdown (single-select,
   raw model names); date range (Today / 7d / 30d / All / custom); refresh
   interval. `tools[]`/`models[]` arrays are just the IPC transport shape.
2. **Hero card:** total tokens (big number) = **input + output + cache write
   + cache read summed** (ccusage-style; consistent with the est. cost beside
   it), request count, estimated cost with sub-label.
3. **Stat cards:** input, output, cache write, cache read, cache hit rate
   (progress bar).
4. **Trend chart:** Recharts area/bar of tokens per bucket with cost line on a
   secondary axis. Daily buckets; hourly when range = Today.
5. **Breakdown tables:** by model and by project — tokens in/out/cache,
   est. cost, requests. Project rows are keyed by absolute path across all
   sources (so the same repo merges across tools) and displayed shortened to
   the basename. All widgets respect the active filters.
6. **Status footer:** last scan time / "scanning…", per-source event counts,
   skipped-line counts, unpriced models (each with a "set price…" action).

Raw stored model names are displayed everywhere; normalization exists only
inside price matching.

## Error handling

Principle: degrade per source, never crash the dashboard.

- Malformed log line → skip, increment per-source skipped counter (footer).
  A trailing partial line is *not* malformed — it is left for the next scan.
- Missing tool directory → tool shows zero events; not an error.
- Hermes DB locked → serve cached rows + "last synced X ago" note.
- Pricing fetch failure → bundled/cached snapshot, silent.
- A panicking adapter is caught per-source; other sources still ingest.

## Testing

- **Adapter unit tests (Rust):** fixture files trimmed and anonymized from the
  real logs on this machine — formats verified against reality. Must cover:
  - Codex: fixture derived from a file with duplicate snapshots asserting the
    adapter total equals the file's final `total_token_usage` (3,373,003 for
    the reference file), not the naive sum (6,666,021); `info: null` and
    all-zero rows skipped.
  - Codex + Gemini: input-excludes-cached subtraction asserted per fixture.
  - Claude: duplicate message id across two files deduped; all-zero
    `<synthetic>` lines skipped; worktree cwd rolls up to parent repo;
    5m/1h cache split sums to `cache_creation_input_tokens`.
  - Hermes: `api_calls` taken from `api_call_count`; fractional `started_at`
    truncated.
- **Aggregation test:** fixed fixture set → exact expected summary, trend, and
  breakdown numbers, bucketed in a pinned timezone.
- **Pricing tests:** known tokens × known rates → exact cost; `gpt-5.4`
  resolves to $2.5/M input despite null-cost `chatgpt/gpt-5.4` existing;
  `gemini-2.5-flash` resolves to $0.30/M despite an 8× reseller entry;
  unknown model → "unpriced", not $0; an override wins over LiteLLM.
- **Manual sanity check:** compare Claude totals against `ccusage` on this
  machine before calling v1 done (both bucket in local time).

## Tech stack

- Tauri v2, Rust (rusqlite, serde, glob), SQLite (WAL)
- React 18 + TypeScript + Vite, Recharts
- Target: macOS (Apple Silicon)
