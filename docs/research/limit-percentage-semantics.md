# What vendor Limit percentages measure

**Status:** researched 2026-08-12

**Scope:** Claude subscription usage limits and Codex ChatGPT-plan rate limits

**Question:** can a vendor-reported utilization percentage be converted into TokenLedger's canonical token total?

## Conclusion

No vendor-published formula permits an exact conversion from either percentage to raw tokens. Claude exposes a proprietary utilization measure whose consumption changes with model, effort, context, tools, features, and caching. Current Codex plans are documented in credit-weighted token terms, with weights that differ by model and token category, but the rate-limit payload does not expose its numerator, denominator, or unit. Some Codex Enterprise workspaces also remain on a legacy message-based meter.

Therefore TokenLedger can estimate only a **local, historical canonical-token equivalent per percentage point** for a sufficiently stable workload. It must not present that estimate as the vendor's token quota or as a universal conversion.

TokenLedger's canonical total is an unweighted sum of input, output, cache-read, and cache-write tokens ([implementation](../../src-tauri/src/queries.rs#L242-L256)). Limit readings intentionally retain the vendor's percentage without converting it and do not enter the Ledger ([data type](../../src-tauri/src/types.rs#L38-L56)). That separation is correct: the vendor meters described below are not the same quantity as the canonical total.

## Evidence standard

“Proven” below means stated in official vendor documentation, represented by first-party source/schema, or present in a captured first-party response in this repository. “Strong inference” means the sources support the interpretation but do not state the exact relationship. “Unknown” means the evidence does not support a reliable conclusion. A captured response establishes an observed shape, not a stability guarantee.

## Claude

### Proven facts

1. **The drain is workload-dependent, not a fixed request count.** Anthropic describes usage as a conversation budget and says consumption varies with message and conversation length, attachments, model, effort, tools, and features. Claude Code also sends conversation history and project context on each turn; Opus consumes meaningfully more quota than Sonnet. ([Usage and length limits](https://support.claude.com/en/articles/11647753-how-do-usage-and-length-limits-work), [Claude Code models, usage, and limits](https://support.claude.com/en/articles/14552983-models-usage-and-limits-in-claude-code), [usage-limit best practices](https://support.claude.com/en/articles/9797557-usage-limit-best-practices))

2. **Token categories cannot be assumed equal.** Anthropic says reused cached project content does not count against limits and only new or uncached project content counts. Thus a canonical total that includes cache reads cannot have a workload-independent proportional relationship to Claude utilization. ([Usage-limit best practices](https://support.claude.com/en/articles/9797557-usage-limit-best-practices))

3. **The pool spans surfaces.** Activity in Claude, Claude Code, and other Claude surfaces can share the same subscription allowance. A utilization change can therefore occur without corresponding usage in TokenLedger's locally observed artifacts. ([Usage and length limits](https://support.claude.com/en/articles/11647753-how-do-usage-and-length-limits-work), [usage credits](https://support.claude.com/en/articles/12429409-manage-usage-credits-for-paid-claude-plans))

4. **Windows and plan allowances differ.** The session allowance resets after five hours; weekly limits reset at an account-assigned time. Anthropic may apply additional weekly, monthly, model, or feature limits, and the included allowance differs between plans such as Pro and Max. ([Pro plan](https://support.claude.com/en/articles/8325606-what-is-the-pro-plan), [Max plan](https://support.claude.com/en/articles/11049741-what-is-the-max-plan))

5. **The first-party schema describes utilization, not tokens.** Anthropic's Agent SDK defines five-hour, seven-day, model-scoped seven-day, and overage limit types. Its `utilization` is a consumed fraction from 0.0 to 1.0; a separate status can be allowed, warning, or rejected, and reset time is separate. The schema contains no token numerator, denominator, or category weights. ([Agent SDK rate-limit types](https://github.com/anthropics/claude-agent-sdk-python/blob/be2d0dfbd9ee884ff43efd44e5a3158aa09a6a34/src/claude_agent_sdk/types.py#L1361-L1403))

6. **The live response has both general and scoped windows but still no unit.** A first-party `/api/oauth/usage` response captured by TokenLedger contains legacy `utilization` fields and a newer `limits[]` list with `kind`, `percent`, reset, activity, severity, and optional model scope. `extra_usage` and `spend` are separate. It provides no token numerator, denominator, or weights ([captured response](../../src-tauri/src/bin/claude-limits.rs#L381-L403); [parser](../../src-tauri/src/bin/claude-limits.rs#L286-L320)).

7. **One observed display path is quantized.** The newer captured response uses integer `percent`, while the legacy form and Agent SDK use floating-point utilization. This proves that available readings can differ in precision, but not how the server rounds all responses.

8. **100% is not necessarily the end of all service.** Reaching the included-plan allowance can reject included usage, while separately purchased usage credits can continue at API rates. Overage and utilization are distinct in the first-party schema. ([Usage credits](https://support.claude.com/en/articles/12429409-manage-usage-credits-for-paid-claude-plans), [Agent SDK rate-limit types](https://github.com/anthropics/claude-agent-sdk-python/blob/be2d0dfbd9ee884ff43efd44e5a3158aa09a6a34/src/claude_agent_sdk/types.py#L1361-L1403))

### Strong inferences

- Claude's percentage is a proprietary, token-derived quota utilization with effective weighting or exclusions for model, effort, feature, and cache behavior. It is not an unweighted raw-token percentage.
- A request-count interpretation is also inadequate: Anthropic explicitly says messages and turns vary in consumption.
- A locally measured canonical-tokens-per-point ratio may be useful only while the workload mix, plan, account, scope, and window remain comparable.

### Unknowns

- The quota denominator and its unit.
- Exact input, output, cache-read, and cache-write treatment.
- Exact model, effort, tool, and feature multipliers.
- Whether public API prices correspond to subscription-limit weights.
- Server-side rounding, dynamic capacity adjustments, promotions, experiments, or account-specific factors.
- Whether every future scoped limit can be mapped reliably to a model in local usage data.

## Codex

### Proven facts

1. **Current included usage is weighted by model and token category.** OpenAI's current Codex rate card translates input, cached-input, and output tokens into credits at different rates for each model. Output is much more expensive than input, cached input is cheaper, and the rates differ across models. Fast mode and image generation consume included limits faster. Codex does not charge cache writes in this rate card. ([Codex pricing](https://developers.openai.com/codex/pricing), [Codex rate card](https://help.openai.com/en/articles/20001106))

2. **Not every account uses the same meter.** OpenAI documents a small legacy Enterprise cohort that still uses approximate message-based credit rates, while migrated workspaces use token-based rates. Plan allowances also differ. ([Codex pricing](https://developers.openai.com/codex/pricing), [Codex rate card](https://help.openai.com/en/articles/20001106))

3. **Task consumption is variable.** Model, task size and complexity, context, reasoning, tools, retrieval, caching, and local versus cloud execution affect usage; similar tasks can consume different amounts. Prompt length alone is not a reliable predictor. ([Codex pricing](https://developers.openai.com/codex/pricing))

4. **The pool spans surfaces.** Codex allowance can be shared with ChatGPT Work, Excel, Workspace Agents, local tasks, and cloud tasks. Consequently a percentage change can include activity absent from TokenLedger's local logs. ([Using Codex with a ChatGPT plan](https://help.openai.com/en/articles/11369540-using-codex-with-your-chatgpt-plan), [Codex pricing](https://developers.openai.com/codex/pricing))

5. **First-party rate-limit structures expose percentage and time, not the denominator.** The Codex protocol represents a limit ID/name, primary and secondary windows, credits, plan type, and reached state. A window contains `used_percent`, duration, and reset timing. Credits and spend controls are separate objects; no window field supplies a token or credit numerator/denominator. ([Protocol types](https://github.com/openai/codex/blob/16fbfe557446a1af94da81e1144029ccc1311ad0/codex-rs/protocol/src/protocol.rs#L2165-L2227))

6. **Several opaque limit families can coexist.** Rate-limit headers are parsed dynamically by family name, and backend responses can contain `additional_rate_limits` with a metered feature and optional display name. A duration alone is not a safe identity for a pool. ([Header/event parser](https://github.com/openai/codex/blob/16fbfe557446a1af94da81e1144029ccc1311ad0/codex-rs/codex-api/src/rate_limits.rs#L60-L178), [endpoint test](https://github.com/openai/codex/blob/16fbfe557446a1af94da81e1144029ccc1311ad0/codex-rs/app-server/tests/suite/v2/rate_limits.rs#L135-L183))

7. **Precision depends on the path.** Per-response headers/events accept floating-point percentages, including a first-party test fixture of `12.5`. The generated backend usage schema uses an integer. The app-server converts a floating-point internal value to an integer with `round()`. A polled whole-number reading is therefore quantized and must not be treated as an exact boundary. ([Parser and fixture](https://github.com/openai/codex/blob/16fbfe557446a1af94da81e1144029ccc1311ad0/codex-rs/codex-api/src/rate_limits.rs#L196-L238), [backend schema](https://github.com/openai/codex/blob/16fbfe557446a1af94da81e1144029ccc1311ad0/codex-rs/codex-backend-openapi-models/src/models/rate_limit_window_snapshot.rs#L16-L25), [app-server conversion](https://github.com/openai/codex/blob/16fbfe557446a1af94da81e1144029ccc1311ad0/codex-rs/app-server-protocol/src/protocol/v2/account.rs#L610-L624))

8. **100% is a saturation point, not always an immediate service stop.** OpenAI says an active turn may finish after the limit is reached and purchased credits may permit more work. The protocol also separates percentage, credits/spend, and reached reason; image-generation handling uses `used_percent >= 100` only for that specific limit family's reset message. ([Codex pricing](https://developers.openai.com/codex/pricing), [protocol types](https://github.com/openai/codex/blob/16fbfe557446a1af94da81e1144029ccc1311ad0/codex-rs/protocol/src/protocol.rs#L2165-L2227), [image-limit handling](https://github.com/openai/codex/blob/16fbfe557446a1af94da81e1144029ccc1311ad0/codex-rs/ext/image-generation/src/tool.rs#L252-L277))

9. **Reset schedules can change.** OpenAI's banked-reset feature resets both five-hour and weekly allowances immediately and moves the weekly reset time. A nominal duration is insufficient to identify a continuous epoch; `resets_at` must be observed. ([Using Codex with a ChatGPT plan](https://help.openai.com/en/articles/11369540-using-codex-with-your-chatgpt-plan))

### Strong inferences

- For migrated accounts, the main Codex percentage most plausibly tracks a credit-weighted included allowance, because official documentation connects the same model/category rates and feature multipliers to included-limit consumption. The endpoint schema does **not** explicitly state `used_percent = credits used / included credits`, so that equation is not proven.
- Raw canonical tokens cannot be proportional across heterogeneous workloads: the documented weights for fresh input, cached input, output, model, and mode differ materially.
- Integer readings interval-censor utilization. An unchanged percentage does not prove zero consumption, and a one-point change does not identify the exact token amount at the transition.

### Unknowns

- The included-limit denominator and whether it is expressed internally in public credits.
- Whether every plan and cohort follows the currently published rate card exactly.
- Server rounding before client conversion, dynamic quota changes, promotions, capacity effects, and experiments.
- The meaning of an opaque additional limit unless the server supplies a trustworthy identity and scope.
- How cache writes or other categories not charged by the public rate card affect every internal subscription meter.

## Cross-vendor interpretation

| Question | Claude | Codex |
|---|---|---|
| Raw canonical-token percentage? | No evidence; caching and model behavior contradict a universal raw-token ratio. | No; documented category/model credit weights contradict it. |
| Request/message percentage? | No; requests vary with context, model, effort, tools, and features. | Not for current migrated accounts; a legacy Enterprise cohort is message-based. |
| Credit/weighted unit? | Strong inference only; exact latent unit is undisclosed. | Strong inference for the percent itself; token-to-credit weighting is documented. |
| Denominator exposed? | No. | No. |
| Precision | Float utilization exists; observed modern percent is integer; server rounding unknown. | Float headers/events exist; polled/backend/app paths can be whole-number and rounded. |
| 100% | Included allowance exhausted/rejected; overage can be separate. | Included window saturated; active work or separately purchased credits may continue. |

The operationally important result is not merely that the denominator is hidden. The numerator is also unlike TokenLedger's canonical total: it is affected by factors that TokenLedger either deliberately leaves unweighted or may not observe at all.

## Constraints for TokenLedger's Limit Token Estimate

1. **Name the output honestly.** Label it “approximate local token equivalent” (or equivalent), use `≈`, and never call it the vendor quota, exact token limit, or authoritative tokens per percent.

2. **Keep canonical tokens unweighted.** The estimate may answer the product question in TokenLedger's canonical total, but it must be described as an empirical workload-dependent conversion. Do not silently import today's vendor credit prices as canonical-token weights.

3. **Fit narrowly.** Partition evidence at least by Source, account/workspace when available, plan or meter regime, vendor limit ID/window key, reset epoch, and model scope. Separate materially different modes such as Codex fast mode. Never combine Claude and Codex evidence.

4. **Respect scope.** Use a model-scoped limit only with local usage confidently matched to that model. If identity or mapping is ambiguous, withhold the estimate rather than fall back to all usage.

5. **Pair observations within one epoch.** Compute local canonical-token deltas only between successive readings with the same reset identity. Exclude intervals that cross a reset, change plan/account/limit identity, regress unexpectedly, or have incomplete Source ingestion. Align tokens by observation time and avoid requests that straddle boundaries when possible.

6. **Treat percentages as interval-censored.** Do not infer a slope from an unchanged whole-number reading or one isolated transition. Require multiple monotonic percentage movements, use robust aggregation, and report uncertainty from the observed spread rather than pseudo-precise confidence.

7. **Detect hidden-pool contamination.** Reject an interval when percentage rises with zero matching local tokens. Even when local tokens exist, simultaneous activity on another vendor surface remains unknowable; require consistency across several intervals and withhold when unexplained variation is too large.

8. **Do not learn through saturation.** Exclude 100% and rejected/overage intervals from slope fitting. At 100%, show included-window exhaustion separately; do not infer overall service unavailability or use subsequent tokens to enlarge the included quota estimate.

9. **Expire stale evidence.** Refit after a reset, plan/cohort change, account change, limit-family change, model/category-mix shift, or material vendor pricing/meter revision. Store the observation source and meter/version date with the estimate.

10. **Withhold when evidence is insufficient.** No estimate is preferable to a numeric answer derived from a single quantized transition, unknown model scope, a mixed or legacy meter, hidden shared-pool activity, or unstable token/category mix.

Under these constraints, a displayed value can answer: “For this account, window, and recent workload mix, how many TokenLedger canonical tokens have historically corresponded to one reported percentage point?” It cannot answer: “How many raw tokens does the vendor allow?”
