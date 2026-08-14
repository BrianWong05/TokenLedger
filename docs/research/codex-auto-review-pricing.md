# `codex-auto-review`: identity and List Price

Date: 2026-08-13

## Finding

`codex-auto-review` has no authoritative public API List Price. TokenLedger
should preserve that raw Model name and leave it **Unpriced** unless future
primary evidence proves a public, appropriately scoped mapping. A user-supplied
Override remains valid.

OpenAI documents Auto-review as a separate reviewer agent that handles eligible
sandbox-boundary approval requests; it does not change the main agent or grant
additional permissions.[^auto-review-docs]

The current open-source Codex client uses `codex-auto-review` as the default
approval-review backend slug. The OpenAI provider selects `gpt-5.6-luna`
instead only for API-key authentication.[^provider-defaults][^provider-selection]
The introducing commit says this explicitly: API-key Guardian reviews use Luna,
while ChatGPT-authenticated reviews retain `codex-auto-review`.[^luna-commit]

The route is dynamic rather than a stable alias. A parent Model can supply an
`auto_review_model_override`; the client then checks the available Model catalog
and can fall back to the parent Model slug when the requested reviewer Model is
unavailable.[^guardian-routing] Consequently, the API-key choice of Luna is not
authoritative evidence that ChatGPT's `codex-auto-review` slug equals Luna.

OpenAI's public API Model catalog and Codex credit rate card list named public
Models, including GPT-5.6 Luna, but neither lists `codex-auto-review` as a priced
row.[^public-models][^codex-pricing] Codex under ChatGPT plans uses shared usage
limits and credits; API-key sessions are separately charged at API token rates.
Those are different concepts from TokenLedger's estimated public API List
Price.[^codex-pricing]

## Local artifact check

Metadata-only inspection of representative parent/Guardian rollout pairs found
the Guardian usage recorded as `codex-auto-review` at low effort alongside
parents recorded as `gpt-5.5` and `gpt-5.6-sol`. The Guardian artifacts identify
their parent session but do not identify the backend weights or a public API
Model mapping:

| Date | Parent Model | Guardian Model |
| --- | --- | --- |
| 2026-07-03 | `gpt-5.5` | `codex-auto-review` |
| 2026-07-24 | `gpt-5.6-sol` | `codex-auto-review` |
| 2026-08-13 | `gpt-5.6-sol` | `codex-auto-review` |

No prompts, responses, or hidden reasoning were inspected. These local Source
Artifacts confirm the raw label seen by TokenLedger, not what Model served it.

## Decision-ready conclusion

Treat `codex-auto-review` as an internal, dynamically resolved reviewer route:

1. Preserve the raw Model identity.
2. Leave it Unpriced by default.
3. Do not inherit the parent Model's price or assign Luna's price.
4. Allow an explicit user Override.
5. Add a mapping only when a future primary source proves its Model and public
   List Price for the relevant product/authentication route and time period.

This follows TokenLedger's definition of Cost as estimated public API
list-price value rather than billed spend, and its rule to prefer a publisher's
rate only when that rate can be identified.[^adr-cost][^adr-publisher]

[^auto-review-docs]: OpenAI, [Auto-review](https://learn.chatgpt.com/docs/sandboxing/auto-review).
[^provider-defaults]: OpenAI Codex source, [`provider.rs` reviewer defaults](https://github.com/openai/codex/blob/b1373b74a27d1d9b65074a873202683355cae772/codex-rs/model-provider/src/provider.rs#L99-L106).
[^provider-selection]: OpenAI Codex source, [`approval_review_preferred_model`](https://github.com/openai/codex/blob/b1373b74a27d1d9b65074a873202683355cae772/codex-rs/model-provider/src/provider.rs#L314-L325).
[^luna-commit]: OpenAI Codex commit, [Use Luna for API-key Guardian reviews](https://github.com/openai/codex/commit/c4f42d161ae44a8d696ee9fb595709661979d187).
[^guardian-routing]: OpenAI Codex source, [`guardian_review_session_config`](https://github.com/openai/codex/blob/b1373b74a27d1d9b65074a873202683355cae772/codex-rs/core/src/guardian/review.rs#L740-L830).
[^public-models]: OpenAI, [API Model catalog](https://developers.openai.com/api/docs/models/all).
[^codex-pricing]: OpenAI, [Codex pricing](https://learn.chatgpt.com/docs/pricing).
[^adr-cost]: TokenLedger, [ADR-0002: Cost is estimated list-price value, not billed spend](../adr/0002-cost-is-estimated-list-price-value-not-billed-spend.md).
[^adr-publisher]: TokenLedger, [ADR-0009: Price a Model at its publisher's rate](../adr/0009-price-a-model-at-its-publishers-rate.md).
