---
status: superseded by ADR-0009
---

> **Superseded.** The reasoning below rests on a misreading, and is kept for that
> reason rather than corrected: the figure it calls OpenRouter's rate for
> `z-ai/glm-5.2` is not a rate anyone charges. It is a **Routed Rate** — blended
> across all 33 hosts serving the Model and pulled down by resellers discounting
> up to 47%. Z.AI, which publishes GLM, charges $1.40/$4.40, exactly what the
> LiteLLM row below was dismissed as "Cloudflare's resale rate". Cloudflare was
> matching the publisher, not marking it up. Acting on that reading moved the
> Model *away* from its publisher's price. See ADR-0009.

# Price resolution: Override → exact catalog match → normalised catalog match

A Model's rate resolves in strict precedence:

1. a user **Override**, on the raw Model name;
2. **LiteLLM**, exact match on the raw Model name;
3. **OpenRouter**, exact match on the raw Model name;
4. **LiteLLM**, match on the normalised Model name;
5. **OpenRouter**, match on the normalised Model name.

Failing all five, the Model is **Unpriced**.

## Why LiteLLM leads

OpenRouter is a *fallback*, not a peer: for a Model with an official provider
price (Claude, GPT, Gemini) LiteLLM's rate is the true list price, whereas
OpenRouter's is a marked-up resale rate, so preferring LiteLLM avoids silently
inflating Cost for metered-provider Models. But for a self-hosted Model with no
official price (the Hermes Qwen/GLM/MiniMax/DeepSeek family) OpenRouter's resale
rate is the best available proxy and is preferred over leaving it Unpriced.

## Why match quality outranks catalog order

The order above was originally recorded as a flat *LiteLLM tier, then OpenRouter
tier*. Building it revealed that the reasoning above holds only where LiteLLM
carries an official provider price — and inverts where it does not.

The measured case: `z-ai/glm-5.2`. LiteLLM's only coverage is
`cloudflare/@cf/zai-org/glm-5.2` — Cloudflare Workers AI's own resale listing,
reachable only after normalisation — at $1.40/$4.40 per 1M. OpenRouter carries
the Model under the exact raw name at $0.77/$2.43. A flat tier order would pick
Cloudflare's rate and inflate Cost by roughly 1.8x, which is precisely the
outcome preferring LiteLLM was meant to prevent.

So the deciding evidence is *match quality*, not which catalog it came from: an
exact match on a vendor-qualified raw name is stronger evidence than a normalised
match that may land on an arbitrary host's row. The `CANONICAL` provider guard
already protects the major commercial providers from this failure inside the
LiteLLM catalog; tier 3 extends the same protection to open-weight Models.

Measured against a real Ledger of twenty-three Models, moving OpenRouter's exact
tier above LiteLLM's normalised tier changes exactly one resolved rate — the case
above. Every high-volume Model is an exact LiteLLM match and is untouched, and
both catalogs agree to the cent on all of them.

## Mechanics

Precedence is settled when the price table is rebuilt, not when it is read: the
five tiers run as ordered write passes and the first pass to claim a Model key
owns it, so a lookup needs only two probes (exact raw name, then normalised
name). This is equivalent to a tiered read-time resolve because the two key
spaces cannot collide — every OpenRouter identifier is vendor-prefixed and so
always contains a path separator, while normalisation strips through the last
separator and so never yields one. A test asserts that invariant rather than
assuming it.

Each row records the catalog it came from, which is what the Pricing tab's Rate
source column reports.

## Status

Implemented. LiteLLM ships with a bundled snapshot for first-run offline use;
OpenRouter does not, so a machine that has never reached it resolves without
tiers 3 and 5 — the same result as the pre-OpenRouter behaviour.
