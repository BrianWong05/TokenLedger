# Canonical token semantics: input excludes cache reads

TokenLedger stores every Usage Record using Anthropic-style **exclusive-input**
semantics — Input Tokens, Cache Read Tokens, and Cache Write Tokens are mutually
exclusive buckets — because that is the only partition under which a cross-source
"total tokens" and Cache Hit Rate are meaningful (no double counting).

Claude and Hermes report this natively; Codex, Gemini, and Grok report cached
tokens *inside* input, so their adapters subtract cached-from-input at ingest. We
chose to normalise at ingest (the stored ledger is already uniform) rather than
store each source's native semantics and reconcile at query time.

## Consequences

The rule is load-bearing across every adapter that ingests a cache figure, the
cost calculation, and the Cache Hit Rate formula. Reversing it (e.g. switching to
inclusive-input) would touch all of them, so it is recorded here to stop a future
reader from "fixing" a subtraction that looks odd in isolation. Grok joined the
subtracting adapters when its logs began reporting a per-Turn usage rollup, which
is the shape this rule anticipates: a Source's native semantics change under us,
the stored partition does not.
