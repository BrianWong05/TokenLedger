---
status: accepted (OpenRouter fallback planned — not in v1; v1 reads LiteLLM only)
---

# Price resolution: Override → LiteLLM → OpenRouter → Unpriced

A Model's rate is resolved in strict precedence: a user **Override** wins; else
the **LiteLLM** catalog (exact model key, then normalised key) — the canonical
provider list price; else the **OpenRouter** catalog; else the Model is
**Unpriced**.

OpenRouter is a *fallback*, not a peer: for a Model with an official provider
price (Claude, GPT, Gemini) LiteLLM's rate is the true list price, whereas
OpenRouter's is a marked-up resale rate, so preferring LiteLLM avoids silently
inflating Cost for metered-provider Models. But for a self-hosted Model with no
official price (the Hermes Qwen/GLM/MiniMax/DeepSeek family) OpenRouter's resale
rate is the best available proxy and is preferred over leaving it Unpriced.

## Status

v1 ships with LiteLLM only; the Override → LiteLLM → Unpriced subset is
implemented. Adding the OpenRouter catalog as the fallback tier is planned
follow-up work, recorded here so the precedence is settled before it is built.
