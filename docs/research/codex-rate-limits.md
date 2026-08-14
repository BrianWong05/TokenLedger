# Codex rate-limit evidence

Status as of 2026-08-12: Codex writes a complete rolling-window quota reading
into the Session Artifact the scan already walks, once per request. The block is
usable for a live Limits card without any authenticated call (map decision 10),
but three of its fields do not mean what their names suggest, and the figure's
worth is dominated by *when it was observed* rather than by what it says.

Evidence base: 214 `rollout-*.jsonl` files under `~/.codex/sessions/` on one
genuine macOS installation, spanning 2026-02-23 to 2026-08-10. 212 of the 214
carry at least one `rate_limits` block; together they hold **22,240
observations** written by 15 distinct recorded `cli_version` values (0.61.0
through 0.147.0-alpha.6.5). Every count below is from that corpus. The two files
with no block are Sessions that ended before their first Model turn — 2 and 9
lines long — so **every Session that made a request recorded its limits**.

Semantics are confirmed against Codex's own source, read at
`openai/codex@279b93242cfef379e65da97e87e44b83c5934fd7` (main, 2026-08-11).
Where the logs and the source agree, the finding is settled; the two places they
diverge are called out.

Corroboration also comes from four prior-art projects (orca, openusage,
TokenTracker, tokscale). Their agreement is load-bearing in one place and
actively misleading in two, both flagged below. **None of the four reads this
block**: all four fetch Codex limits from `GET
https://chatgpt.com/backend-api/wham/usage`, a differently-shaped payload
(`rate_limit`, `primary_window`, `limit_window_seconds`, `reset_at`). Where the
logs and the prior art disagree, the logs win.

## Where the block lives

All 22,240 observations sit at exactly one JSON path — `.payload.rate_limits`,
on records with `type == "event_msg"` and `payload.type == "token_count"`. It is
a sibling of the `payload.info` token counts the Codex adapter already parses,
so the scan walks past every one of these for free.

Two path traps, both measured:

- The envelope `timestamp` (ISO-8601, UTC) is the only trustworthy ordering key.
  The **filename date is local time**: 123 of 212 files have a name-date that
  differs from their first observation's UTC date.
- Within a file, the last `rate_limits` line is also the newest by timestamp in
  **212 of 212** files, so last-line-per-file is a safe read. Across files it is
  not — pick the maximum envelope timestamp.

## What Codex's own source says

`RateLimitSnapshot` and `RateLimitWindow` live in
`codex-rs/protocol/src/protocol.rs:2155-2226`. Every field is `Option`, and
**not one field carries a serde attribute** — no `alias`, no `rename`, no
`default`, no `skip_serializing_if`. So a writer emits all nine snapshot keys and
all three window keys including nulls, while a reader tolerates missing keys
because `Option` deserialises from absent. That is exactly the shape the logs
show.

```rust
pub struct RateLimitWindow {
    /// Percentage (0-100) of the window that has been consumed.
    pub used_percent: f64,
    /// Rolling window duration, in minutes.
    pub window_minutes: Option<i64>,
    /// Unix timestamp (seconds since epoch) when the window resets.
    pub resets_at: Option<i64>,
}
```

The values originate in **HTTP response headers**, not a JSON body —
`x-codex-primary-used-percent`, `x-codex-primary-window-minutes`,
`x-codex-primary-reset-at`, the matching `-secondary-` trio,
`x-codex-credits-{has-credits,unlimited,balance}`,
`x-codex-rate-limit-reached-type`, and `x-codex-active-limit`
(`codex-rs/codex-api/src/rate_limits.rs`). The prefix is `x-{limit_id}`, which is
how more than one limit family can be reported at once.

One parser quirk worth knowing: a window is discarded unless `used_percent`
parses **and** (`used_percent != 0` or `window_minutes != 0` or `resets_at` is
present). A genuinely 0%-used window carrying nothing else is dropped rather than
reported as zero.

Three things the CLI does with this block are directly reusable, because they are
the vendor's own answers to the questions this card has to settle:

- It labels each window **from `window_minutes`, with a ±5% tolerance** —
  300 → "5h", 1440 → "daily", 10080 → "weekly", 43200 → "monthly",
  525600 → "annual" (`get_limits_duration`).
- It renders **one row per window that exists**, never a placeholder for a
  `null` one, and states percentages as **`N% left` (100 − `used_percent`)**.
- It has its own staleness model: `RATE_LIMIT_STALE_THRESHOLD_MINUTES = 15`, and
  a four-state enum — `Available` / `Stale` / `Unavailable` / `Missing`
  (`codex-rs/tui/src/status/rate_limits.rs`).

## The charted sample is narrower than the real block

The block recorded while charting (#103) has five fields. Live blocks carry up
to nine. Observed key sets, all 22,240 observations:

| Fields present | Observations |
| --- | --- |
| `credits` `individual_limit` `limit_id` `limit_name` `plan_type` `primary` `rate_limit_reached_type` `secondary` `spend_control_reached` | 12,813 |
| the same without `spend_control_reached` | 8,779 |
| the same without `spend_control_reached` and `individual_limit` | 440 |
| `credits` `limit_id` `limit_name` `plan_type` `primary` `secondary` | 208 |

The window objects themselves never vary: **`resets_at`, `used_percent`,
`window_minutes` in all 24,977 window objects**, no other spelling and no other
field.

## `primary` versus `secondary`: the slot is not the window

This is the central finding, and it contradicts the naive reading.
`window_minutes` takes only two values in the whole corpus — 300 (5 hours) and
10080 (7 days) — and they appear in exactly two arrangements:

| `primary.window_minutes` | `secondary.window_minutes` | Observations | Sessions |
| --- | --- | --- | --- |
| 300 | 10080 | 2,742 | 60 of 212 |
| 10080 | `null` | 19,493 | 152 of 212 |
| `null` | `null` | 5 | 4 (all `limit_id: "premium"`) |

So **`primary` carries the 7-day window in 88% of this corpus and the 5-hour
window in the rest**. A card that labels `primary` "5h" is wrong for most of the
data on this machine, and a card that labels it "Weekly" is wrong for the
earlier third. The slot is a position, not a meaning: **classify on
`window_minutes`**.

`secondary` is only ever the 7-day window or `null`. The 5-hour window never
appears in `secondary` here.

**Codex's own source settles this, and it agrees.** The slots carry no duration
meaning at all:

- `limit_label_for_window(window_minutes, is_secondary)` derives the label from
  the duration, and falls back to the slot **only** when `window_minutes` is
  absent — at which point it refuses to name a duration, labelling the slots
  `"usage"` and `"secondary usage"`
  (`codex-rs/tui/src/chatwidget/rate_limits.rs:11-12,77-114`).
- The status line's `five_hour_status_window` explicitly searches **both** slots
  by duration and has a dedicated branch for a weekly `primary`, pinned by a test
  literally named `status_line_shows_secondary_non_weekly_when_primary_is_weekly`
  (`codex-rs/tui/src/chatwidget/status_surfaces.rs:999-1085`).
- The header names (`x-codex-primary-*`, `x-codex-secondary-*`) are symmetric and
  semantically empty.

So the weekly-in-`primary` shape is a normal, expected payload that the vendor's
own client is written to handle, not an anomaly. `window_minutes` may also take
1440, 43200, and 525600, which this corpus never saw — so a reader must handle a
duration it does not recognise rather than assuming a third value is corrupt.

The changeover is clean, and that is itself a problem. The last two-window
observation is `2026-07-11T10:04:28.121Z` on `cli_version` 0.144.0-alpha.4; the
first one-window observation is `2026-07-13T15:34:08.897Z` on 0.144.2. **Zero
observations of either shape fall on the wrong side of that boundary**, and no
single version ever emitted both shapes. Version and wall-clock are collinear
across the switch, so the logs cannot *prove* whether the client changed what it
records or the account's entitlement changed server-side.

Two measurements nonetheless put the weight on server-side:

- In the one-window era `secondary` is **present and explicitly `null` in all
  19,493 observations** — the key is never absent. The client still serialises
  the field; something upstream is saying there is no second window.
- Three versions each emitted **more than one `credits` state**, on overlapping
  dates: 0.144.2 wrote `credits: null` 5,765 times and `balance: null` 915
  times; 0.142.5 wrote `credits: null` 1,012 times and `balance: "0"` 36 times
  across the same four days; 0.144.0-alpha.4 produced all three states in two
  days. A fixed client cannot vary its own output that way, so **the server
  demonstrably drives field population in this block.**

That does not settle the window question — no version straddles the boundary —
but a reader should treat the shape as server-controlled and version-independent,
and must not assume that pinning a CLI version pins the shape.

Prior art offers an explanation that **does not fit this machine**. TokenTracker
(`src/lib/usage-limits.js:229-232`) comments that "Free-tier accounts only get a
weekly window, often delivered in the `primary_window` slot", and openusage
(`Sources/OpenUsage/Providers/Codex/CodexUsageMapper.swift:117-120`) that Codex
"can move a temporarily sole weekly limit into the primary slot". Both describe
the shape observed here, but the cause does not transfer: `plan_type` is
`"plus"` on **21,978 of 22,240** observations, including every one of the 19,493
weekly-only ones. A paid Plus account produced the sole-weekly shape. Whatever
drives the collapse to one window, it is not the free tier.

Where prior art *is* worth copying is the classifier. orca's
`src/main/rate-limits/codex-rate-limit-window-classification.ts` already works
in minutes rather than seconds, keys off the duration, and keeps the positional
mapping only as a fallback for durations it does not recognise, with a ±1 minute
tolerance for upstream `Math.ceil` drift. It ports to this block with a
snake-case rename. tokscale is the anti-pattern: it labels each slot from its
own duration but never reorders, so its rows come out `["Weekly", "5h"]` for a
swapped payload (`crates/tokscale-cli/src/commands/usage/codex.rs:2205-2233`).

## `limit_id` and `limit_name`

`limit_name` is **`null` in 22,240 of 22,240 observations**, and the source says
that is by design rather than by accident: it is **always `None` for the default
`codex` family** on every ingestion path — the header path reads
`x-{limit}-limit-name` only for named families, the event path hardcodes
`limit_name: None`, and the `/usage` path passes `None` for the main bucket. When
it *is* populated it holds a **Model slug**, not a plan; the repo's own fixture is
`x-codex-bengalfox-limit-name: gpt-5.2-codex-sonic`
(`codex-rs/codex-api/src/rate_limits.rs:330-346`), and the CLI uses
`limit_name ?? limit_id` as a row prefix, suppressed when it equals `"codex"`. On
a single-family account it will never carry anything, so nothing can be built on
it. Prior art agrees: its populated `limit_name` values (`"GPT-5.3-Codex-Spark"`)
live in the wham API's `additional_rate_limits[]`, which has no counterpart here.

`limit_id` takes two values here: `"codex"` (22,235) and `"premium"` (5). The plan
label lives in `plan_type`, not here — `"plus"` on 21,978, `null` on 262 (the
262 are the 0.104.0-era Sessions and part of 0.144.0-alpha.4). `plan_type` is a
**closed vocabulary** in the source, which makes it safe to map to display names:
`free`, `go`, `plus`, `pro`, `prolite`, `team`, `self_serve_business_prolite`,
`self_serve_business_usage_based`, `business`, `ent26`,
`enterprise_cbp_automation`, `enterprise_cbp_usage_based`, `enterprise`, `edu`,
and an `#[serde(other)] Unknown` catch-all
(`codex-rs/protocol/src/account.rs:12-38`).

The five `premium` records matter more than their count. Every one carries
`primary: null` and `secondary: null` — no windows at all:

```json
{"limit_id":"premium","limit_name":null,"primary":null,"secondary":null,
 "credits":{"has_credits":false,"unlimited":false,"balance":"0"},
 "individual_limit":null,"plan_type":"plus","rate_limit_reached_type":null}
```
`sessions/2026/07/03/rollout-2026-07-03T13-44-17-…jsonl` line 1176,
`2026-07-03T10:16:42.271Z`, `cli_version` 0.142.5.

All five sit on a turn that ends immediately — `task_complete` fires on the very
next line, with no reasoning, tool call, or Model output between. In the sample
above the preceding line is a `codex` reading at `used_percent: 100.0`, and ten
lines later the agent writes "The reviewer subagent hit the account usage limit,
so I can't get a fresh delegated review right now." So `premium` looks like a
second, separate entitlement that Codex reports in place of the main one when a
request is refused against it, and it reports no percentage for it.

The Model hypothesis is refuted by the logs: `codex-auto-review` accounts for 676
`codex` observations and only 2 `premium` ones, and `gpt-5.5`, `gpt-5.4`, and
`gpt-5.6-sol` all appear under both `limit_id` values.

**The source explains it exactly, and it matches the logs.** On a 429 the server
sends `x-codex-active-limit: premium`; the CLI then calls
`parse_rate_limit_for_limit(headers, Some("premium"))`, which builds the prefix
`x-premium` and looks for `x-premium-primary-used-percent` and friends. Those
headers do not exist, so both windows resolve to `None` — and unlike the
additional-families path, which filters through `has_rate_limit_data`, that
function returns `Some(RateLimitSnapshot { … })` **unconditionally**
(`codex-rs/codex-api/src/rate_limits.rs:45-48,57-101`). The empty snapshot then
flows through the normal merge into a `TokenCount` event.

So **a `premium` record is the fingerprint of a refused request** — a 429 against
a limit family whose usage the server does not report. That is precisely
consistent with all five landing on turns that `task_complete` immediately, and
with the agent's own "hit the account usage limit" message. `"premium"` itself
appears nowhere in Codex's source; it is a pure server-supplied string, so the
value space of `limit_id` is open (the repo's own fixtures use `codex`,
`codex_other`, `codex_secondary`, `codex_bengalfox`).

Two consequences, both actionable:

- **Filter to `limit_id == "codex"`.** Taking "the newest observation" unfiltered
  will land on an all-`null` `premium` record and blank a gauge that has perfectly
  good data one line earlier. The merge is what makes this safe to do — it
  backfills `credits`, `individual_limit`, `spend_control_reached`, and
  `plan_type` from the previous snapshot but **never `primary`/`secondary`**
  (`codex-rs/core/src/state/session.rs:325-345`), so a `premium` record's null
  windows are genuinely absent rather than stale copies.
- A Session holds a **single** `latest_rate_limits` slot, so when several limit
  families report, the last one emitted wins and `limit_id` can flip between
  consecutive records. Do not assume a Session's records all describe one family.

## `credits`

Three states occur, and they drift by version rather than by anything the user
did:

| `credits` | Observations | Era |
| --- | --- | --- |
| `{"has_credits":false,"unlimited":false,"balance":"0"}` | 12,894 | 2026-07 onward, universal by 2026-08 |
| `null` (the whole object) | 8,170 | 2026-04 through 2026-06, mixed in 2026-07 |
| `{"has_credits":false,"unlimited":false,"balance":null}` | 1,176 | 2026-02 (0.104.0) and part of 2026-07 |

`has_credits` is `false` and `unlimited` is `false` on **every** observation, and
`balance` is never anything but `"0"` or `null`, so the logs alone cannot say what
it measures.

**The source settles the units: they are Codex credits, not currency.** `balance`
is `Option<String>`, a decimal string carried verbatim end-to-end and documented
as "Raw balance text as provided by the backend". The CLI parses it as `i64` then
`f64`, rounds, requires `> 0`, and renders `"{n} credits"` — no `$`, no
conversion (`codex-rs/tui/src/status/rate_limits.rs:360-416`). The sibling
`individual_limit` (workspace spend controls) renders in the same unit:
`"Monthly credit limit: … 8,000 of 25,000 credits used"`.

The gating precedence is the part that matters for the card:

1. `unlimited: true` → `Credits: Unlimited`, and it **wins even when
   `has_credits` is false**.
2. `has_credits: false` → **the row is omitted entirely.**
3. Otherwise a balance that rounds above zero renders as a count; `null`, `""`,
   `"0"`, or unparseable renders as `"Available"`.

So the state seen throughout this corpus —
`{"has_credits":false,"unlimited":false,"balance":"0"}` — renders **nothing at
all** in Codex's own `/status`. That is the vendor's own answer to the question of
whether it is a bar, a footnote, or nothing.

Prior art is split four ways and one of its choices is now clearly an assumption
rather than a fact: openusage prices credits at **1 credit = $0.04**
(`CodexUsageMapper.swift:11-13`), which Codex itself does not do. It does at least
agree on presentation, rendering two scalars and never a bar because there is no
cap to draw against. tokscale passes the value through opaquely; orca and
TokenTracker ignore `credits` entirely, TokenTracker's credit bar coming from
`spend_control.individual_limit` instead. One caution from the fixtures: `balance`
is a **string** here and in one openusage fixture but a **number** in another
(`Tests/OpenUsageTests/CodexProviderTests.swift:198-206` versus `:469`), so parse
it as a scalar of unknown JSON type even though the Rust type says `String`.

## Schema drift

The charted premise that "one prior-art fixture used `used_percentage` rather
than `used_percent`" does not survive checking. **`used_percentage` occurs zero
times in this corpus**, across all 24,977 window objects and all 15 versions.
The prior-art fixture carrying that spelling is orca's *Claude* statusline
payload (`src/shared/claude-statusline-rate-limits.test.ts:17-18`), not a Codex
one; Codex is uniformly `used_percent` in all four projects. The source closes it
for good: `git log -S'used_percentage' --all` over a full clone returns nothing,
and the struct carries no serde attributes, so there is no alias either. **Do not
build a `used_percentage` alias for Codex.**

**The rename that does matter is `resets_at`, and this corpus is too young to
show it.** Codex's history has a real relative-to-absolute switch:

| Version | Window reset field |
| --- | --- |
| ≤ 0.47.x | `resets_in_seconds` — **relative** seconds, recalculated against "now" |
| 0.48.0 | `resets_at`, briefly an RFC3339 **string** in pre-release builds |
| ≥ 0.48.0 | `resets_at` — absolute Unix seconds, `Option<i64>` |

The commit that switched it (`0e08dd6055`, 2025-10-17) gives the reason plainly:
"Previously these got recalculated relative to current time, which leads to the
displayed reset times to change over time, including after doing a `codex
resume`." Codex still ships a migration for the string form,
`normalize_legacy_rate_limit_resets`
(`codex-rs/thread-store/src/local/rollout_migration/line_parser.rs:110-135`).

The oldest Artifact here is 0.61.0, well past that boundary, so **this corpus
cannot show the relative form** — but TokenLedger reads historical logs, and a
machine with pre-0.48 Artifacts will have them. A reader that wants those
Sessions must accept `resets_at` as an integer, as an RFC3339 string, or absent,
and `resets_in_seconds` on older lines. Reading a pre-0.48 relative value as an
absolute epoch yields a reset date in January 1970.

Earlier still, the block was flat rather than nested — 0.40.0 wrote
`primary_used_percent` / `primary_window_minutes` as siblings, and the nested
`primary`/`secondary` objects arrived in 0.41.0. `credits` appeared in 0.60.1,
`plan_type` in 0.66.0, `limit_id`/`limit_name` in 0.100.0.

The drift that *is* real is field addition, and it is monotonic with version:

| First seen at | Field added |
| --- | --- |
| 0.104.0 (2026-02) | `limit_id` `limit_name` `primary` `secondary` `credits` `plan_type` |
| 0.124.0 | `rate_limit_reached_type` |
| 0.142.4 | `individual_limit` |
| 0.145.0-alpha.18 | `spend_control_reached` (absent in 0.144.2) |

Every added field is **present but `null` in every observation that has it** —
`rate_limit_reached_type` `null` in all 22,032 that carry it,
`spend_control_reached` `null` in all 12,813, `individual_limit` `null` in all
21,592. `rate_limit_reached_type` in particular stayed `null` through all 147
observations at `used_percent: 100.0`, so it is **not** simply "set when the limit
is hit".

The source supplies the value spaces the logs could not. `rate_limit_reached_type`
is a closed enum of five variants — `rate_limit_reached`,
`workspace_owner_credits_depleted`, `workspace_member_credits_depleted`,
`workspace_owner_usage_limit_reached`, `workspace_member_usage_limit_reached`
(`codex-rs/protocol/src/protocol.rs:2175-2184`) — all four of the latter being
workspace/enterprise conditions, which is why a personal Plus account never sets
it. `spend_control_reached` is `Option<bool>` carrying an explicit note that
"**`None` is unavailable, not a sparse-update recovery**", and `individual_limit`
is a workspace monthly credit allowance
(`SpendControlLimitSnapshot { limit: String, used: String, remaining_percent: i32, resets_at: i64 }`).
All three are workspace features, absent on a personal plan by design rather than
by omission.

Treat all four late fields as optional and nullable. A reader that requires them
breaks on 2026-02 Artifacts; a reader that trusts them to signal saturation gets
nothing.

One discrepancy worth recording. The source history dates `individual_limit` and
`spend_control_reached` to the same commit (0.137.0), but on this machine
`individual_limit` appears from 0.142.4 while `spend_control_reached` is **absent
from 0.144.2** and first appears at 0.145.0-alpha.18. Since no field carries
`skip_serializing_if`, a single writer should emit both or neither. Either the
version attribution is imprecise or the field was reverted and re-added; the logs
are the record of what was actually written, so a reader should key on presence,
never on a version number.

## Behaviour at the edges

**At 100%, the block is written normally and the Session keeps working.** 147
observations sit at exactly `used_percent: 100.0` — 76 on the 5-hour window
across 8 Sessions, 71 on the 7-day window across 2 Sessions. `resets_at` is
still present, `rate_limit_reached_type` is still `null`, and the transcript
continues with reasoning, patches, and Model output afterwards. The newest
observation in the whole corpus is one of these:

```json
{"limit_id":"codex","limit_name":null,
 "primary":{"used_percent":100.0,"window_minutes":10080,"resets_at":1786879486},
 "secondary":null,
 "credits":{"has_credits":false,"unlimited":false,"balance":"0"},
 "individual_limit":null,"spend_control_reached":null,
 "plan_type":"plus","rate_limit_reached_type":null}
```
`sessions/2026/08/10/rollout-2026-08-10T13-13-46-…jsonl` line 498,
`2026-08-10T03:39:01.000Z`, `cli_version` 0.147.0-alpha.6.5 — and lines 480-497
of that same file are ordinary work, ending in a Model message and
`task_complete`. So **100% does not mean blocked**, and a card must not render it
as "exhausted, come back later". Whether the percentage saturates at 100 while a
different limiter governs, or the account was served on overage, is not
determinable from the logs; `credits` was `{"balance":"0","has_credits":false}`
throughout, which argues against a credit-funded overage.

**Windows never vanish and `resets_at` is never missing.** Across 24,977 window
objects, `resets_at` is never `null`, never `0`, and **never in the past
relative to its own record's timestamp** (0 of 24,977) — though the type permits
`null`, so a reader must still handle it. `resets_at - timestamp` tops out at
exactly 7.00 days for the weekly window and never exceeds the stated
`window_minutes`.

That last property, together with the ±1s wobble described below, means the stamp
is being **recomputed per response as roughly "now + remaining"**. It is not the
client doing it: since 0.48.0 the CLI copies `x-codex-primary-reset-at` verbatim
with no arithmetic, and the very reason for that release was to stop recalculating
against the current time. So the recomputation is **server-side** — the `/usage`
schema carries both `reset_after_seconds` and `reset_at` for the same window,
which fits. That the server derives one from the other is inference, not something
the source states.

No `unlimited: true` observation exists, so "do windows vanish when unlimited" is
**undetermined**.

**`used_percent` is integer-valued.** 101 distinct values, 0.0 through 100.0,
**zero non-integer values** in 24,977 windows. It is serialised as a float but
carries whole percentage points only. A card must not render a decimal place — it
would be false precision.

## Evolution within a window

Grouping observations by exact `resets_at`, `used_percent` is **effectively
monotonic**: 0 decreases in 2,621 consecutive pairs on the 5-hour window, and 5
in 21,646 (0.02%, all −1 or −4 points) on the weekly. The window is a fixed
epoch that fills and resets, **not a sliding window that decays as old usage
ages out**. That matters for the forecast ticket: a burn-rate projection may
assume the figure only rises until reset.

`resets_at` itself is not a stable epoch identity. Within what is plainly one
window it **jitters by a median of 1 second**, occasionally up to ±117s. Treat
two readings whose `resets_at` differ by a couple of minutes as the same window;
an exact-equality key will shatter one window into dozens.

One anomaly deserves recording because it defeats "just take the newest
reading". In five Sessions between 2026-07-08 and 2026-07-11, **a single Session
alternates between two entirely different limit buckets on consecutive turns**,
seconds apart, same Model (`gpt-5.5`, effort `high`), with nothing in the
payload to tell them apart — `limit_id` is `"codex"` for both and `limit_name`
is `null` for both. From
`sessions/2026/07/09/rollout-2026-07-09T01-06-05-…jsonl`:

```
line 47  2026-07-08T17:07:13.715Z   primary 61% (resets 1783537868) | secondary 80% (resets 1783595952)
line 51  2026-07-08T17:07:27.492Z   primary  3% (resets 1783537888) | secondary  0% (resets 1784102603)
line 57  2026-07-08T17:07:39.544Z   primary 62% (resets 1783537868) | secondary 80% (resets 1783595952)
```

The two weekly stamps are 5.9 days apart, so these are genuinely different
entitlements, not jitter. This sits three days before the shape changeover, and
the simplest story that fits is a server-side migration in flight, with requests
landing on either the old or the new limiter. It affects **5 of 212 Sessions**
and no Session after 2026-07-11, so it reads as a transition artifact rather
than steady state. It is still evidence that a "current limits" reading can jump
without the user doing anything, and that the block carries no key with which to
separate buckets.

## Staleness dominates the figure's worth

This is the finding that should shape the card. The reading is only as fresh as
the last Codex request, and on real usage that is often very stale:

- Only **29 of the 169 days** in the corpus span (17%) carry any observation at
  all.
- The **longest run with no Codex activity is 59 days**, and the largest gap
  between consecutive observations is **60.2 days**.
- Within an active Session, observations are dense — median gap 0s, p90 16s, p99
  148s — because one is written per request. Density inside a Session says
  nothing about freshness between Sessions.
- At the time of this audit the newest observation was **~35 hours old**.

The two windows degrade at completely different rates, and this is what makes a
single staleness rule wrong:

- A **5-hour** window is worthless once the reading is older than 5 hours. Its
  `resets_at` has passed, the window has rolled, and the true figure is
  unknowable — not "still 61%".
- A **7-day** window survives much better. The 35-hour-old 100% reading above
  still names a `resets_at` of 2026-08-16, four days out, so the 100% is very
  likely still true.

So the correct staleness test is **per window, against that window's own
`resets_at`**, not a single age cutoff on the observation. TokenTracker reached
the same conclusion for its cache
(`src/lib/usage-limits.js:2425-2433`): a window whose reset stamp has passed "is
stale data, not a usable fallback, so drop it". Its provenance record —
`{source, confidence, captured_at, stale, age_seconds}` with a `confidence`
downgrade to `"observed"` for anything locally derived
(`src/lib/usage-limits.js:3376-3395`) — is the right shape to borrow for a
log-derived figure, and none of the other three projects needs an equivalent
because they all read live.

One further trap inherited from openusage's log scanner
(`Sources/OpenUsage/Providers/Codex/CodexLogUsageScanner.swift:16-23`): forked
and subagent Sessions **replay the parent's history with rewritten timestamps**,
so a child rollout's records can look recent while describing old state. That
scanner detects them via `forked_from_id` / `parent_thread_id` /
`thread_source == "subagent"`. A "newest observation wins" rule needs the same
guard, or a replayed window can present itself as current.

## Recommendation for the Codex card

**Bars.** Render one bar per window actually present in the chosen observation,
labelled from `window_minutes` and never from the slot. Copy Codex's own mapping,
including its ±5% tolerance, so labels match what the user sees in `/status`:

| `window_minutes` (±5%) | Label |
| --- | --- |
| 300 | 5h |
| 1440 | Daily |
| 10080 | Weekly |
| 43200 | Monthly |
| 525600 | Annual |
| anything else | generic duration; do not guess a lane |

On current data that means **most machines show a single "Weekly" bar**, and
older Artifacts show two. The card must not reserve an empty "5h" row — a `null`
`secondary` is a window that does not exist, not a window at zero, which is the
same distinction the scan contract draws between `—` and `0`. Codex's own
`/status` does exactly this, emitting a row only for a window that is `Some`.

Plan label comes from `plan_type` ("Codex — Plus"), mapped through the closed
enum with `Unknown` as the fallback; `limit_name` is unusable. State the figure as
**`N% left`** (100 − `used_percent`), matching both the card reference in #103 and
Codex's own bars.

**Selection.** Newest envelope `timestamp` across all Sessions, filtered to
`limit_id == "codex"`, skipping forked/subagent replay. Last-line-per-file is
safe as a per-file shortcut.

**Staleness.** Express it per bar, against that bar's own `resets_at` — not as a
single age cutoff on the observation:

- Reset still in the future → show the bar with an observed-at note ("as of 35h
  ago"), because the window has not rolled and the figure still stands.
- Reset already passed → show the bar as **unknown**, not zero and not the last
  value. State why: "last seen 3d ago; the 5h window has reset since".
- No `codex` observation at all → the "not signed in"-style disabled card of map
  decision 9.

Codex's own four-state model (`Available` / `Stale` / `Unavailable` / `Missing`)
is the right vocabulary, and its 15-minute staleness threshold is worth knowing —
but it should **not** be copied as the cutoff. Codex refreshes live and can afford
to call a 16-minute-old reading stale; a log-derived figure is routinely hours or
days old, and 17% day-coverage means a 15-minute rule would render the card
useless nearly always. The window's own `resets_at` is the honest test, with the
observed-at age shown alongside so the user judges for themselves.

**Do not render `credits` as a bar**, and do not render it at all in the state
this corpus shows. Follow Codex's own gating: omit the row when `has_credits` is
false, show "Unlimited" when `unlimited` is true, otherwise show a rounded count
in credits — never a currency figure, and never a bar, because there is no cap to
draw against.

**Do not render a decimal place** on `used_percent`, and do not render 100% as
"blocked" — the Sessions at 100% kept working.

**Filter to `limit_id == "codex"`** and ignore `rate_limit_reached_type` as a
saturation signal; on a personal plan it is always `null`, including at 100%.

## What could not be determined

- Whether the 2026-07-13 collapse from two windows to one is a client-side or a
  server-side change. Version and date are collinear across the switch on this
  machine. The `credits` evidence proves the server drives *some* field population
  and the CLI's own labeller is written to expect a weekly `primary`, both of
  which favour server-side, but no version straddles the window boundary. A second
  installation on an older CLI would settle it.
- **Why 100% still serves requests.** `credits` was
  `{"balance":"0","has_credits":false}` throughout, which argues against a
  credit-funded overage, and `rate_limit_reached_type` never fired. No positive
  account of the mechanism.
- **What `"premium"` counts.** The *mechanism* by which it appears with no windows
  is now established (a 429 carrying `x-codex-active-limit`), but the string is
  server-supplied and appears nowhere in Codex's source, so what that entitlement
  actually meters is unknown.
- **Whether the server derives `resets_at` from a relative remaining time.** The
  ±1s wobble and the "never exceeds `window_minutes`" property both fit, and the
  `/usage` schema carries both fields, but the source does not state it.
- **The behaviour of `credits` when `has_credits` or `unlimited` is true**, and of
  `individual_limit` / `spend_control_reached` when populated. All are workspace
  features; a personal Plus account never exercises them, so nothing here is
  observed rather than read off the type.
- **Whether `window_minutes` takes values beyond the five Codex recognises.** Only
  300 and 10080 occur in this corpus; 1440, 43200, and 525600 are handled by the
  source but unobserved. No prior-art project special-cases any duration outside
  300/10080 either.
- **Whether pre-0.48 Artifacts with `resets_in_seconds` exist on real machines.**
  This corpus starts at 0.61.0, so the relative form is known only from the source
  history. If old Sessions are in scope, that path needs a fixture from a machine
  that has one.
