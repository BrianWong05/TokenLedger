# Limit Token Estimates Design

The destination of Wayfinder map
[#118](https://github.com/BrianWong05/TokenLedger/issues/118). This document
assembles the decisions from
[#124](https://github.com/BrianWong05/TokenLedger/issues/124),
[#120](https://github.com/BrianWong05/TokenLedger/issues/120),
[#125](https://github.com/BrianWong05/TokenLedger/issues/125),
[#119](https://github.com/BrianWong05/TokenLedger/issues/119),
[#122](https://github.com/BrianWong05/TokenLedger/issues/122), and
[#123](https://github.com/BrianWong05/TokenLedger/issues/123) into one
implementation contract. The existing
[Limits page v1 specification](./2026-08-12-limits-page-v1-design.md) remains
normative except where this document adds the estimate beneath a Limit row.

## Goal

For every token-correlatable Limit row, show an approximate local token
equivalent for the selected Used or Left percentage and an approximate token
equivalent per displayed percentage point. Derive both from matching canonical
Usage Records and completed Limit Reading history. Show no number until the
evidence is proven, sufficient, recent, and uniquely consistent.

This is a local, workload-dependent correlation. It is never the vendor's
token denominator, ceiling, allowance, billing unit, or promise about future
service. Neither Claude nor Codex publishes the formula needed to recover such
a figure.

## Scope and hard boundaries

In scope:

- Claude, Codex, and a participation contract for a future Source;
- evidence identity, attribution, estimation, readiness, persistence, query,
  presentation, migration, diagnostics, and acceptance tests;
- the existing page-level Left/Used framing; and
- the existing English and Traditional Chinese locales.

Out of scope:

- hardcoded, purchased, or reverse-engineered vendor token ceilings;
- a separate authoritative 100% token quota;
- vendor weights, credits, Requests, Cost, or spend converted into tokens;
- new vendor endpoints, calls, polling, credential behavior, Companion
  behavior, or Limit acquisition;
- burn-rate forecasts, confidence percentages, notifications, history charts,
  Menu Bar Extra presentation, and durable estimate materialization; and
- promoting either throwaway prototype into production.

The Ready explanation may show `tokensPerPct * 100`, labelled **local
equivalent at 100%**. It appears only inside the explanation, never as a Limit
quota or a separate row figure.

## Domain semantics

`CONTEXT.md`, ADR-0021, and ADR-0022 are normative.

- A **Limit Reading** is a vendor percentage and reset state at one observed
  time. It holds no tokens.
- A **Limit Evidence Partition** contains comparable Readings in one reset
  epoch with the same proven Source, privacy-safe account/subscription
  identity, plan, metering regime, stable Limit identity, and explicit Model
  scope.
- A **Limit Evidence Interval** is a positive movement between comparable
  Readings plus the matching Usage Records after the earlier Reading and
  through the later Reading. It is candidate correlation, not causation.
- A **Limit Evidence Series** is the cross-epoch calibration grouping: sibling
  Limit Evidence Partitions whose identity fields are equal except for reset
  epoch. The readiness algorithm compares completed epochs only within one
  Series. This term makes the approved multi-epoch rule explicit; it does not
  relax any Partition identity requirement.
- A **Limit Token Estimate** is the approximate canonical tokens represented by
  one displayed percentage point and by the current selected Used/Left share.

Unknown identity is never a wildcard, and unavailable or incomplete evidence
is never zero.

## Canonical tokens

For every participating Usage Record:

```text
canonical tokens =
    input_tokens
  + output_tokens
  + cache_read_tokens
  + cache_write_5m_tokens
  + cache_write_1h_tokens
```

`reasoning_tokens` and context-attribution fields are not added because they
are classifications or subsets of the canonical buckets. `api_calls`, Cost,
credits, and Requests do not participate. All terms are the current canonical
Ledger values, so a later Record upgrade or supersession changes the derived
evidence.

## Evidence participation contract

### Required facts

The backend may derive an interval only when durable evidence proves every
field below. Physical table layout is an implementation choice; these logical
facts and their behavior are not.

| Fact | Contract |
| --- | --- |
| Reading identity | An immutable, privacy-safe identity for the exact Reading. Re-ingesting the same source observation deduplicates; a genuinely later equal-percentage observation remains distinguishable. |
| Source | Exact Source identity on both Reading and Usage capture context. |
| Account identity | Stable opaque account/subscription identity shared by both sides. Never persist an email, access token, refresh token, or reversible credential. Token fingerprints are not stable account identities. |
| Plan identity | The plan in force for the evidence, separate from presentation copy. Missing is unknown. |
| Metering regime | A stable adapter-defined identity for the vendor meter in force. A plan label alone is insufficient when one plan can use multiple regimes. |
| Limit identity | A raw vendor identity or adapter-defined canonical identity with a documented one-to-one mapping. A duration, display label, slot, slug, or pricing alias is insufficient by itself. |
| Reset epoch | A stable epoch identity. Existing reset-time jitter may use the Limits page's ten-minute band only when all other identity matches and no decrease makes the grouping ambiguous; ambiguity rejects the grouping. |
| Model scope | `all` for a Source-wide Limit, or a non-empty exact set of raw logged Model identities. A display-name-derived key is not a Model mapping. |
| Observation order | `observed_at` plus stable source order sufficient to order Readings. Equal-time Readings with no proven order cannot bound an interval. |
| Completeness coverage | Durable proof that local Source capture for the exact Source/account covers the interval. Current `SourceStatus`, absence of an error today, or a missing path treated as empty is not historical proof. |
| External activity | Durable known/detected external-activity facts. An overlapping fact rejects the interval; mere possibility of invisible activity does not reject every interval. |

Evidence provenance must survive a restart and must be correctable without
rewriting a derived estimate. Corrections, unreadable-artifact discoveries,
late Records, and supersession take effect on the next query.

### Series and partition keys

Use these exact logical keys:

```text
series key = (
  source,
  account identity,
  plan identity,
  metering regime,
  limit identity,
  model scope
)

partition key = series key + reset epoch
```

A change to any Series field starts a new Series. A reset starts a new
Partition within the same Series. Window duration remains metadata used by
recency policy; it is not proof of Limit identity.

### Usage membership and time boundary

For consecutive comparable Reading anchors at `t0` and `t1`, Usage Records
must satisfy:

```text
t0 < usage.timestamp <= t1
```

This previous-exclusive/current-inclusive rule includes a Codex token delta
emitted in the same snapshot as the later Reading. It does not claim request
start or completion time.

- A Source-wide Limit includes every Usage Record with the matching Source and
  account identity, including Unattributed Usage.
- A model-scoped Limit includes only Records whose raw Model is explicitly in
  the stored scope set. A potentially matching Unattributed Usage Record
  invalidates the interval. Known nonmatching Models are excluded.
- The same Usage Record may independently participate in different Limits
  whose proven scopes include it. It is counted once within each interval, not
  globally allocated between vendor pools.
- A Usage Record with unknown account identity cannot participate.

### Interval eligibility

An interval is ineligible when any of these applies:

- either endpoint lacks a required identity or provenance fact;
- endpoint ordering is ambiguous;
- reset epoch or any Series identity changes;
- the displayed percentage decreases, does not advance to a later anchor, or
  reaches saturation at 100%;
- completeness coverage does not cover the whole interval;
- known/detected external activity overlaps it;
- its model-scoped membership contains potentially matching Unattributed
  Usage; or
- positive percentage movement has zero matching canonical tokens, which is
  detected non-local activity rather than a zero token conversion.

Repeated Readings at the same displayed percentage do not create zero-ratio
intervals. While evidence stays clean, tokens continue accumulating from the
last positive-movement anchor until the next positive movement. A gap,
identity change, reset, decrease, saturation, or external-activity fact ends
that accumulation; a later clean Reading may start a new run.

### Current Source behavior

| Source / Limit | Attribution contract |
| --- | --- |
| Codex general window | Keep the existing `limit_id == "codex"` filter and canonical duration window key, but persist a stable identity that distinguishes the Codex entitlement and window. Scope is Source-wide, so all matching Codex Usage Records participate. The Reading and same-envelope Usage delta use the later-inclusive boundary. `primary`/`secondary` position is never identity. |
| Claude `five_hour` / `seven_day` | Use a stable raw or documented canonical identity for the source-wide window. All matching Claude Usage Records, including Unattributed Usage, participate. Named response keys are presentation inputs unless the adapter documents their one-to-one identity mapping. |
| Claude model-scoped window | Participate only when the stored vendor scope maps to exact raw Claude Model values. `scope.model.display_name`, a `seven_day_*` key tail, or its slug is not proof. Missing raw identity or mapping makes the current estimate Blocked. |
| Future Source | May participate without estimator changes when it supplies every required fact, exact raw Model mapping where scoped, completeness coverage, and known external-activity provenance. Adding estimation never justifies another vendor call. |

The current v14 history does not contain account identity or historical
completeness. Claude's current model-scoped key also does not prove raw Model
identity. Such evidence remains usable for the existing Limit display but is
not estimator evidence. If a current Source cannot populate the contract from
data it already reads, its estimate truthfully remains Blocked; this feature
does not invent the missing proof.

## Estimator

All estimator percentages below are **displayed percentages**:

```text
p = round(clamp(vendor used_pct, 0, 100))
```

The same rounded `p` is used for movement, saturation, current Used/Left
conversion, and the visible percentage numeral. Invalid non-finite percentages
are rejected as technical data errors rather than coerced.

### Clean runs

A clean run is a maximal monotonic sequence of eligible intervals inside one
Partition.

For each run:

```text
T = sum of matching canonical tokens across the run
d = sum of its positive displayed-percentage movements
  = last displayed percentage - first displayed percentage

run ratio = T / d
```

Retain the count of distinct positive movements, total movement `d`, raw Model
composition, contributor Reading identities, contributor Usage Record
identities, and endpoint-quantization range:

```text
lower = T / (d + 1)
upper = T / (d - 1)   # +infinity when d <= 1
```

The range is a diagnostic for endpoint rounding only. It is not a confidence
interval or user-facing confidence percentage.

### One representative per completed epoch

The active reset epoch never trains an estimate. An epoch is completed when
its reset instant is not later than the single evaluation clock.

Within each completed Partition:

1. form all clean runs;
2. keep runs with at least two distinct positive movements and at least ten
   displayed points of total movement;
3. choose the qualifying run with greatest displayed span as the epoch
   representative; and
4. if two or more qualifying runs tie for greatest span, exclude the epoch as
   `ambiguous-greatest-run`.

An epoch contributes at most one representative.

### Recent candidates

For a Series, inspect no more than its newest five qualifying completed epoch
representatives. A representative is recent when:

```text
epoch ended at >= evaluatedAt - max(7 days, 6 * Limit window duration)
```

Use seven days when duration is missing. At least three distinct recent epochs
are required.

### Unique stable core

For `N` recent candidates, enumerate every subset that:

- has at least three members;
- has at least `ceil(0.75 * N)` members (`3/3`, `3/4`, or `4/5` minimum);
- has a non-empty common intersection of every member's quantization range;
  and
- satisfies `max(run ratio) / min(run ratio) <= 1.25`.

Prefer qualifying subsets with greatest membership. Exactly one subset may
remain at that greatest size. No subset or multiple competing greatest subsets
means Unstable.

Model composition is diagnostic metadata, not another weighting system. The
same intersection and 1.25 spread rules are the sole v1 materiality test for
different Model mixes: compatible mixes may share one core; separated cohorts
produce no unique core and are not averaged.

For the unique stable core, sort its whole-run ratios and take the conventional
median. With an even count, use the arithmetic mean of the two middle ratios.
Retain full floating-point precision internally:

```text
tokensPerPct = median(core run ratios)
```

No pooled ratio, interval median, regression, vendor weight, or former estimate
participates.

## Readiness state machine

Evaluate states in this order:

1. **Blocked** — the current Series/Partition identity, current Reading, or
   Source completeness is unproven.
2. **Ready** — at least three recent candidates have exactly one unique stable
   core.
3. **Unstable** — at least three recent candidates exist but no unique stable
   core qualifies.
4. **Stale** — fewer than three recent candidates qualify, but replaying the
   same current policy over completed historical epochs finds a unique stable
   core that has aged outside the recent horizon.
5. **Gathering** — fewer than three recent candidates qualify and no historical
   Ready core exists under the current policy.

Stale is reconstructed from current canonical evidence; it does not mean a
previous app process happened to observe Ready. For deterministic Stale
classification, evaluate historical epoch-completion clocks newest-first with
the same recency, newest-five, and unique-core rules. Stop at the first Ready
proof or when history is exhausted. Page completed-epoch summaries backwards;
do not load the whole Ledger into memory.

Only Ready carries a number. Contradictory evidence withdraws a former Ready
result immediately as Unstable. There is no grace period. Every state restores
to Ready automatically when current evidence passes again.

The active current percentage never trains the estimate. While Ready, it is
only the multiplier:

```text
usedPct = round(clamp(current used_pct, 0, 100))
leftPct = 100 - usedPct
selectedTokens = tokensPerPct * (mode == Used ? usedPct : leftPct)
```

## Derived ownership and query contract

ADR-0022's boundary is strict: persist facts and derive conclusions.

Persist:

- current canonical Usage Records;
- append-only Limit Readings;
- immutable Reading identities and observation order; and
- identity, scope, completeness, and external-activity provenance.

Do not persist:

- intervals, clean runs, epoch representatives, cores, outliers, readiness
  states, explanations, ratios, selected tokens, or historical Ready flags.

The Rust backend owns the whole derivation in one consistent SQLite read and
uses one injected `evaluatedAt` value. The normal Limits command remains the
single page query and adds one tagged evaluation to each `LimitWindow`.

### Public shape

The generated TypeScript contract is equivalent to:

```ts
type LimitEstimateState =
  | 'ready'
  | 'gathering'
  | 'unstable'
  | 'stale'
  | 'blocked';

interface EstimateEpochSummary {
  epochKey: string;             // privacy-safe diagnostic identity
  endedAt: number;
  movementPoints: number;
  positiveMovements: number;
  inCore: boolean;
  reasonCodes: LimitEstimateReasonCode[];
}

interface LimitEstimateExplanation {
  reasonCodes: LimitEstimateReasonCode[];
  rejections: Array<{
    reasonCode: LimitEstimateReasonCode;
    count: number;
  }>;
  qualifyingEpochs: number;
  requiredEpochs: 3;
  recentCutoffAt: number;
  newestCompletedEpochAt: number | null;
  candidates: EstimateEpochSummary[]; // at most five for the evaluated set
  ratioRange: { min: number; max: number } | null;
  quantizationIntersection: { lower: number; upper: number | null } | null;
}

interface LimitEstimateEvaluationBase {
  state: LimitEstimateState;
  evaluatedAt: number;
  nextEvaluationAt: number | null;
  policyVersion: 'limit-token-estimate-v1';
  explanation: LimitEstimateExplanation;
}

type LimitEstimateEvaluation =
  | (LimitEstimateEvaluationBase & {
      state: 'ready';
      tokensPerPct: number;
    })
  | (LimitEstimateEvaluationBase & {
      state: 'gathering' | 'unstable' | 'stale' | 'blocked';
    });
```

Represent an unbounded quantization upper limit as `null`, never JSON infinity.
`tokensPerPct` must be finite and positive. Do not return pre-rounded
`usedTokens`, `leftTokens`, or a 100% equivalent; the frontend derives them
from the current displayed percentage.

The normal response is bounded to five epoch summaries. Exact contributing
Reading and Usage Record identities remain reconstructible by a separate
backend diagnostic path; v1 requires no additional visible UI or public Tauri
command for that path.

### Reason codes

The backend returns codes and values, never localized prose. V1 supports:

```text
no-current-reading
missing-account-identity
missing-plan-identity
missing-metering-regime
missing-limit-identity
missing-model-scope
unproven-source-completeness
ambiguous-reading-order
identity-change
reset-boundary
percentage-decrease
percentage-saturation
zero-local-usage
known-external-activity
unattributed-model-usage
no-qualifying-run
ambiguous-greatest-run
insufficient-recent-epochs
quantization-ranges-disjoint
ratio-spread-exceeded
competing-stable-cores
historical-core-aged-out
```

An explanation may carry multiple factual codes. `rejections` aggregates every
interval, run, or epoch rejection by reason so the normal payload stays bounded;
the diagnostic path expands those counts to contributor identities. UI state
titles come from the state, not by parsing codes.

### Evaluation timing

Evaluate:

- when the Limits page opens;
- after an ordinary Scan changes relevant Reading, Usage, or provenance facts;
- after a live Reading changes relevant facts; and
- at `nextEvaluationAt` while the page is open.

`nextEvaluationAt` is the earliest future second at which time alone can change
the result: the active epoch reset, or a candidate's recency expiry. A candidate
whose epoch ended at `e` and horizon is `h` expires at `e + h + 1` under the
inclusive recency rule. The frontend runs one local timer and reissues the
ordinary Limits query; it does not fetch a vendor or trigger background
polling.

Start with direct indexed range queries. Index persisted evidence by Series,
epoch/observation time, and Usage Records by the matching Source/account/time
fields (with raw Model available for scoped filtering). Do not scan unrelated
Ledger history per row. Add no cache until profiling demonstrates a need. Any
later disposable memory cache must include complete evidence revision,
`policyVersion`, and expiry in its key.

## Migration and backfill

The schema migration from v14 must:

1. preserve every existing `limit_readings` row and the existing Limits display;
2. add durable storage for the required evidence identity and provenance;
3. retain distinct future equal-percentage observations when they are needed as
   post-gap anchors, while still deduplicating re-ingestion of the exact same
   source observation by immutable Reading identity;
4. make Usage capture account identity nullable so legacy Records remain valid
   Ledger facts but cannot accidentally match estimator evidence;
5. leave all legacy provenance unknown rather than synthesizing account,
   meter, scope, completeness, observation order, or external-activity facts;
6. create no estimate/materialization/history table; and
7. add only the range indexes required by the direct query.

Do not clear `scanned_files` merely to manufacture a token-estimate backfill.
Re-parsing today's artifacts cannot prove historical account identity or
completeness. Eligible stored history participates automatically if and only if
it already carries every required fact. Otherwise Claude and Codex deliberately
cold-start and display Blocked until current identity/completeness is provable,
then Gathering until enough completed epochs qualify.

## UI

Use the approved **Variant A — Quiet evidence line** from the throwaway
[presentation prototype](https://github.com/BrianWong05/TokenLedger/blob/6146fe1/src/limits/limit-token-estimate-presentation.prototype.html).
Add one compact secondary line beneath the existing bar. Preserve the current
percentage numeral, window/reset line, scarcity bar, neutral time tick, card
order, and Left/Used control.

### Ready

Render this hierarchy:

```text
≈12M tokens left · ≈350K / 1% · from 4 consistent completed windows ⓘ
```

- Left/Used changes only the first figure and its `left`/`used` word.
- Tokens per 1% and evidence count do not change with framing.
- Format both figures with the active locale using compact notation and
  approximately two significant digits; prepend `≈` to each.
- Accessible text says “approximately”; it does not rely on the symbol.
- The evidence count is stable-core membership, not all candidate epochs.
- When the Limit is used up, hide the selected Used/Left token figure so the
  row shows neither `≈0 left` nor a prominent 100% figure. Keep tokens per 1%,
  evidence count, and the info control.

The info control is a real keyboard-focusable button with an accessible name,
focus-visible styling, and an explanation available to keyboard, pointer, and
assistive-technology users. Ready explanation:

> Approximation from matching token use across consistent completed Limit
> windows. Local equivalent at 100%: {total} tokens. It is not the vendor's
> token quota.

The 100% local equivalent uses the same locale-aware approximate formatting.

### Withheld states

Replace the whole numeric estimate line with neutral title and factual detail:

| State | Title | Detail |
| --- | --- | --- |
| Gathering | **Not enough data** | `{qualifying} of 3 recent completed windows collected` |
| Unstable | **Estimate withdrawn** | `Recent local history does not form one consistent evidence set` |
| Stale | **Estimate out of date** | `Fewer than 3 qualifying completed windows remain recent` |
| Blocked | **Estimate unavailable** | `Matching local Usage Records or Source completeness cannot be verified` |

No withheld state exposes a prior or diagnostic numeric estimate in visible
copy, accessible copy, tooltip, or popover. Estimate-state styling stays
neutral; success/warning/danger colors remain exclusive to Limit scarcity.

The evidence line wraps below the bar without truncation, horizontal scrolling,
or shrinking the primary percentage. Localized copy may use multiple lines.

### Localization

Add production translation keys rather than concatenating backend prose. Use
locale grammar for the count and Left/Used phrase. The approved Traditional
Chinese meaning is:

```text
Ready origin: 根據 {n} 個一致的已完成時段
Gathering: 資料不足 — 最近需要 3 個時段，目前有 {n} 個
Unstable: 估算已撤回 — 最近的本機歷史並不一致
Stale: 估算已過期 — 最近仍合資格的已完成時段少於 3 個
Blocked: 無法估算 — 無法驗證相符的本機用量或來源完整性
Explanation: 根據多個一致且已完成限額時段的相符 token 用量作近似估算。
             本機 100% 等值：{total} 個 token。這不是供應商提供的 token 限額。
```

Use `Intl.NumberFormat(locale, { notation: 'compact',
maximumSignificantDigits: 2 })` or an equivalent locale-aware helper. Do not
change the existing Overview formatter globally to achieve this row.

## Failure and ambiguity

- Missing or unproven domain evidence yields Blocked, Gathering, Unstable, or
  Stale according to the state machine; it is not a technical error.
- Unexpected SQLite, serialization, arithmetic, or invariant failures reject
  the Limits command through the existing technical error path. They do not
  masquerade as Blocked.
- Non-finite percentages, ratios, or token totals are invariant failures. Zero
  local tokens with positive movement is evidence rejection
  (`zero-local-usage`), not division by zero.
- An expired displayed Reading does not prove a new current Partition. Until a
  current identity-bearing Reading exists, the estimate is Blocked even if the
  v1 Limit bar can render its existing expired-window fallback.
- A current 100% Reading may still use a Ready historical `tokensPerPct`; the
  row follows the used-up presentation rule and hides the selected equivalent.
- Plan, account, meter, Limit, or scope changes never carry an estimate forward;
  the new Series starts from its own evidence.
- Unexpected vendor fields remain opaque display data unless an adapter
  documents their evidence identity. No fuzzy or label-derived fallback is
  permitted.

## Acceptance tests

An implementation is complete when automated tests prove all of the following.

### Evidence and migration

1. V14 migration preserves current Limit rows and display behavior.
2. Legacy Readings and Usage Records with missing provenance never match and
   yield Blocked, not zero or a backfilled estimate.
3. Exact source re-ingestion deduplicates one Reading identity, while a later
   equal-percentage observation can remain a distinct post-gap anchor.
4. Account, plan, meter, Limit, epoch, or scope changes separate evidence.
5. Source-wide membership includes Unattributed Usage; model-scoped membership
   includes exact mapped raw Models, excludes known nonmatches, and rejects a
   potentially matching Unattributed Record.
6. The `(t0, t1]` boundary includes a same-snapshot Codex delta at `t1` and
   excludes a Record at `t0`.
7. A later Record upgrade, replacement, or coarse-to-fine supersession changes
   the next evaluation without a persisted estimate invalidation step.
8. Completeness corrections and known external-activity facts withdraw affected
   intervals on the next evaluation.

### Estimator and readiness

9. Canonical token sums include Input, Output, Cache Read, and both Cache Write
   buckets exactly once and exclude reasoning/context/API-call fields.
10. Duplicate displayed percentages accumulate tokens to the next positive
    movement and never create a zero-movement ratio.
11. Reset, decrease, saturation, incomplete coverage, external activity, and
    zero local usage end or reject runs as specified.
12. A run with `T = 1_000_000` and `d = 10` has ratio `100_000` and
    quantization range `[1_000_000/11, 1_000_000/9]`.
13. A run needs at least two positive movements and ten total points; an epoch
    chooses its unique greatest-span run and rejects a greatest-span tie.
14. The active epoch never trains; the recency cutoff uses
    `max(7 days, 6 * duration)` and missing duration uses seven days.
15. Stable-core fixtures cover `3/3`, `3/4`, and `4/5`, disjoint quantization
    ranges, ratio spread just below/above `1.25`, a unique largest subset, and
    competing largest subsets.
16. Median fixtures cover odd and even core sizes, with the even median equal to
    the mean of the middle two whole-run ratios.
17. Ready converts only the current displayed Used/Left percentage; changing the
    toggle does not retrain or requery the estimator.
18. State-precedence fixtures cover Blocked over all states, Ready, Unstable,
    reconstructed Stale, Gathering, immediate withdrawal, and automatic
    restoration.
19. `nextEvaluationAt` chooses the earliest active reset or inclusive-cutoff
    expiry and a timer reevaluation makes no vendor call.

### Query, UI, localization, and accessibility

20. Every `LimitWindow` returns exactly one tagged evaluation with one
    `evaluatedAt`; only Ready serializes a finite positive `tokensPerPct`.
21. The normal response contains no more than five candidate summaries and no
    Usage Record identity list, includes all rejection reasons as bounded
    counts, and lets diagnostics reconstruct the exact contributors.
22. Ready renders the quiet line below the existing bar with two approximate,
    locale-aware, roughly two-significant-digit figures and the core epoch count.
23. Left/Used changes only the selected equivalent; a used-up row hides that
    equivalent but retains per-1%, evidence, and info.
24. Gathering, Unstable, Stale, and Blocked render the approved neutral copy and
    expose no numeric estimate in the DOM or accessible name/description.
25. The Ready info control is operable by keyboard and pointer, has visible
    focus, exposes the same assistive explanation, labels the optional 100%
    figure as a local equivalent, and states it is not the vendor quota.
26. English and Traditional Chinese tests cover Ready and every withheld state,
    compact large values, plural/count interpolation, and wrapped narrow-card
    layout without truncation or horizontal scrolling.

## Implementation handoff

The smallest correct implementation extends the existing Limits query and row,
adds one pure backend estimator seam with deterministic fixtures, and adds only
the evidence provenance storage that the contract requires. It does not need a
new frontend store, endpoint, polling loop, estimator service, cache, or durable
aggregate.
