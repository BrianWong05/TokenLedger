# AI Usage Tracker — Design

**Date:** 2026-07-07
**Status:** Approved by user (brainstorming session)

## Overview

A macOS desktop app (Tauri v2) that tracks token usage and estimated cost across
the AI coding tools on this machine: Claude Code, Codex CLI, Gemini CLI, and
Hermes. It parses each tool's local logs into a normalized SQLite cache and
shows a dark-themed dashboard: totals, estimated cost, cache hit rate, a daily
trend chart, and breakdowns by tool, model, project, and date range.

## Goals

- Zero-effort tracking: parse local logs automatically; no manual entry.
- Token detail per event: input, output, cache write, cache read.
- Estimated cost from public API list prices (LiteLLM pricing database).
- Fast: incremental scanning; instant queries after first scan.
- Works fully offline (pricing falls back to a bundled snapshot).

## Non-goals (v1)

- Provider billing/usage API polling (no API keys, no real-billing overlay).
- Windows/Linux support.
- Tracking tools other than the four above.
- Live file watchers (a periodic incremental re-scan is sufficient).

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
| `summary(filters)` | Totals: tokens by category, requests, est. cost, cache hit rate |
| `trend(filters, bucket)` | Time series (daily; hourly when range = today) of tokens + cost |
| `breakdown(by, filters)` | Grouped totals, `by` ∈ {tool, model, project} |

`filters` = tools[], models[], project, date range. All aggregation happens in
SQL; the frontend only renders.

### Refresh model

Scan on launch, then on a user-configurable timer (off / 30s / 60s; default
30s). Incremental scans are cheap, so no file watchers.

## Data model (SQLite)

```sql
events (
  dedup_key   TEXT PRIMARY KEY,   -- source-specific, see adapters
  source      TEXT NOT NULL,      -- claude | codex | gemini | hermes
  timestamp   INTEGER NOT NULL,   -- unix epoch seconds, UTC
  model       TEXT NOT NULL,
  project     TEXT,               -- repo/directory the usage belongs to
  input_tokens        INTEGER NOT NULL DEFAULT 0,
  output_tokens       INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens   INTEGER NOT NULL DEFAULT 0,
  cache_write_tokens  INTEGER NOT NULL DEFAULT 0,
  source_file TEXT NOT NULL       -- for gemini replace-per-file and debugging
)

scanned_files (
  path        TEXT PRIMARY KEY,
  size        INTEGER,
  mtime       INTEGER,
  byte_offset INTEGER             -- resume point for append-only JSONL
)

prices (
  model            TEXT PRIMARY KEY,  -- normalized model name
  input_per_tok    REAL,
  output_per_tok   REAL,
  cache_read_per_tok  REAL,
  cache_write_per_tok REAL
)
```

Cost is **never stored** on events; it is computed at query time by joining
`prices`, so pricing updates apply retroactively to all history.

One event = one API call (Claude/Codex/Gemini) or one session (Hermes — its
database only stores session-level totals, which is fine for aggregation).

## Source adapters

Each adapter emits normalized events. All four are independent; a failure in
one never blocks the others.

### Claude Code

- **Files:** `~/.claude/projects/**/*.jsonl` (append-only JSONL).
- **Extract:** assistant messages' `message.usage`: `input_tokens`,
  `output_tokens`, `cache_creation_input_tokens` → cache_write,
  `cache_read_input_tokens` → cache_read; model from `message.model`.
- **Project:** decoded from the project directory name under `projects/`.
- **Dedup key:** `claude:{message.id}:{requestId}` — the same message can
  appear in multiple files after session resume; `INSERT OR IGNORE` handles it.
- **Incremental:** resume from stored byte offset when size grew and the prefix
  is unchanged (mtime+size heuristic); re-parse whole file otherwise.

### Codex CLI

- **Files:** `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` (append-only JSONL).
- **Extract:** `event_msg` payloads of type `token_count`, using
  `last_token_usage` (per-call delta): `input_tokens`, `cached_input_tokens` →
  cache_read, `output_tokens` (includes reasoning). Cache write: 0 (not
  reported by Codex).
- **Model / project:** from the session-meta line at the top of each file
  (model and cwd).
- **Dedup key:** `codex:{file_stem}:{line_number}`.
- **Incremental:** byte-offset resume, same as Claude.

### Gemini CLI

- **Files:** `~/.gemini/tmp/*/chats/session-*.json` (whole-file JSON, rewritten
  as the session grows).
- **Extract:** per-message token fields; exact field names confirmed against
  real files during implementation (fixtures exist on this machine).
- **Project:** the `tmp/` subdirectory name; hash-named dirs resolved to real
  paths via `~/.gemini/projects.json` where possible, else the hash is shown.
- **Dedup/incremental:** on mtime/size change, `DELETE WHERE source_file = ?`
  then re-insert — replace-per-file, no offsets.

### Hermes

- **Source:** `~/.hermes/state.db` (SQLite), `sessions` table — already has
  `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`,
  `reasoning_tokens`, `model`, `started_at`.
- **Extract:** one event per session row; `reasoning_tokens` are counted into
  output. Timestamp = `started_at`. Project: NULL (Hermes sessions are not
  directory-scoped).
- **Dedup key:** `hermes:{session_id}`, upserted (session rows accumulate
  tokens while live).
- **Access:** open read-only (`?mode=ro`) with a busy timeout; on lock failure,
  keep prior cached rows and report staleness.

## Pricing

- Bundle a snapshot of LiteLLM's `model_prices_and_context_window.json` in the
  app. On launch, fetch the latest from GitHub and cache to the app data dir;
  any fetch failure silently falls back to the newest local copy.
- Model matching: normalize names (lowercase, strip provider prefixes and date
  suffixes) and match log model → price entry. Rates used: input, output,
  cache read, cache write (per token).
- Unknown model: tokens still counted everywhere; cost shows as "unpriced"
  (never a silent $0). The status footer lists unmatched model names.
- Cache hit rate = cache_read / (input + cache_read + cache_write).

## Dashboard UI

Single screen, dark theme, English. Layout mirrors the reference screenshot:

1. **Filter bar:** tool picker (All + per-tool icons), model dropdown,
   date range (Today / 7d / 30d / All / custom), refresh interval.
2. **Hero card:** total tokens (big number), request count, estimated cost.
3. **Stat cards:** input, output, cache write, cache read, cache hit rate
   (progress bar).
4. **Trend chart:** Recharts area/bar of tokens per bucket with cost line on a
   secondary axis. Daily buckets; hourly when range = Today.
5. **Breakdown tables:** by model and by project — tokens in/out/cache,
   est. cost, requests. All widgets respect the active filters.
6. **Status footer:** last scan time, per-source event counts, skipped-line
   counts, unpriced models.

## Error handling

Principle: degrade per source, never crash the dashboard.

- Malformed log line → skip, increment per-source skipped counter (footer).
- Missing tool directory → tool shows zero events; not an error.
- Hermes DB locked → serve cached rows + "last synced X ago" note.
- Pricing fetch failure → bundled/cached snapshot, silent.
- A panicking adapter is caught per-source; other sources still ingest.

## Testing

- **Adapter unit tests (Rust):** fixture files trimmed and anonymized from the
  real logs on this machine — formats verified against reality.
- **Aggregation test:** fixed fixture set → exact expected summary, trend, and
  breakdown numbers; includes the Claude duplicate-message dedup case.
- **Pricing test:** known tokens × known rates → exact cost; unknown model →
  "unpriced", not $0.
- **Manual sanity check:** compare Claude totals against `ccusage` on this
  machine before calling v1 done.

## Tech stack

- Tauri v2, Rust (rusqlite, serde, glob), SQLite
- React 18 + TypeScript + Vite, Recharts
- Target: macOS (Apple Silicon)
