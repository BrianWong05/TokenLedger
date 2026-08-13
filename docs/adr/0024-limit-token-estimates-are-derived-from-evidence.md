# Limit token estimates are derived from canonical evidence

A Limit Token Estimate is a derived read model, not an authoritative stored
fact. TokenLedger persists canonical Usage Records, append-only Limit Readings,
and the identity, scope, completeness, and external-activity provenance needed
to prove their pairing; the backend derives the current readiness state, stable
core, estimate, and explanation from that evidence under the current estimator
policy in one consistent read. This keeps the result reconstructible when late
Records arrive, a coarse Record is superseded, evidence is corrected, or time
alone moves an otherwise unchanged result past its recency boundary.

## Consequences

- Every currently eligible fact participates immediately. Late or superseding
  Usage Records can revise an older eligible epoch, while legacy evidence stays
  excluded until its historical identity and completeness are genuinely proven.
- `Stale` means that evidence which forms a stable core under the current policy
  has aged outside the current readiness window; it is reconstructed rather than
  remembered from a prior app run.
- The public Limits query returns a tagged evaluation. `Ready` alone carries the
  full-precision `tokensPerPct`; every state carries `evaluatedAt`,
  `nextEvaluationAt`, `policyVersion`, and a bounded factual explanation. The
  frontend applies the current Used/Left percentage and display rounding.
- The Limits page evaluates on open, after Scan or live-Reading changes, and at
  `nextEvaluationAt`. Domain evidence gaps produce `Blocked`; unexpected storage
  or computation failures remain technical errors rather than readiness states.
- The normal response includes compact epoch, core, outlier, range, date, and
  reason summaries. Exact contributing Reading and Usage Record identities are
  deterministically reconstructible through diagnostics rather than sent on
  every page load.
- Start with a directly indexed query over the bounded evidence horizon. A
  disposable process-memory cache may be added only after profiling, and must be
  keyed by a complete evidence revision, estimator-policy version, and
  evaluation expiry. No authoritative or durable estimate materialization is
  introduced.
