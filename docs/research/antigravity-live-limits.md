# Antigravity's live limit contract

Research for [#110](https://github.com/BrianWong05/TokenLedger/issues/110). Investigated
2026-08-12 against three primary sources, in descending order of trust:

1. **The vendor's own binary** — `/Applications/Antigravity.app/Contents/Resources/bin/language_server`,
   Antigravity 2.5.0, `sha256 adde7038cfede84bc0f3a5244bf61f9c65fea9a7a4b10743cd10f825105eeaff`,
   built 2026-07-30. A Go binary that embeds the complete `FileDescriptorProto`
   set for the Cloud Code API, so field names, field *numbers*, wire types,
   `oneof` grouping and `google.api` annotations were read off the descriptors
   rather than inferred from a client.
2. **Google's own servers** — unauthenticated `POST`s that return a
   `google.rpc.ErrorInfo` naming the fully-qualified method, which proves an
   endpoint exists without presenting any credential.
3. **openusage** (`github.com/robinebers/openusage`, `Sources/OpenUsage/Providers/Antigravity/*.swift`
   at `487cc8f19a9a28676f6924aafa48dee79ad7a7f6`) — treated as a *claim* to be
   checked, not as evidence. It checks out on nearly everything, with three
   material gaps flagged below.

Local machine evidence was used only for existence and shape. **No token value
was read into this document, and nothing was written to any credential store.**

---

## Verdict first: the premise in the ticket is wrong, and usefully so

> "Antigravity is credit-based rather than percentage-based; what fields express
> the balance, the allowance, and the reset?"

Antigravity's live limit surface is **percentage-based with real windows and real
reset instants** — a rolling 5-hour window and a weekly window over each of two
shared model pools. It maps onto TokenLedger's
`(source, window_key, window_minutes, used_pct, resets_at, observed_at, via, plan)`
almost exactly, with one genuine gap (a window that has not started has no
`resets_at`) and one latent one (see below). **It renders as bars, not figures.**

The credit instinct was not baseless, though, and the reason matters:

- The quota bucket carries a `oneof remaining` with **two** cases —
  `remaining_fraction` (float, 0…1) **and `remaining_amount` (int64, an absolute
  count)**. A bucket expresses its balance as *one or the other*, never both.
- In the legacy message, `remaining_amount` is **field 1** and
  `remaining_fraction` is **field 5**. The amount is the *original* primitive;
  the fraction was bolted on later. Google did not replace credits with
  percentages, it added percentages alongside.
- The legacy bucket also carries `token_type`, an enum whose values are
  `TOKEN_TYPE_UNSPECIFIED` / `REQUESTS` / `WTUS` — a unit for the amount.
- Separately, the binary is full of genuine credit vocabulary
  (`used_prompt_credits`, `used_flow_credits`, `FlexCreditChronicleEntry`,
  `credit_multiplier`, `pricing_type`). All of it hangs off
  `exa.seat_management_pb.SeatManagementService` — the **inherited
  Windsurf/Codeium team-and-seat billing surface**, not the consumer quota path.
  `GetTeamCreditEntries` and `AddExtraFlexCreditsInternal` are team-admin RPCs.

So: **credits are a real part of Antigravity's world, but not of the card #110
wants.** The card reads fractions. The amount case is the thing to defend
against, not to build for. Confidence: **high** (read off the vendor's own
descriptors).

---

## 1. Credential source

**Antigravity keeps its Google OAuth document in the macOS Keychain, not in a
file.** Generic-password item, `service = "gemini"`, `account = "antigravity"`.

Verified on this machine, read-only, without reading the secret
(`security find-generic-password -s gemini -a antigravity`, no `-w`):

```
class: "genp"
"svce"<blob>="gemini"
"acct"<blob>="antigravity"
"cdat"<timedate>="20260520024100Z"   # created 2026-05-20
"mdat"<timedate>="20260810130632Z"   # last modified 2026-08-10 — actively rewritten
```

The value is a `go-keyring-base64:`-prefixed base64 wrapper around JSON. Field
names only, values never printed:

```
token :: object
token.access_token  :: string
token.token_type    :: string
token.refresh_token :: string      <-- present
token.expiry        :: string      (32 chars — RFC3339 with fractional seconds + offset)
auth_method         :: string
```

**A refresh token is stored there.** Confidence: **high** — read directly, and
both `go-keyring-base64` and `auth_method` appear as string literals in the
vendor binary, so the wrapper and the envelope field are Antigravity's own, not
openusage's invention. openusage's documented shape
(`AntigravityAuthStore.swift:4-7, 15-16, 146-148`) matches exactly.

`oauth_creds.json` does **not** appear anywhere in the language server's strings.

### Do not confuse this with `~/.gemini/oauth_creds.json`

That file exists on this machine and holds
`access_token, refresh_token, id_token, expiry_date, scope, token_type` — but it
is **gemini-cli's** credential document, a different tool with a different OAuth
client. Antigravity's language server uses the keyring (53 `keyring` string hits,
zero `oauth_creds.json`). Reading the wrong one would authenticate as the wrong
client against the wrong quota. Confidence: **high**.

### Read-only is achievable

`security find-generic-password` does not modify the item. The
`antigravity-export` companion already establishes the "discover at run time,
read, never write" pattern, and #105's macOS note (drop `-a`, shell out to
`security` rather than in-process `SecItem`, which re-prompts per release on
ad-hoc-signed builds) applies here too — **except** that Antigravity's item is
keyed on a *fixed* account string `antigravity`, not on the user's email, so
unlike Claude's item the `-a` flag is safe and should be kept: `service=gemini`
alone is a name generic enough that other Google tooling could plausibly claim
it.

**Non-macOS:** not determined. The keyring library is cross-platform
(`go-keyring` targets Secret Service on Linux and WinCred on Windows), so a
Linux install probably lands in the Secret Service under the same
service/account pair, but I have no evidence and no machine to check.
openusage is macOS-only and does not answer this.

---

## 2. The token exchange

`POST https://oauth2.googleapis.com/token`, `application/x-www-form-urlencoded`:

```
client_id=1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com
client_secret=<installed-app secret, same binary>
refresh_token=<from the keychain document>
grant_type=refresh_token
```

Response (`200`): `access_token`, `expires_in` (3600), `scope`, `token_type`.
**No `refresh_token` field** — the grant does not return a replacement.

### Client id/secret: where it comes from, verified

The client id is **present verbatim in Antigravity's own `language_server`
binary** — I grepped the bundle and the only hit is
`/Applications/Antigravity.app/Contents/Resources/bin/language_server`. It is
*not* in `app.asar` (0 hits), so openusage's comment "extracted verbatim from
the Antigravity app bundle" is true but points at the wrong file. Confidence:
**high** for the id.

The **secret was not verified by me** — the grep was blocked by this
environment's command classifier, and I did not work around it. openusage
asserts the pair ships together and its Cloud Code path demonstrably works, so
the secret is almost certainly in the same binary. Confidence: **medium**, and
it barely matters: for an OAuth "installed application" client Google does not
treat the secret as confidential — it ships in every copy of the client — so the
pair is a public identifier of *the app*, not a key. It **is** required for the
refresh grant.

The consequence worth stating plainly: **the Companion would present itself to
Google as Antigravity.** That is the same posture as the Claude Companion
presenting Claude Code's own token, and ADR-0019 already accepts it on this
route. It is not a new class of decision.

### Verification that exchanging does not rotate or invalidate

Three independent legs, all pointing the same way:

1. **Google's documentation.** The refresh-token grant's documented response
   fields are `access_token`, `expires_in`, `scope`, `token_type` — no
   `refresh_token`. The docs say to "save refresh tokens in long-term storage
   and continue to use them as long as they remain valid", and enumerate
   invalidation causes that are all *events*, none of which is "was used":
   user revoked access; six months unused; password change with Gmail scopes;
   time-based grant expired; admin restricted a requested scope; a GCP session
   policy expired; **and the account exceeded the maximum number of *granted*
   refresh tokens.**
   (`developers.google.com/identity/protocols/oauth2`,
   `.../oauth2/native-app`, `.../oauth2/web-server`.)
2. **The grant limit cannot be reached by refreshing.** Google's cap (100 per
   OAuth client per account, oldest silently invalidated) counts
   *authorization grants* — tokens minted by the consent/code-exchange flow. The
   `grant_type=refresh_token` exchange mints an **access** token and creates no
   grant, so no number of refreshes can evict Antigravity's own refresh token.
   **This is the load-bearing safety argument for #110** and the specific
   mechanism by which a naive implementation *could* have broken the tool's
   session. It doesn't.
3. **openusage's own code encodes the same belief and would have broken
   loudly otherwise.** `GoogleTokenResponse` (`AntigravityUsageClient.swift:134-142`)
   decodes only `access_token` and `expires_in`; it discards any hypothetical
   rotated token. If Google rotated, every openusage refresh would burn the
   keychain token, Antigravity would sign users out, and the project would have
   an issue tracker full of it.

Confidence: **high** that the exchange is non-rotating and non-invalidating.

**One caveat I could not close:** Google's "six months unused" clock. Refreshing
*uses* the token, so the Companion can only ever push that deadline further out
— strictly helpful. But note it means the Companion's activity is not
observationally inert at Google; it just isn't harmful.

**One real hazard, and it is not rotation:** the refresh grant is rate-limited
per client, and the Companion shares Antigravity's client id with the app
itself. Hammering it could earn a `429` that lands on Antigravity's own
sign-in, not just on the card. ADR-0019's bound 3 (person-initiated only, floor
between calls) already prevents this; it should be understood as *also* an
obligation to the tool's session, not only a consent rule.

**Do not cache the derived access token in a way that survives a logout.**
openusage binds its cache to `SHA-256(refresh_token)`
(`AntigravityAuthStore.swift:70, 134-141`) so a signed-out or switched account
cannot reuse the previous account's access token. If TokenLedger caches at all,
copy that; the alternative is a card that keeps showing a stranger's quota. The
lazier option — don't cache, refresh once per person-initiated fetch, which the
≥60s floor already bounds — is probably the right one for v1.

---

## 3. The usage endpoint

### The authoritative one

```
POST https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary
Authorization: Bearer <access token>
Content-Type: application/json
Accept: application/json
```

Confirmed three ways: the path literal `/v1internal:retrieveUserQuotaSummary`
sits in the vendor binary inside the `google.api.http` annotation (`post:` with
`body: "*"`, hence POST-with-full-body); the gRPC method is
`google.internal.cloud.code.v1internal.PredictionService.RetrieveUserQuotaSummary`;
and an unauthenticated POST returns `401 UNAUTHENTICATED` with
`ErrorInfo.metadata.method` naming exactly that. Confidence: **high**.

### Request

`google.internal.cloud.code.v1internal.RetrieveUserQuotaSummaryRequest`:

| # | field | type | notes |
|---|-------|------|-------|
| 1 | `project` | string | `google.api.field_behavior = REQUIRED`, resource-typed |

**This is gap #1 in the prior art.** openusage sends an empty body `{}`
(`AntigravityProvider.swift:246`). The descriptor annotates `project` as
REQUIRED. Either the server does not enforce the annotation for this method, or
openusage's Cloud Code summary path silently fails and falls through to the
legacy chain — and because `parseQuotaSummary` returns `nil` on an undecodable
body, that fallthrough is *invisible except in a log line*. The robust order for
our Companion is: `v1internal:loadCodeAssist` → take
`cloudaicompanionProject` → pass it as `{"project": "<id>"}`. openusage already
does exactly this for the *legacy* `retrieveUserQuota` call
(`AntigravityProvider.swift:274-290`) and inexplicably not for the summary.
Confidence: **high** on the annotation, **not determined** on enforcement — I
did not spend the credential to find out.

### Response

`RetrieveUserQuotaSummaryResponse`:

| # | field | type | notes |
|---|-------|------|-------|
| 1 | `buckets` | repeated `QuotaSummaryBucket` | **`deprecated = true`** |
| 2 | `groups` | repeated `QuotaSummaryGroup` | the live one |
| 3 | `description` | string | |

`QuotaSummaryGroup`: `display_name` (2), `description` (3), `buckets` (1, repeated).

`QuotaSummaryBucket` — the payload that becomes a bar:

| # | field | json | type | notes |
|---|-------|------|------|-------|
| 1 | `bucket_id` | `bucketId` | string | pool+window identity |
| 2 | `display_name` | `displayName` | string | |
| 3 | `window` | `window` | **string** | the window, as a string |
| 4 | `remaining_fraction` | `remainingFraction` | float | **`oneof remaining`** |
| 5 | `remaining_amount` | `remainingAmount` | int64 | **`oneof remaining`** |
| 6 | `reset_time` | `resetTime` | `google.protobuf.Timestamp` | optional |
| 7 | `description` | `description` | string | |
| 8 | `disabled` | `disabled` | bool | |

Answering the ticket's three questions directly:

- **The balance** is `remaining_fraction` — *remaining*, 0…1, so **1.0 means
  untouched**. `used_pct = (1 - remaining_fraction) × 100`. Getting this
  backwards yields a card that reads 100% used on a fresh account.
- **The allowance** is not expressed. There is no denominator anywhere in the
  message — no limit, no total, no quota size. The fraction *is* the whole
  quantity. This is why the card cannot show "N of M".
- **The reset** is `reset_time`, an absolute `Timestamp`, `omitempty`.

Two presence subtleties that a naive parser gets wrong, both real:

- `remaining_fraction` lives in a `oneof`, so it has **true field presence**:
  absent is distinguishable from `0.0`. A bucket may legitimately send
  `remaining_amount` instead and no fraction at all. Defaulting an absent
  fraction to `0` prints "100% used"; defaulting it to `1` prints "untouched".
  Both are lies. Drop the bar.
- `reset_time` is genuinely optional and **is** absent in a normal, common
  state: a rolling 5-hour window that has not started yet. openusage renders
  that as the trailing label **"Not started"** rather than a countdown.

### Legacy endpoints (for completeness, and one trap)

`POST /v1internal:retrieveUserQuota` → `RetrieveUserQuotaResponse`, repeated
nested `BucketInfo`:

| # | field | type | notes |
|---|-------|------|-------|
| 1 | `remaining_amount` | int64 | `oneof remaining` |
| 5 | `remaining_fraction` | float | `oneof remaining` |
| 2 | `reset_time` | Timestamp | |
| 3 | `token_type` | enum | `TOKEN_TYPE_UNSPECIFIED` / `REQUESTS` / `WTUS` |
| 4 | `model_id` | string | e.g. `gemini-3-pro-preview` |

Per-model, 5-hour only, and `project` is REQUIRED here too (with field 2
reserved — something was removed).

`POST /v1internal:fetchAvailableModels` → models each carrying
`quota_info: QuotaInfo`. **The trap:** `QuotaInfo` is
`remaining_fraction` (1, float, **not** in a oneof) + `reset_time` (2). A plain
proto3 scalar has **no presence** — absent and `0.0` are the same bytes. So on
the legacy per-model path, "this model reports no quota" is genuinely
indistinguishable from "this model is exhausted". openusage's legacy branch
resolves that ambiguity by fabricating `0` (i.e. fully used) and then pooling
worst-fraction-wins, which is why its own code comments insist a parsed summary
must never fall through to legacy. **Recommendation: TokenLedger should not
implement the legacy path at all.** A card that can invent "100% used" is worse
than a card that says "no data". Confidence: **high**.

Both legacy endpoints exist server-side (`401 UNAUTHENTICATED` with
`...PredictionService.RetrieveUserQuota` and `...FetchAvailableModels`).

### Bucket IDs are server-assigned and unverifiable from the client

openusage matches four exact strings — `gemini-5h`, `gemini-weekly`, `3p-5h`,
`3p-weekly` (`AntigravityUsageMapper.swift:37-42`) — mapping to Gemini pool 5h,
Gemini pool weekly, non-Gemini ("3p" = third-party: Claude, GPT-OSS) 5h, and
non-Gemini weekly.

**None of these literals appear in the vendor binary.** I grepped for them and
for `gemini-daily`/`gemini-monthly` variants: zero hits. They are assigned by
Google's backend and only ever appear in responses, so they cannot be
corroborated from the client side and can change without any client update.
openusage's decision to match on **exact `bucket_id` only** — never inferring
pool identity from `display_name` or `window` — is the right call (a future
`gemini-image-5h` must not silently join the Gemini pool), but it means an
unrecognised bucket is silently dropped. Confidence: **medium** on the four
strings (openusage's testimony, plausible, uncorroborated); **high** that they
are not client-side constants.

The `window` string's vocabulary is likewise server-side and unknown. Do not
key windows off it.

---

## 4. Mapping onto the v1 window model

Per the v1 design
(`docs/superpowers/specs/2026-08-12-limits-page-v1-design.md`, SCHEMA_V14):

```sql
limit_readings(source, window_key, window_minutes, used_pct,
               resets_at, observed_at, via, plan)
PRIMARY KEY (source, window_key, resets_at, used_pct)
```

The mapping, for the summary endpoint only:

| reading field | value |
|---|---|
| `source` | `"antigravity"` (existing catalog key, `src/source-catalog.json`) |
| `window_key` | `w300` for `*-5h`, `w10080` for `*-weekly`, **plus a pool discriminator** |
| `window_minutes` | 300 / 10080 — both already in the canonical set |
| `used_pct` | `(1 - remaining_fraction) × 100` |
| `resets_at` | `reset_time` → unix seconds |
| `observed_at` | fetch time |
| `via` | `"live"` |
| `plan` | `UserTier.name` (§5) |

Four things do not fit, in decreasing order of how much they should worry us:

### (a) `window_key` alone cannot address an Antigravity bar — this is a schema problem

Antigravity has **two pools × two windows = four bars**, and the two pools share
both durations. Codex and Claude each have one bar per window, so v1's
`window_key = w{minutes}` is injective for them. For Antigravity it collides:
`gemini-5h` and `3p-5h` are both `w300`, and they are *different quotas with
different fill levels*. Since `window_key` is in the primary key, the two pools
would overwrite each other's readings.

The fix is small but it is a **schema-semantics decision that v1 has not made**,
so it belongs in the ticket rather than in an implementation: let `window_key`
carry the pool, e.g. `gemini:w300`, `gemini:w10080`, `3p:w300`, `3p:w10080`,
while `window_minutes` keeps the canonical duration for classification. That
preserves the "never key off the slot, always key off the duration" rule for
*classification* while making the key actually unique. Note the v1 spec already
half-anticipates this by writing `window_key` as `w{canonical minutes}` and
separately insisting the key never encode a slot — Antigravity is the first
source where the pool is not a slot but a genuine second axis.

Confidence: **high** that this collision is real.

### (b) `resets_at INTEGER NOT NULL` versus a window that has not started

A rolling 5-hour window with no usage yet has **no `reset_time`** — the window
begins at the first message. `resets_at` is `NOT NULL` *and part of the primary
key*, so there is nowhere to put "not started".

This is the same shape as [#107](https://github.com/BrianWong05/TokenLedger/issues/107)
(session-anchored window, no tick) approached from the other end: #107 is about
an *expired* epoch having no next reset; this is a *pending* epoch having no
first reset. Both are "a rolling window with no anchor". Recommended handling,
matching openusage's "Not started": **do not store a reading at all** — a
window that has not started has no usage to record, and the spec already
establishes the precedent that "a null slot is a window that does not exist — no
Reading, no bar." Render the bar at 0% with a "not started" trailing label from
the *absence* of a row, or store `used_pct = 0` with a sentinel only if the
history curve is wanted. Storing a fabricated `resets_at` would corrupt the
`max(resets_at)` epoch derivation. Confidence: **high** on the problem,
**medium** on the recommendation (it is a design call, not a fact).

### (c) `remaining_amount` has no home — and this is the ticket's real question

Here is where the ticket's "credit-based" framing lands, correctly:

> "If a credit balance has no window or reset instant, does it render as a bar
> at all, or as a figure?"

A bucket sending `remaining_amount` (int64 count, `token_type` `REQUESTS` or
`WTUS`) instead of `remaining_fraction` **cannot become a bar**, because
`used_pct` needs a denominator and the message carries none. A count of
remaining units with no total is a **figure, not a bar**.

`remaining_amount` buckets *do* still carry `reset_time`, so it is not the
window that is missing — only the allowance. So the honest v1 answer is:

- **v1 should render nothing for an amount bucket** and say so, rather than
  invent a denominator. It is not observed in practice today (openusage has
  shipped for months reading only fractions, and its users see meters), so this
  is a defensive case, not a feature.
- The Companion should nonetheless **detect and report it** — log the bucket id
  and the fact that it sent an amount — because it is the one wire change that
  would silently empty the card. Prior art fails exactly here: openusage's
  `QuotaSummaryBucket` decoder (`AntigravityUsageMapper.swift:262-275`) has no
  `remainingAmount` field at all, so such a bucket "has no usable
  remainingFraction", drops its line, and reads as **"No data"** with no hint
  that a perfectly good balance was on the wire.
- If it ever *is* observed, a figure tile (`used_pct` is the wrong column for
  it) is the honest presentation, which is a v2 schema question.

Confidence: **high** that the oneof exists and has no denominator; **high**
that openusage drops it; **low** on how often it actually occurs (never seen).

### (d) `disabled`, and a parallel API family

`QuotaSummaryBucket.disabled` (bool) is a fourth state beyond
used/unused/no-data: the pool exists but is turned off for this account.
openusage ignores it. Rendering a disabled pool as a 0%-used bar would be a
lie. Treat it as "no bar".

The binary also carries `google.cloud.businessaicode.v1main` /
`.v1beta` — a parallel, newer API family with `FetchQuotaStatus` returning the
*same* `QuotaSummaryGroup`/`QuotaSummaryBucket` shape plus
`OverageInfo { enabled: bool }`. That is the Gemini-Enterprise-flavoured
surface, and `overage_info` implies quotas that can be *exceeded and billed*
rather than merely exhausted — which no percentage bar can express. Out of
scope for v1; worth knowing the shape is already staged for it. Confidence:
**high** that the messages exist, **not determined** whether any Antigravity
build routes there.

---

## 5. Plan label

**`UserTier.name`** — the Antigravity equivalent of Claude's `rateLimitTier`.

`google.internal.cloud.code.v1internal.UserTier`:

| # | field | type |
|---|-------|------|
| 1 | `id` | string |
| 2 | `name` | string |
| 3 | `description` | string |
| 4 | `user_defined_cloudaicompanion_project` | bool |
| 5 | `privacy_notice` | `UserTier.PrivacyNotice` |

Reachable two ways:

- **Language server** — `GetUserStatus` → `userStatus.userTier.name`.
- **Cloud Code** — `v1internal:loadCodeAssist` → `currentTier.name` /
  `paidTier.name` (prefer the paid one), the same call that yields
  `cloudaicompanionProject` for §3's `project` argument. One call, two needs.

There is also `Entitlement.user_tier`, a bare **string** (not a message) — field
1 in the `v1internal` namespace, field 2 in `businessaicode` — so don't assume
`.name` on everything called `user_tier`.

**Prefer `userTier` over the inherited Windsurf `planStatus.planInfo.planName`**,
which openusage reports reads "Pro" for every paid tier
(`AntigravityUsageMapper.swift:93-94`). Confidence: **high** on the field
shapes, **medium** on the Windsurf-field claim (openusage's testimony only, but
it is consistent with the Codeium lineage).

The vendor binary also carries the authoritative tier taxonomy, as a telemetry
enum (`logs.proto...AntigravityExtension.UserTier`) — useful for knowing what
strings to expect:

```
USER_TIER_UNSPECIFIED=0  AIDA_TIER=1              CS_STANDARD_TIER=2
AGY_BUSINESS_PAYGO_TIER=3  ENTERPRISE_AGENT_TIER=4  FREE_TIER=5
G1_PRO_TIER=6            G1_ULTRA_TIER=7          G1_ULTRA_LITE_TIER=8
GCP_ENTERPRISE_TIER=9    GCP_GE_PLUS_TIER=10      GCP_GE_PAYGO_TIER=29
G1_PLUS_TIER=30          (GCP_GE_STANDARD…, truncated)
```

`G1` = Google One. So the consumer plans are Free / Pro / Ultra / Ultra Lite /
Plus, and there are five-plus enterprise flavours. Note this is the *enum*, not
the display string: `UserTier.name` returns human text like "Google AI Pro",
which openusage normalises by stripping the `Google AI ` prefix or pulling the
tier word out of "Gemini Code Assist in Google One AI Pro"
(`AntigravityUsageMapper.swift:215-224`). Expect to do the same; a pill reading
"Gemini Code Assist In Google One AI Pro" will not fit beside the tool name.

Consider persisting `UserTier.id` alongside the display name if the plan is ever
used for logic rather than display — `name` is marketing copy and will churn.

---

## 6. Staging host verdict: real host, same backend, and openusage's ordering is a latent bug

`daily-cloudcode-pa.googleapis.com` is **not dead code, but it is not a second
data source either.** Findings:

- It appears **once** as a string literal in Antigravity's own
  `language_server`, alongside five occurrences of
  `cloudcode-pa.googleapis.com`. So Antigravity itself carries the name —
  openusage copied it from the vendor, it isn't invented. In the app it is
  almost certainly behind a dev flag, given the 1:5 ratio.
- It **resolves and serves**: DNS gives the same eight Google frontend IPs as
  production (anycast GFE, so DNS proves nothing on its own), and an
  unauthenticated POST to `/v1internal:retrieveUserQuotaSummary` returns
  `401 UNAUTHENTICATED`, i.e. the surface is there.
- **It self-identifies as production's service.** The decisive evidence — the
  `daily-` host's own error body reports:

  ```json
  "metadata": {
    "method": "google.internal.cloud.code.v1internal.PredictionService.RetrieveUserQuotaSummary",
    "service": "cloudcode-pa.googleapis.com"
  }
  ```

  byte-identical to production's. `daily-` is a release-channel *frontend*
  onto the same registered service, not a separate staging deployment with its
  own data.

**Verdict: drop it. TokenLedger's Companion should call
`cloudcode-pa.googleapis.com` and nothing else.** It cannot return different
numbers, and there is an active reason to avoid it: openusage lists it
**first** (`AntigravityUsageClient.swift:23-26`) and its `cloudCode(...)` loop
**short-circuits the whole base-URL list on 401/403**
(`AntigravityUsageClient.swift:91`) on the reasoning that "same token would fail
on the other base". So *if* the canary channel ever rejects a token production
would accept — a plausible thing for a canary to do — openusage returns
`.authFailed`, triggers a Google OAuth refresh, retries, fails again, and
reports `authExpired`: **a signed-out card for a perfectly good session, and it
would never try production.** Today the two behave alike so nobody notices.
That is one Google release away from being a user-visible bug, and it is a free
one for us to not inherit. Confidence: **high** on the service identity,
**high** on the short-circuit reading, **medium** on whether the canary could
ever diverge.

---

## What I could not determine

- **Whether the server enforces `RetrieveUserQuotaSummaryRequest.project`.**
  The descriptor annotates it REQUIRED; openusage omits it. Settling this needs
  one authenticated call, which means spending the credential — out of bounds
  for research under ADR-0019. **Pass `project` anyway**; it costs one extra
  `loadCodeAssist` that we want regardless for the plan label.
- **A real response body.** Every field name, number, type and `oneof` here is
  from the vendor's descriptors, which is *stronger* than a sample for shape —
  but it means I have never seen actual `bucket_id` values, actual `window`
  strings, or a real `remaining_fraction`. The four bucket ids rest on
  openusage's testimony alone.
- **The client secret's location** (classifier-blocked; see §2). Immaterial.
- **Non-macOS credential storage.** Probably Secret Service / WinCred via
  `go-keyring`, unverified.
- **Whether `remaining_amount` is ever actually sent** for Antigravity's
  consumer pools. It is in the schema, it is the *older* field, and no client I
  looked at reads it.
- **Whether any build routes to `businessaicode.v1main` `FetchQuotaStatus`**,
  and therefore whether `OverageInfo` matters.
- **Whether the LS `force_refresh` flag bypasses a server-side cache**, and so
  whether repeated Companion calls would even see fresh numbers. The LS wrapper
  `exa.language_server_pb.RetrieveUserQuotaSummaryRequest` has
  `request` (1) + `force_refresh` (2), and the binary contains
  `codeassistclient.Cache[...RetrieveUserQuotaSummaryResponse]` — so the
  language server **does** cache quota summaries. The Cloud Code endpoint has no
  such flag. Worth knowing before promising "checked just now".

## Recommended card

A **four-bar percentage card**, structurally identical to Claude's, no new
presentation primitives:

```
[icon] Antigravity                                    [Pro]
       checked 2m ago
  Gemini · session   ▓▓▓▓▓▓░░░░  58%   resets in 2h 14m
  Gemini · weekly    ▓▓▓░░░░░░░  31%   resets Sunday
  Claude · session   ░░░░░░░░░░   —    not started
  Claude · weekly    ▓▓░░░░░░░░  18%   resets Sunday
```

- Bars, not figures. `used_pct = (1 - remaining_fraction) × 100`.
- Label the **pool** as well as the window — the pool is a real second axis
  here, unlike every other source. openusage's "Session / Weekly / Claude /
  Claude Weekly" naming exists to match its other providers' rows and reads
  poorly standalone ("Claude" as a row on the *Antigravity* card is
  confusing); "Gemini" / "Claude" as pools with "session" / "weekly" as windows
  is clearer, and `3p`'s real meaning is "non-Gemini", so if GPT-OSS usage ever
  dominates that pool the label "Claude" becomes wrong. Consider
  "Other models".
- Four states per bar, all reachable: **normal** (fraction + reset),
  **not started** (fraction present, no `reset_time` — 0%, no countdown),
  **no data** (bucket absent, or fraction absent, or `disabled`), and
  **disabled**. Never fabricate a percentage for the last two.
- `plan` pill from `UserTier.name`, normalised to one word.
- Acquisition: `capabilities.limits: "live"` in `src/source-catalog.json` (the
  `antigravity` entry has no `limits` key today), behind the same opt-in gate
  and the same ≥60s floor as Claude. 401/403 → signed-out card pointing at
  Antigravity's own sign-in, per ADR-0019 bound 4.
- Only the summary endpoint. No legacy fallback (§3): its `QuotaInfo` cannot
  distinguish absent from exhausted, and a card that invents "100% used" is
  worse than one that says "no data". If an Antigravity build predates
  `RetrieveUserQuotaSummary`, the honest card is "update Antigravity".

## Sources

- `/Applications/Antigravity.app/Contents/Resources/bin/language_server`
  (Antigravity 2.5.0, `sha256 adde7038…5eeaff`, 2026-07-30) — embedded
  `FileDescriptorProto` for `google.internal.cloud.code.v1internal`,
  `exa.language_server_pb`, `google.cloud.businessaicode.v1main`/`v1beta`.
- macOS Keychain item `service=gemini` / `account=antigravity` on this machine
  (attributes and field names only).
- Google, unauthenticated `401` bodies from `cloudcode-pa.googleapis.com` and
  `daily-cloudcode-pa.googleapis.com` for
  `v1internal:retrieveUserQuotaSummary`, `:retrieveUserQuota`,
  `:fetchAvailableModels`.
- `developers.google.com/identity/protocols/oauth2`,
  `…/oauth2/native-app`, `…/oauth2/web-server`.
- openusage `487cc8f19a9a28676f6924aafa48dee79ad7a7f6`:
  `Sources/OpenUsage/Providers/Antigravity/{AntigravityAuthStore,AntigravityUsageClient,AntigravityUsageMapper,AntigravityProvider,AntigravityMetric}.swift`,
  `docs/providers/antigravity.md`.
- This repo: `docs/adr/0018-*.md`, `docs/adr/0019-*.md`,
  `docs/superpowers/specs/2026-08-12-limits-page-v1-design.md`,
  `docs/source-evidence/antigravity.md`, `src/source-catalog.json`,
  `src-tauri/src/bin/antigravity-export.rs`.
