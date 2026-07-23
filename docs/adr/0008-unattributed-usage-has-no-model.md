# Unattributed Usage has no Model: Model is nullable, never a sentinel

Some usage arrives with no trustworthy Model. pi reports token usage on tool
results and on extension-provided summaries — nested model work whose Model the
Source does not record. TokenLedger counts those tokens (they are real usage)
but must not invent a Model for them, and must not let them masquerade as a
priced call.

The decision: a Usage Record's Model is **nullable**. Unattributed Usage stores
`model = NULL` end to end — never a sentinel like `unknown`, `__unattributed__`,
or "Tools/summaries". A sentinel would leak into model filters, price-resolution
joins, the Overrides editor, and the Pricing table as if it were a real Model;
NULL is excluded from all of those by construction (`WHERE model IS NOT NULL`),
while still contributing to Source, Project, time-series, Trend, Activity, and
overall token and Request totals.

This forced two contracts to distinguish **two different reasons** a Cost can be
incomplete, which callers must tell apart:

- **Unpriced Model** — a real Model with no Override and no matching List Price.
- **Unattributed Usage** — usage with no Model at all.

Cost is `NULL` (shown as *unavailable*, never `$0`) when a selection has no
priced Model usage; it is a **Partial Cost** (a `≥` marker) when priced usage is
mixed with Unpriced Models, Unattributed Usage, or both. The model breakdown
carries a distinct null-Model row so "Unattributed usage" renders as a
non-clickable, non-editable line rather than a Model.

The schema migration (v7) rebuilds `events` with a nullable `model` column
without touching token counts, and the price tables stay keyed exclusively by
real Model identities.

## Consequences

Any new query or view that resolves, filters, or prices Models must treat NULL
as "no Model" — excluded from Model-level surfaces, included in every aggregate.
Reintroducing a sentinel to avoid nullable handling re-litigates this decision.
Adding a Source that reports Model-less usage (as pi does) needs no new
machinery: emit `model = NULL`.
