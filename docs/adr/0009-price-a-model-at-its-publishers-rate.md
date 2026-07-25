---
status: accepted
---

# Price a Model at its publisher's rate

Supersedes ADR-0003.

A Model's **List Price** is the per-token rate set by the organisation that
publishes it — Anthropic's for Claude, Z.AI's for GLM (CONTEXT.md). Resolution
prefers that rate wherever it can be identified:

1. **Override** — user-supplied, on the raw Model name.
2. **Publisher rate** — the publisher's own entry in OpenRouter's per-Model host
   listing, with any field it omits filled from LiteLLM.
3. **LiteLLM** — exact key, then normalised key.
4. **Routed Rate** — OpenRouter's blended per-Model figure, exact then normalised.
5. **Unpriced.**

## Why this replaces a catalog-ordered rule

ADR-0003 ranked catalogs against each other and then, on the evidence of one
Model, ranked match *quality* across them. Both framings share a defect: neither
can express whose price a figure is. That is the only question that matters once
a Model has thirty-three hosts, and not asking it is what let a promotional
average outrank a publisher's own rate.

The correction is not a better ordering of the same inputs. It is a new input:
the publisher's entry, which neither catalog's headline figure exposes.

## Identifying the publisher

Structurally, not from a list of who publishes what. In OpenRouter's per-Model
host listing each host carries a tag, and the host whose tag vendor matches the
Model identifier's vendor is the publisher — `z-ai/fp8` publishes `z-ai/glm-5.2`,
`cloudflare` does not. Verified against Anthropic, OpenAI, Z.AI, DeepSeek and
MiniMax Models, and it correctly finds nothing for Models whose publishers do not
self-host there (Google does not serve Gemini on OpenRouter).

This is deliberately unlike the `CANONICAL` allowlist, which does the same job
inside LiteLLM and needs a human to maintain it. Where a structural signal exists,
prefer it.

A publisher may list several tiers; the first, cheapest one is taken as the List
Price rather than a premium `fast` variant.

## Why the merge is field-level

A publisher sometimes quotes fewer fields than LiteLLM does. Anthropic's OpenRouter
entry for `claude-fable-5` omits the 1-hour cache-write TTL that LiteLLM publishes,
and taking the publisher's entry wholesale would fall back to the 5-minute rate and
undercount those tokens by 37%. So the publisher's rate is the base and LiteLLM
fills only what it leaves empty — the same "never overwrite a non-null with a null"
rule the normalised merge already uses. Adopting a publisher rate can therefore
never *lose* a rate that was already published.

## Why the Routed Rate survives at the bottom

It is set by nobody and moves with other companies' discounts, so it is a poor
List Price. But it is better than Unpriced for a Model neither the publisher nor
LiteLLM covers, and no Model that is priced today may become Unpriced because of
this change.

## Mechanics

Write-time precedence alone cannot express this order. The stored keyspace holds
both raw ids and normalised names, and a routed id is always a vendor-prefixed
*exact* key while LiteLLM's fallback is only ever reachable *normalised* — so a
resolver that probes exact-before-normalised hands the Routed Rate the win no
matter what order the rows were written in. ADR-0003's mechanics worked only
because its intended order happened to agree with that bias.

So precedence is now settled at read time. Each row records its source in the
`catalog` column — a publisher's name for tier 2, otherwise the catalog's — and a
lookup takes the better-sourced of the two candidate keys, breaking ties toward
the exact match. Write order still decides who owns a key within a tier.

Publisher rates are read one request per Model, only for Models the Ledger holds,
cached to their own snapshot file with per-Model timestamps.
