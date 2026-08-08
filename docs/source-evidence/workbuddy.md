# WorkBuddy Source evidence

Status as of 2026-08-07: the genuine private-Artifact gate is met from a live
desktop install; the adapter and synthetic fixture are implemented (Issue #77).
This file records what the live Artifact proves and the agreed parser design.

## Artifact

The Source's primary history is `~/.workbuddy/projects/<project>/<session>.jsonl`
— one file per Session, written by the WorkBuddy desktop app in a
Claude-Code-like transcript shape. The directory slug encodes the working
directory (`Users-brianwong-Project-usage` ↔ `/Users/brianwong/Project/usage`),
and the `cwd` field on every line carries the authoritative absolute path.
Per-session `subagents/agent-*.jsonl` transcripts and `tool-results/` blocks
live alongside the main file.

## Evidence from the live install

Verified against real WorkBuddy data from one genuine macOS installation
(2026-08-07): four real
Sessions under `~/.workbuddy/projects/`, including the Session in which this
documentation was written, status `working` in `~/.workbuddy/workbuddy.db`.

Line types observed: `message`, `function_call`, `function_call_result`,
`reasoning`, `file-history-snapshot`, `ai-title`. Usage appears on
`function_call` lines (153 of 161 in one Session) and on a minority of
`message` lines; `reasoning`, `function_call_result`, `file-history-snapshot`,
and `ai-title` lines never carry usage.

Each usage-bearing line carries the model at that Request:

- `providerData.requestModelId` — the clean Model id the request ran on
  (e.g. `glm-5.2`), what a price resolves against; the parser prefers it
- `providerData.model` — usually equals `requestModelId`, but can carry an
  internal variant suffix no catalog carries (`glm-5.2-x` beside
  `requestModelId` `glm-5.2`); the parser uses it only as fallback
- `providerData.requestModelName` — display casing only (`Deepseek-V4-Flash`)

and three usage representations of the same Request:

- `providerData.usage` — normalized: `requests` (=1), `inputTokens`,
  `outputTokens`, `totalTokens`, plus `inputTokensDetails`/`outputTokensDetails`
- `providerData.rawUsage` — OpenAI-style: `prompt_tokens`, `completion_tokens`,
  `prompt_cache_hit_tokens`, `prompt_cache_miss_tokens`,
  `cache_creation_input_tokens`, `prompt_cache_write_tokens`,
  `completion_thinking_tokens`, `credit`, `cached_tokens`
- nested `message.usage` — Anthropic-style: `input_tokens`, `output_tokens`,
  `total_tokens`, `cache_read_input_tokens`

## The cache field disagreement (resolved)

`usage.inputTokens` **includes** cache reads: in one Request
`inputTokens=32221` while `prompt_cache_hit_tokens=32000` and
`prompt_cache_miss_tokens=221` (32221 = 32000 + 221). Yet `rawUsage` reports
`cache_read_input_tokens=0` for the same Request, while the nested
`message.usage` reports `cache_read_input_tokens=36224` for its own Request.
The two conventions disagree; the parser must not trust one blindly.

Agreed rule (ADR-0016): OpenAI-style `prompt_cache_hit_tokens` /
`prompt_cache_miss_tokens` is primary, Anthropic-style
`cache_read_input_tokens` is the fallback; Input Tokens =
`inputTokens − cacheRead`, Cache Read = the resolved cache-read figure, Cache
Write from `cache_creation_input_tokens` / `prompt_cache_write_tokens`. This
keeps the four token categories mutually exclusive across Sources.

## SQLite is metadata, not usage (resolved)

`~/.workbuddy/workbuddy.db` holds a `sessions` table (id, cwd, model, status,
created_at, updated_at, title) and a `session_usage` table whose `used`/`size`
columns are storage accounting — their values do not match JSONL token totals —
plus a `credit_json` of billed credit amounts. No table carries token usage.
The SQLite fallback (Issue #79, implemented) therefore supplies Session
metadata and discovery for Sessions whose JSONL is pruned: a Session present in
`sessions` but with no transcript on disk is recorded into the Ledger's
`source_sessions` table (cwd, model, timestamps, title) — metadata only, never
token figures. Deleted Sessions (`deleted_at` set) never resurface. `used`/
`size`/`credit_json` are never read as usage (ADR-0016).

## Subagents are additive (resolved)

A parent `Agent` function_call reported `inputTokens=66104`, while that
subagent's `subagents/agent-*.jsonl` transcript totalled 3,606,759 input
tokens across 38 Requests. Parent usage does not include subagent usage, so
the subagent transcripts are scanned as additional Usage Records in the same
Session rather than skipped (ADR-0016). The fixture must verify this on a
synthetic parent/subagent pair.

## Logged credit is ignored (resolved)

`rawUsage.credit` reports billed credit per Request (e.g. 0.93) and
`session_usage.credit_json` sums credits per Session. Per ADR-0002 and the
Goose precedent, Cost remains the list-price estimate resolved from the
catalog by raw Model name; `credit` is ignored and documented as a fidelity
limit (ADR-0016).

## Fidelity limits

- One Usage Record per usage-bearing line, deduplicated on the line `id`.
- Cache-write TTL is not distinguishable; both write fields book as a single
  Cache Write bucket (no 5-minute/1-hour split is provable from the Artifact).
- The `summary` line type appears in the CodeBuddy Artifact but not yet in
  WorkBuddy samples; its usage-bearing status is unproven and must not be
  assumed either way.
