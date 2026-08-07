# WorkBuddy and CodeBuddy share one transcript parser

WorkBuddy and CodeBuddy are two independent Sources that write the same
Claude-Code-like JSONL transcript shape to different roots
(`~/.workbuddy/projects/**/*.jsonl` and `~/.codebuddy/projects/**/*.jsonl`),
and the CodeBuddy CLI/IDE/VS Code plugin and WorkBuddy desktop app share an
account identity. We decided they remain two Sources with one shared parser —
each keeps its own identity, Capabilities, artifact root, and pricing
resolution, but usage extraction, cache semantics, subagent handling, and
deduplication are a single implementation (precedent: Oh My Pi sharing pi's
parser). All rules below apply to both.

## Usage Record granularity

One Usage Record per line carrying non-zero usage — `function_call` lines
(and the minority of `message` lines that report usage), never `reasoning`,
`function_call_result`, `file-history-snapshot`, or `ai-title` lines. Each
usage-bearing line is one Request (`usage.requests`, 1 in the observed
Artifacts). Deduplication uses the line `id`. `summary` lines are booked only
when they carry non-zero usage — a zero-token `summary` is not a Usage Record
(Issue #77 acceptance criterion), and the line type alone never makes one.

## Cache rule

The transcript's `usage.inputTokens` includes cache reads, so it cannot be
booked as Input Tokens directly. OpenAI-style `prompt_cache_hit_tokens` /
`prompt_cache_miss_tokens` are the primary cache fields; Anthropic-style
`cache_read_input_tokens` (nested `message.usage`, then `rawUsage`) is the
fallback. The writer populates exactly one convention per line type and leaves
the other at a placeholder zero, so the first non-zero candidate in priority
order is the Cache Read — a present-but-zero primary never defeats a populated
fallback. Input Tokens = `inputTokens − cacheRead`; Cache Write from
`cache_creation_input_tokens` / `prompt_cache_write_tokens`. This keeps the
four token categories mutually exclusive across Sources, as ADR-0001 requires.

## Subagents are additive

The parent `Agent` function_call's usage does not include the subagent's
usage; `subagents/agent-*.jsonl` transcripts report their own Requests. They
are scanned as additional Usage Records in the same Session, never skipped.
The synthetic fixture must verify a parent/subagent pair does not double-count.

## SQLite is metadata, never usage

The `workbuddy.db` `sessions` table supplies Session metadata and discovery
(cwd, model, status, timestamps) for Sessions whose JSONL is pruned;
`session_usage.used/size` are storage accounting, not tokens, and
`credit_json` holds billed credits. No table can produce a Usage Record, so
usage comes exclusively from the JSONL transcripts. Implemented (Issue #79)
via the Ledger's `source_sessions` table: a pruned Session is recorded as
metadata (never token figures), deleted Sessions never resurface, and the
upsert is idempotent per (source, session_id).

## Logged credit is ignored

`rawUsage.credit` and `session_usage.credit_json` report billed credit. Per
ADR-0002 (Cost is estimated list price, never billed spend) and the Goose
precedent, these figures are ignored; Cost resolves from the catalog by raw
Model name. The ignored-credit gap is documented as a fidelity limit.

## Consequences

- Two Source identities, one parser: changing one must not silently change the
  other's Source-native identity, but schema fixes apply to both.
- Cache-write TTL is indistinguishable from the Artifact; both write fields
  book as a single Cache Write bucket rather than the 5-minute/1-hour split.
- The fixture family is shared, which is what makes the CodeBuddy
  `function_call`-with-usage path testable before a live CodeBuddy sample
  exists.
