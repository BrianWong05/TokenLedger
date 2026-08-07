# CodeBuddy Source evidence

Status as of 2026-08-07: the genuine private-Artifact gate is met from a live
install that produced usage during this session; the adapter and synthetic
fixture are implemented (Issue #78). This file records what the live Artifact
proves and the agreed parser design.

## Artifact

The Source's primary history is `~/.codebuddy/projects/<project>/<session>.jsonl`
— one file per Session, written by the CodeBuddy CLI/IDE/VS Code plugin in the
same Claude-Code-like transcript shape as WorkBuddy. The `cwd` field on every
line carries the absolute working directory.

## Evidence from the live install

Verified against the machine's own CodeBuddy data (2026-08-07): one Session
(`f7bde4fb…`, cwd `/Users/brianwong/Project/usage`) whose transcript grew from
12 lines with no usage at 18:53 to 25 lines with 4 usage-bearing lines by
19:00 while this documentation session ran. Line types observed: `message`,
`file-history-snapshot`, `ai-title`, and `summary` — a type not yet seen in
WorkBuddy samples. All 4 usage-bearing lines were `message` lines; the 2
`summary` lines carried no usage.

The usage schema is byte-for-byte the WorkBuddy shape:

- `providerData.usage` — normalized `requests`(=1), `inputTokens`,
  `outputTokens`, `totalTokens`, plus details arrays
- `providerData.rawUsage` — OpenAI-style `prompt_tokens`, `completion_tokens`,
  `prompt_cache_hit_tokens`, `prompt_cache_miss_tokens`,
  `cache_creation_input_tokens`, `prompt_cache_write_tokens`,
  `completion_thinking_tokens`, `credit`, `cached_tokens`
- nested `message.usage` — Anthropic-style `input_tokens`, `output_tokens`,
  `total_tokens`, `cache_read_input_tokens`
- `providerData.requestModelId` — the clean Model id the request ran on; the
  shared parser prefers it (ADR-0016). `providerData.model` usually equals it
  but can carry an internal variant suffix no catalog carries, so it is only
  the fallback. `providerData.requestModelName` is display casing. In every
  live CodeBuddy sample all three agreed on `hy3`

The cache convention matches WorkBuddy exactly: `usage.inputTokens` includes
cache reads (`inputTokens=25190`, `prompt_cache_hit_tokens=512`,
`prompt_cache_miss_tokens=24678`; 25190 = 512 + 24678) while
`rawUsage.cache_read_input_tokens=0`. The agreed rule (ADR-0016) therefore
applies identically: OpenAI-style hit/miss primary, Anthropic-style
`cache_read_input_tokens` fallback, Input = `inputTokens − cacheRead`.

## Shared parser

CodeBuddy and WorkBuddy are two independent Sources (per the Source boundary
decision) that share one transcript parser, in the same way Oh My Pi shares
pi's parser. The two artifact roots differ (`~/.codebuddy/projects` vs
`~/.workbuddy/projects`), the Source identity, Capabilities, and pricing
resolution differ, but the line schema, usage extraction, cache rule, subagent
rule, and deduplication are one implementation. Both are covered by the same
synthetic fixture family.

## Fidelity limits

- The live CodeBuddy sample contains only `message`-type usage so far; the
  `function_call`-with-usage path and the `summary` line type are proven by
  schema identity with WorkBuddy plus the shared fixture, not by a live
  CodeBuddy `function_call` sample.
- `summary`-type usage-bearing status is unproven and must not be assumed
  either way; a zero-token `summary` is not a Usage Record.
- Credit is logged (`credit`) and ignored, per ADR-0016 and ADR-0002.
