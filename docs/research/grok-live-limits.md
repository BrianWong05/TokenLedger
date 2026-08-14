# Grok live limit contract

Research for [#111](https://github.com/BrianWong05/TokenLedger/issues/111).
Status as of 2026-08-12. Grok is source key `il` in `source-catalog.json`
(`environment: GROK_HOME`, artifact `.grok/sessions`).

## Verdict

Two findings, and the second outranks the first.

**1. The current surface is
`GET https://cli-chat-proxy.grok.com/v1/billing?format=credits`.** This is
openusage's endpoint, and it is the one Grok Build itself calls. tokscale's
three grok.com endpoints belong to the consumer web app, not the CLI.

**2. Grok does not need a Companion at all.** The CLI writes every quota
snapshot it fetches into `~/.grok/logs/unified.jsonl` as a plain JSON line.
That file is on this machine right now with **24 real readings** carrying the
percentage, the window bounds, the period type and the plan name. Grok's limits
are therefore a **passive, from-logs** Source in the Codex sense (ADR-0013) —
`capabilities.limits: "logs"`, not `"live"`. No credential is read, no request
is made, ADR-0019 is never invoked, and the expired-token and refresh-token
rotation hazards documented below simply do not arise.

The ticket asked which of three authenticated surfaces to use. The better
answer is that the authenticated surface is optional: xAI already spilled the
answer onto disk. Recommendation is to ship the log path and not build a Grok
Companion.

### Which surface is current

The three surfaces are not competing versions of one API — they belong to two
different products.

| Surface | Belongs to | Called by Grok Build? |
| --- | --- | --- |
| `cli-chat-proxy.grok.com/v1/billing?format=credits` | **Grok Build (the CLI)** | **Yes** — verbatim in its source |
| `cli-chat-proxy.grok.com/v1/settings` | Grok Build | Yes, but it is the remote feature-flag document, not a quota surface |
| `grok.com/rest/subscriptions` | grok.com web app | No |
| `grok.com/rest/tasks/usage` | grok.com web app | No |
| `grok.com/grok_api_v2.GrokBuildBilling/GetGrokCreditsConfig` | grok.com web app | No |

`rg -a -c 'rest/subscriptions|rest/tasks/usage|GrokBuildBilling|GetGrokCreditsConfig|grok_api_v2'`
over the shipped 127 MB Grok Build binary exits 1 — **zero matches**. All three
tokscale endpoints are real and live in tokscale (verified in source), but the
CLI never calls them.

There is a **second supersession inside the chosen endpoint**, which the ticket
does not mention and which matters more than the choice of URL. xAI's own
`BillingConfig` doc comment says so directly:

> Carries both the newer credits-config fields (`credit_usage_percent`,
> `current_period`) and the deprecated `GrokBuildBillingConfig` fields
> (`monthly_limit`, `used`, `billing_period_*`). Consumers should prefer the
> new fields and fall back to the deprecated ones…

openusage acted on this in `5fcb431` — *"refactor(grok): fetch weekly pool via
the CLI's JSON credits endpoint, drop legacy monthly meter"* — and its
`docs/providers/grok.md` states: *"The weekly shared pool is the limit Grok
enforces for unified-billing accounts (the old monthly credits meter is legacy
and no longer shown)."* tokscale, by contrast, reads `monthlyLimit` and
`usage.totalUsed` — **the deprecated fields**. So the prior art disagrees on
more than the URL, and tokscale is the stale one on both counts.

### Evidence base

1. **xAI's own client source is public** — `github.com/xai-org/grok-build`,
   Apache-2.0, default branch `main`, HEAD `b13fa526f5112c0b20dad5f1f2300d3d3b127895`
   (2026-08-10T17:22:17Z), 24,682 stars. This is the ticket's preferred source
   and it exists; the assumption that it might not be available was wrong. It is
   a snapshot export of xAI's internal monorepo (hence `crates/codegen/…`
   paths), with some constants wrapped in `obfstr!` but readable in plaintext.
   The `cli-chat-proxy` server itself is not public.
2. **The shipped client binary** — `~/.grok/downloads/grok-macos-aarch64`
   (Mach-O arm64, 127,259,280 bytes, 2026-07-10; symlinked `~/.grok/bin/grok`),
   version `0.2.93`. Built with symbols, so it carries its crate paths and
   `serde` field tables as strings. Used to confirm what actually ships matches
   the public repo, and to prove the absence of the tokscale endpoints.
3. **A real captured payload on this machine** — 24 entries in
   `~/.grok/logs/unified.jsonl`, plus vendor docs shipped in the install
   (`~/.grok/README.md`, `~/.grok/docs/user-guide/02-authentication.md`).
4. **openusage** — `robinebers/openusage` `main` @ `487cc8f19a9a28676f6924aafa48dee79ad7a7f6`,
   `Sources/OpenUsage/Providers/Grok/*.swift`. Its
   `Tests/OpenUsageTests/GrokCreditsConfigFixtures.swift` carries a payload
   annotated *"captured live from cli-chat-proxy.grok.com on 2026-07-06"*.
5. **tokscale** — `junhoyeo/tokscale` `main` @ `246765b1f32c384c375601c4307477847355fbbf`,
   `crates/tokscale-cli/src/commands/usage/grok.rs` (blob
   `c43aa79e3b93ff782f55eecc2d6d195e9095e0c4`, added by `28aec2006a`, #669).

No request was made to xAI. Everything below is read from source, from vendor
docs, or from local files.

## The recommended path: read the log (no credential, no request)

`crates/codegen/xai-grok-shell/src/extensions/billing.rs` logs every successful
fetch, with a comment explaining why:

```rust
// Every prompt / /usage / poll path hits `x.ai/billing`; log the fetched
// credits snapshot so support can correlate limit UX with real balances.
xai_grok_telemetry::unified_log::info(
    "billing: fetched credits config",
    None,
    Some(billing_unified_log_ctx(&billing)),
);
```

`billing_unified_log_ctx` serialises the whole config, replacing the `history`
array with `historyLen` + `latestHistory` to bound the line length.

### What is actually on disk

`~/.grok/logs/unified.jsonl` (863,222 bytes here), one JSON object per line.
Envelope fields: `ts`, `src`, `pid`, `ver`, `lvl`, `sid`, `msg`, `ctx`, where
`src` ∈ {`shell`, `grok-pager`, `grok-desktop`}. Select on
`msg == "billing: fetched credits config"`.

Verified shape of a real entry (types, not values):

```
ts                                    str   RFC3339 — this is observed_at
src                                   str   "shell"
lvl                                   str   "info"
msg                                   str   "billing: fetched credits config"
ctx.config.creditUsagePercent         float 0–100  ← used_pct
ctx.config.currentPeriod.type         str   "USAGE_PERIOD_TYPE_WEEKLY"
ctx.config.currentPeriod.start        str   RFC3339
ctx.config.currentPeriod.end          str   RFC3339 ← resets_at
ctx.config.onDemandCap.val            int   USD cents
ctx.config.onDemandUsed.val           int   USD cents
ctx.config.prepaidBalance.val         int   USD cents
ctx.config.isUnifiedBillingUser       bool
ctx.config.billingPeriodStart         str   deprecated mirror of currentPeriod.start
ctx.config.billingPeriodEnd           str   deprecated mirror of currentPeriod.end
ctx.config.historyLen                 int
ctx.onDemandEnabled                   null  (from remote settings; null here)
ctx.subscriptionTier                  str   "SuperGrok"  ← plan
```

Measured across the 24 local readings:

- Span: a ~27-hour stretch in early July 2026 (exact instants withheld — account-identifying), all `src: shell`.
- `currentPeriod.type` is `USAGE_PERIOD_TYPE_WEEKLY` on every row.
- Period start → end measured **exactly 7 days apart, 10,080 minutes**, with
  microsecond precision on the wire, anchored to an account-specific instant
  (exact bounds withheld — they identify the account's billing anchor).
  Not a calendar month.
- `billingPeriodEnd == currentPeriod.end` on all 24 rows, so the deprecated
  mirror is a safe fallback.
- `creditUsagePercent` present on **17 of 24** rows. Distinct values across all
  24: `0, 1, 12, 13, 14, 15, 16` — **integer-valued, monotonically rising within
  the period.** Exactly the integer fill-curve the `limit_readings` content PK
  is designed to absorb (≤101 rows per window per epoch).
- `subscriptionTier` = `SuperGrok`; `onDemandCap`, `onDemandUsed`,
  `prepaidBalance`, `historyLen` all `0`; `isUnifiedBillingUser` `true`.

**The 7 rows missing `creditUsagePercent` are the single most important parsing
rule, empirically confirmed.** The response is proto3 serialised as JSON, so
zero-valued scalars are omitted. An absent `creditUsagePercent` means **0%, not
a schema change**. Both sides document this — xAI on its `Cent` type ("proto3
JSON omits zero-valued scalars, so a `$0` Cent arrives as `{}`; default to 0
rather than failing the whole parse") and openusage's decoder ("an absent
`creditUsagePercent` means 0, not a schema change"). Treating absence as
malformed would silently drop every 0% reading, i.e. the start of every window.

### Ingest rules for the log path

- Select `msg == "billing: fetched credits config"`; `ctx.config` is the payload.
- `used_pct` = `ctx.config.creditUsagePercent`, **defaulting to 0.0 when absent**.
- `resets_at` = `ctx.config.currentPeriod.end`, falling back to
  `ctx.config.billingPeriodEnd`; RFC3339 → unix seconds at ingest.
- `observed_at` = the envelope `ts`, never a filename date.
- `plan` = `ctx.subscriptionTier`.
- Rows lacking both `currentPeriod.end` and `billingPeriodEnd` yield no Reading —
  a window with no reset instant cannot be placed on a bar.
- This is a new artifact under `$GROK_HOME`/`~/.grok` (`logs/unified.jsonl`),
  discovered and failing independently of the existing `sessions` artifact
  (ADR-0015).

The log is written by `crates/codegen/xai-grok-telemetry/src/unified_log.rs` via
a `jsonl.tmp` staging file. It is distinct from the sampled `sampling.jsonl`
(`xai-grok-sampler/src/sampling_log.rs`) and is not gated by the
telemetry-upload knobs — trace upload *snapshots* this file, it does not create
it. Local-only, and it is documented in the vendor's own file table
(`05-configuration.md:751`).

## The live contract (reference, and the fallback if the log path is rejected)

Verbatim from `crates/codegen/xai-grok-shell/src/extensions/billing.rs`
(`handle_get_billing`, line 200 onward at the HEAD above):

```rust
let proxy_base = agent.cli_chat_proxy_base_url();
let base = proxy_base.trim_end_matches('/');
let credits_url = format!("{}/billing?format=credits", base);
let credits_resp = crate::http::shared_client()
    .get(&credits_url)
    .header("Authorization", format!("Bearer {}", &auth.key))
    .header("X-XAI-Token-Auth", crate::auth::GrokComConfig::default().token_header)
    .header("x-userid", &auth.user_id)
    .header("x-grok-client-version", xai_grok_version::VERSION)
    .header(crate::http::CLIENT_MODE_HEADER, crate::http::process_client_mode())
    .timeout(std::time::Duration::from_secs(15))
```

- **GET**, not POST. `?format=credits` is load-bearing: it selects the
  `GetGrokCreditsConfig` message. Bare `/v1/billing` returns the deprecated
  monthly shape.
- Base is `https://cli-chat-proxy.grok.com/v1`
  (`crates/codegen/xai-grok-env/src/lib.rs`), overridable by
  `GROK_CLI_CHAT_PROXY_BASE_URL`. Honour the override — self-hosted proxies are
  supported.
- `X-XAI-Token-Auth: xai-grok-cli` is documented as required: *"Tells the auth
  middleware to validate as a CLI session token"* (`~/.grok/README.md:359`).
  Omitting it is the likeliest cause of a spurious 401.
- `x-userid` carries the account id from the credential document.
- Sibling endpoint `/auto-topup-rule` serves the top-up rule
  (`enabled`, `minBeforeHittingSl`, `topupAmount`, `maxAmountPerMonth`). Not
  needed for a card.

### Response shape

Top level (`BillingConfigResponse`): `config`, plus `onDemandEnabled` and
`subscriptionTier` — **but those last two are not from the wire.** The CLI
overwrites them from its cached remote settings immediately after parsing:

```rust
// Enrich with fields from remote settings.
let rs = agent.cfg.borrow().remote_settings.clone();
billing.on_demand_enabled = rs.as_ref().and_then(|rs| rs.on_demand_enabled);
billing.subscription_tier = rs.as_ref().and_then(|rs| {
    rs.subscription_tier_display.clone().or_else(|| rs.subscription_tier.clone())
});
```

So on the live path **the plan label costs a second request** to `/v1/settings`
(`subscription_tier_display`, e.g. `"SuperGrok Heavy"`, `"Free"`, `"API Key"`;
falling back to `subscription_tier` codes `free`/`premium`/`supergrok`/
`supergrok_heavy`) — which is exactly what openusage does. On the **log path it
is free**, because the CLI already merged it before logging.

`config` (`BillingConfig`, all fields optional):

| Field | Type | Note |
| --- | --- | --- |
| `creditUsagePercent` | f64 0–100 | **preferred**; absent = 0 |
| `currentPeriod` | `{type, start, end}` | **preferred**; `type` is the proto enum name, e.g. `USAGE_PERIOD_TYPE_WEEKLY` |
| `monthlyLimit` | `{val}` cents | **deprecated** |
| `used` | `{val}` cents | **deprecated** |
| `billingPeriodStart` / `billingPeriodEnd` | RFC3339 | **deprecated** mirrors of `currentPeriod.start`/`.end` |
| `onDemandCap`, `onDemandUsed`, `prepaidBalance` | `{val}` cents | pay-as-you-go / topped-up balance |
| `isUnifiedBillingUser` | bool | on the shared weekly/monthly pool |
| `history[]` | `{billingCycle:{year,month}, includedUsed, onDemandUsed, totalUsed}` | past periods |

Money is **USD cents**, wrapped as `{"val": <int>}` — xAI's type is literally
`/// Cent value from the billing API (USD cents).` openusage's comment calling
`onDemandCap` "credits" is the looser of the two. A `$0` value arrives as `{}`.
There is also a `productUsage[]` of `{product, usagePercent}` that xAI's own
tests note is unused by the CLI billing surface.

### Credential source, and why the live path is the worse option

`~/.grok/auth.json`, mode `0600` (`$GROK_HOME/auth.json` when set; `GROK_HOME`
is unset here). A sibling `~/.grok/auth.json.lock` exists.

It is a **map keyed by `{issuer}::{client_id}`** — xAI builds the key as
`format!("{}::{}", issuer.trim_end_matches('/'), client_id)`. On this install the
key is `https://auth.x.ai::<uuid>`, where the uuid matches xAI's own default
client-id constant (`obfstr!("b1a00492-…")`), so it is a public shared client id,
not a secret. xAI's README example uses a different key,
`https://accounts.x.ai/sign-in`, and the binary also contains
`accounts.mouseion.dev` and `localhost:20000` issuers. **A reader must iterate
the top-level keys**, not assume one. openusage splits the key on `::` and takes
the last component as the OAuth `client_id`, preferring the entry's own
`oidc_client_id` — which matches xAI's construction exactly.

Per-entry fields on this install (names only, no values read out):

```
auth_mode  oidc_issuer  oidc_client_id  key  refresh_token  expires_at
create_time  principal_id  principal_type  team_id  user_id  email
first_name  last_name  profile_image_asset_id
```

`key` is the access token (a 3-segment JWT; the in-memory struct calls the same
field `access_token`). It is presented directly as the bearer token — **no
exchange is required for a valid token.** xAI's own README shows the recipe:
`Authorization: Bearer $(jq -r '."https://accounts.x.ai/sign-in".key' ~/.grok/auth.json)`
(`README.md:344`). `auth_mode` is `oidc` here; an API-key mode also exists
(`XAI_API_KEY`), used only when no session token is active.

Two hazards make the live path unattractive, and both are avoided entirely by
reading the log:

**Expiry.** xAI's README: *"`auth.json` tokens expire after 7 days. Run `grok
login` to refresh."* Refresh fires proactively ~5 min before expiry
(`GROK_AUTH_EARLY_INVALIDATION_SECS`, default 300) and on any 401
(`02-authentication.md:213-217`); credentials with no server expiry fall back to
30 days. On this machine `expires_at` shows the token expired days before this audit —
**expired nine days ago**, because the CLI has not run since. A Companion
obeying ADR-0019 bound 1 would present a dead token and get a 401. The cheap
mitigation is to read `expires_at` and render the signed-out card *without*
issuing the request — bound 4 reached by local inspection.

**Refresh-token rotation.** ADR-0019 bound 1 forbids spending the refresh token,
and here that bound is protecting something concrete. xAI rotates refresh
tokens and serialises rotation across processes through `auth.json.lock`; its
`storage.rs` states that sending a refresh token to the IdP and writing
`auth.json` MUST be serialised, and the binary carries the failure vocabulary to
match: `sibling_has_different_refresh_token`, `auth: adopted sibling token`,
`auth.refresh.lock_lost_before_idp`, `refresh_token_rejected`,
`no_refresh_authority`, `recovery_exhausted`. **openusage does exactly what the
ADR forbids**: it POSTs form-encoded
`grant_type=refresh_token&client_id=…&refresh_token=…` to
`https://auth.x.ai/oauth2/token` (hardcoded; the CLI instead discovers
`token_endpoint` from `{issuer}/.well-known/openid-configuration`) and then
writes the rotated credentials back via `authStore.save(state)` — **taking no
lock**. A concurrent `grok` refresh and an openusage refresh can therefore race,
and because the tokens rotate, the loser's refresh token is invalidated. That is
the tokscale-broke-Claude-Code failure from bound 1, reproduced on a different
vendor. Do not copy it, and do not refresh at all — not even in memory and
discarded, since the rotation happens server-side regardless of whether we
persist the result.

## Unit verdict: a percentage of a rolling weekly credit pool

The measured window is **exactly 7 days** (10,080 minutes), anchored to an
account-specific instant, with a server-computed integer percentage and an
explicit end timestamp. So despite the endpoint being called "billing":

- It is **not** a task count. `rest/tasks/usage` genuinely is one
  (`usage`/`limit`, `frequentUsage`/`frequentLimit`,
  `occasionalUsage`/`occasionalLimit`), but that is the dead web surface.
- It is **not** a calendar billing period, for weekly accounts. The
  `USAGE_PERIOD_TYPE_MONTHLY` variant exists and would be a calendar period, but
  the enforced limit for unified-billing accounts is the weekly pool.
- The underlying quantity is money (USD cents), but we never touch it: the
  vendor hands us the percentage directly.

**It belongs on a bar.** A vendor-computed `used_pct` plus a real reset instant
is precisely what `limit_readings` stores, with no derived arithmetic and no
unit conversion — a cleaner fit than Codex, whose durations must be classified
heuristically.

One caveat for the card copy: a Grok bar at 80% means *"80% of this week's
credit pool spent"*, whereas a Claude bar at 80% means *"approaching a rate
limit"*. Same geometry, different quantity. Label it "credits" so the two are
not read as the same thing.

Separately, there is **no rolling rate-limit window** for Grok Build beyond
this pool. The only other quota signal is an opaque free-tier cutoff surfaced as
HTTP 402/403 with the stream-error taxonomy `usage_pool_exhausted` (out of
balance) / `usage_limit_reached` (free usage limit), plus
`global_rate_limit` / `concurrency_limit` and the upsell copy *"You've reached
your free Grok Build usage limit for now."* No percentage, no reset time. The
CLI's analytics vocabulary includes `credit_limit_hit` and `rate_limit_error`.

## Mapping onto `(source, window_key, window_minutes, used_pct, resets_at, observed_at, via, plan)`

| Column | Value |
| --- | --- |
| `source` | `il` |
| `window_key` | from `currentPeriod.type`: `USAGE_PERIOD_TYPE_WEEKLY` → `w10080`, `…_MONTHLY` → `w43200` |
| `window_minutes` | measured `currentPeriod.end − currentPeriod.start` (10,080 observed) |
| `used_pct` | `creditUsagePercent`, unconverted; **absent → 0.0** |
| `resets_at` | `currentPeriod.end` (fallback `billingPeriodEnd`) → unix seconds |
| `observed_at` | envelope `ts` (log path) or fetch time (live path) |
| `via` | **`logs`** on the recommended path |
| `plan` | `subscriptionTier` |

One bar per card — Grok reports exactly one window.

**Key `window_key` off `currentPeriod.type`, not off the measured duration.**
The spec classifies `window_minutes` against the canonical set
300/1440/10080/43200/525600 within ±5%, which suits Codex because Codex reports
durations. Weekly is unambiguous (10,080 exactly, canonical). But if a
`USAGE_PERIOD_TYPE_MONTHLY` account ever appears, a calendar month is
40,320–44,640 minutes: 30- and 31-day months land inside 43200 ±5%
(41,040–45,360), while a 28-day February at 40,320 falls **outside** it and
would keep its raw minutes, silently splitting one card's history into two
window keys once a year. The vendor names the period type, so use it and skip
the inference.

**Do not extrapolate Grok's next reset.** The spec extrapolates a periodic
window's next reset as `resets_at + n·duration`. That is sound for the weekly
pool but wrong for a calendar month, and on the log path there is nothing to
extrapolate *from* anyway — a stale reading is stale. Per the spec's own
freshness rule, a reading whose `resets_at` is in the future stands however old
it is; one whose `resets_at` has passed is stale. That rule already does the
right thing here: the newest local reading has a `resets_at` now a month past, so this machine's card would correctly show no current figure until
`grok` runs again.

## Card recommendation

- **Catalog**: `il` gains `capabilities.limits: "logs"` — same class as Codex,
  so no opt-in gate and no Companion.
- **Header**: Grok mark, label, plan pill from `subscriptionTier`, freshness
  copy *"from your logs · last request {t} ago"* — the existing `via: 'logs'`
  string, which is exactly honest here.
- **One bar**, labelled **"Weekly credits"** (or "Monthly credits" from
  `currentPeriod.type`), filled from `creditUsagePercent`, with
  **"Next reset {t}"** from `currentPeriod.end`.
- **Match the CLI's own `/usage` view** where cheap — its format strings are
  `Usage`, `Weekly limit`, `Monthly limit`, `Next reset:`, `Credits:`,
  `used of $ limit`, `Credits left:`, `Pay-as-you-go limit left:`. `/usage` is
  documented as *"View credit usage or manage billing"*
  (`04-slash-commands.md:488`).
- **No dollar subtitle.** A `$X of $Y` line is only derivable from the
  deprecated `monthlyLimit`/`used` pair, which is absent for weekly
  unified-billing accounts — all 24 local readings have no limit in dollars.
  tokscale prints one because it reads the deprecated shape. Show the percentage.
- **Skip the on-demand second bar.** `onDemandUsed`/`onDemandCap` exist and are
  gated by `onDemandEnabled` (which is `null` here, since it comes from remote
  settings the log may not carry). Minority configuration; add it when someone
  with on-demand enabled asks.
- **Non-weekly accounts**: follow openusage's judgement — omit the line rather
  than mislabel it, *"mislabeling its monthly percent would be worse than an
  honest blank."*
- **No readings in the log** (never ran `grok`, or a truncated log): no card,
  the ordinary absent-Source path. Nothing to sign into, nothing to explain.

## Protobuf dependency: none

The chosen path is JSON on both the log and live routes. `src-tauri/src/proto.rs`
is not needed and should not be pulled in.

For the record, the protobuf question comes only from tokscale's grok.com RPC,
which is **grpc-web binary, not Connect-JSON**: `Content-Type` and `Accept` both
`application/grpc-web+proto`, request body a single empty grpc-web frame
`[0,0,0,0,0]` (1-byte compression flag + 4-byte big-endian length 0). tokscale
decodes the reply with ~130 lines of hand-rolled, schema-less protobuf,
positional on field numbers (outer field 1 → config submessage; config field 1,
fixed32 → used percent; fields 4 and 5 → `google.protobuf.Timestamp` for period
start/end) — the same technique as our `proto.rs`, which would be the right
starting point if that endpoint ever became necessary. It is not. Note also
`github.com/xai-org/xai-proto` ("Public protobuf definitions for xAI's gRPC
APIs") if a schema is ever wanted.

## Confidence

**High — read from xAI's own source, its shipped binary, or real local data:**

- `/v1/billing?format=credits` is the CLI's quota surface, issued as a GET with
  `Authorization: Bearer`, `X-XAI-Token-Auth`, `x-userid`,
  `x-grok-client-version` (all quoted from source at a pinned SHA).
- The three tokscale endpoints are absent from the shipped binary.
- `?format=credits` selects the new message; `monthlyLimit`/`used`/
  `billingPeriodStart`/`billingPeriodEnd` are deprecated per xAI's own comment.
- Full response field inventory and nesting under `config`, money as USD cents
  in `{val}` wrappers, proto3 zero-omission.
- `subscriptionTier`/`onDemandEnabled` are injected client-side from remote
  settings, not returned by the billing endpoint.
- The unified-log path exists, its selector, its field paths, and its contents:
  24 readings, `USAGE_PERIOD_TYPE_WEEKLY`, exactly 10,080 minutes, integer
  percentages 0→16, `creditUsagePercent` absent on 7 rows.
- Credential document location, `{issuer}::{client_id}` keying, `key` as the
  bearer token, 7-day expiry, rotation serialised under `auth.json.lock`.
- openusage refreshes and writes `auth.json` back without taking that lock.
- No protobuf needed.

**Medium:**

- Whether the log path is *sufficient in practice* for users who run `grok`
  rarely. The data is only as fresh as the last CLI invocation. Mitigated by the
  spec's existing staleness rule, but it does mean a Grok card can sit empty for
  a user who has the CLI installed and idle. This is the one honest argument for
  the live path, and it is a product call rather than a technical finding.
- Exact `subscriptionTier` vocabulary on the wire. `"SuperGrok"` observed
  locally; xAI documents `"SuperGrok"`, `"X Premium+"`, `"Free"`, `"API Key"`
  for the display field and `free`/`premium`/`supergrok`/`supergrok_heavy` for
  the code field.
- `USAGE_PERIOD_TYPE_MONTHLY` behaviour. The variant is documented by xAI but
  was never observed, so the monthly mapping above is untested.

## Could not determine

- **Whether `unified.jsonl` rotates or is capped.** 863 KB after ~2 days of use
  here, with no rotation strings found. If it truncates we lose history but not
  the newest reading, so the card is unaffected — but a scan that assumes
  append-only offsets should be checked against that.
- **What a free-tier account's payload looks like** — whether `/billing` returns
  zeros, an error, or nothing, and whether the CLI logs it at all. This decides
  the free-tier card state. `subscription_tier_display` is documented to return
  `"Free"`, so the plan pill at least resolves.
- **The live response for a monthly/non-unified account**, per the medium note
  above.
- **Whether `format=` has other billing projections** worth reading
  (`format=duration` appears in the binary but was not traced to this route).
- **The `cli-chat-proxy` server behaviour** — real rate limits or quotas on the
  endpoint itself. Not public. xAI's own comment that *"Every prompt / /usage /
  poll path hits `x.ai/billing`"* suggests frequent polling is expected and that
  ADR-0019's ≥60s floor is conservative, but that is an inference.
- **Whether every refresh rotates the refresh token.** The handling vocabulary
  and the documented locking requirement prove rotation happens and must be
  serialised; they do not prove it is unconditional. The recommendation (never
  refresh) holds either way.

## Sources

- `github.com/xai-org/grok-build` @ `b13fa526f5112c0b20dad5f1f2300d3d3b127895`
  (Apache-2.0) — `crates/codegen/xai-grok-shell/src/extensions/billing.rs`
  (request construction, `BillingConfig`/`Cent`/`UsagePeriod` structs, remote-settings
  enrichment, unified-log call), `…/xai-grok-shell/src/auth/` (`config.rs`,
  `oidc/protocol.rs`, `storage.rs`), `…/xai-grok-env/src/lib.rs` (base URL),
  `…/xai-grok-config-types/src/lib.rs` (`RemoteSettings`),
  `…/xai-grok-telemetry/src/unified_log.rs`.
- `~/.grok/downloads/grok-macos-aarch64` — shipped client v0.2.93, Mach-O arm64,
  127,259,280 bytes, 2026-07-10.
- `~/.grok/logs/unified.jsonl` — 24 `billing: fetched credits config` entries.
- `~/.grok/auth.json` — field inventory only; no values read out.
- `~/.grok/README.md:337-372`; `~/.grok/docs/user-guide/02-authentication.md:15,85,91,213-217,233`;
  `04-slash-commands.md:488`; `05-configuration.md:751`.
- `robinebers/openusage` @ `487cc8f19a9a28676f6924aafa48dee79ad7a7f6` —
  `Sources/OpenUsage/Providers/Grok/{GrokUsageClient,GrokCreditsConfigDecoder,GrokAuthStore,GrokProvider}.swift`,
  `Tests/OpenUsageTests/GrokCreditsConfigFixtures.swift`, `docs/providers/grok.md`;
  supersession commit `5fcb431225e62cb8db365b04fd63aa027507c146`.
- `junhoyeo/tokscale` @ `246765b1f32c384c375601c4307477847355fbbf` —
  `crates/tokscale-cli/src/commands/usage/grok.rs` (blob `c43aa79e3b93ff782f55eecc2d6d195e9095e0c4`).
- `docs/adr/0013-source-acquisition-is-local-and-passive.md`,
  `docs/adr/0015-source-discovery-backfills-and-fails-independently.md`,
  `docs/adr/0019-live-limits-are-fetched-by-a-companion-never-the-app.md`.
</content>
