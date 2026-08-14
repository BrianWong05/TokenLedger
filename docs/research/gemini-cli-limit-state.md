# Gemini CLI limit state

Status as of 2026-08-12: **Gemini CLI gets a card, `capabilities.limits: "live"`,
fetched by a Companion.** The ticket's framing — a request-capped free tier, so
count today's requests from the ledger and gauge against a published number — is
answerable but wrong. Gemini CLI has a real quota endpoint that has been in its
tree since 2025-11-27, and that endpoint returns a **fraction and a reset
instant**: precisely the two things `limit_readings` stores. There is nothing to
derive, no cap to hardcode, and no timezone to guess.

The premise the ticket inherited is stale. It is true that Google publishes the
allowance as *requests per user per day* (1,000 / 1,500 / 2,000 by tier), and
that is what the prose docs talk about. But the wire format is
`remainingFraction`, and Gemini CLI itself renders it as a percentage bar with a
reset countdown — the same widget the Limits page is specced to draw. The unit
question dissolves before it reaches our schema.

What this does cost is a credential decision that #112 cannot make on its own:
Google access tokens live about an hour, and ADR-0019 bound 1 forbids the
Companion from spending a refresh token. That collision is shared with #110
(Antigravity, same host, same RPC, same one-hour tokens) and is the one blocker
worth escalating. See [The credential collision](#the-credential-collision).

Primary sources: Gemini CLI's own source read at
`google-gemini/gemini-cli@659c7aacd96f6632f19e2fac0796db83a2f97e6b` (main,
2026-08-11); Google's published quota pages; openusage's independent
implementation of the same RPC; and 48 session artifacts under
`~/.gemini/tmp/*/chats/` on this machine (field names only — no values left this
document).

## The endpoint exists

```
POST https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota
Authorization: Bearer <access_token>
Content-Type: application/json

{ "project": "<cloudaicompanionProject>", "userAgent": "<optional>" }
```

Host and version are constants at
`packages/core/src/code_assist/server.ts:73-74`
(`CODE_ASSIST_ENDPOINT = 'https://cloudcode-pa.googleapis.com'`,
`CODE_ASSIST_API_VERSION = 'v1internal'`), assembled by `getMethodUrl` as
`` `${endpoint}/${version}:${method}` `` at `:532-534`. The method itself is
`retrieveUserQuota` at `:367-374`, a plain `requestPost`.

The response shape, `packages/core/src/code_assist/types.ts:250-265`, verbatim:

```ts
export interface RetrieveUserQuotaRequest {
  project: string;
  userAgent?: string;
}

export interface BucketInfo {
  remainingAmount?: string;
  remainingFraction?: number;
  resetTime?: string;
  tokenType?: string;
  modelId?: string;
}

export interface RetrieveUserQuotaResponse {
  buckets?: BucketInfo[];
}
```

Age and stability: those types landed 2025-11-27 in
`69188c8538af` — *"Add usage limit remaining in /stats (#13843)"* (GitHub blame
of `types.ts` lines 249-265). Nine months in tree, still the shape `main` uses,
and independently spoken by a third-party client (below). For an undocumented
`v1internal` API that is about as much stability evidence as exists.
Confidence: **high** that the endpoint and field names are current; the response
body itself was **not** probed live (see [What I could not
determine](#what-i-could-not-determine)).

## It already returns used_pct and resets_at

Gemini CLI does the conversion twice, both times to a percentage:

- `packages/cli/src/ui/components/QuotaDisplay.tsx:37` —
  `const usedPercentage = 100 - (remaining / limit) * 100;`
- `packages/cli/src/ui/components/ModelQuotaDisplay.tsx:174-175` —
  `const usedFraction = 1 - data.remainingFraction;` then
  `const usedPercentage = usedFraction * 100;`

`ModelQuotaDisplay` is worth reading in full, because it is the card we are about
to build. It groups buckets by model *tier* (`pro` / `flash` / `flash-lite`, from
`config.modelConfigService.getModelDefinition(modelId).tier`, falling back to the
raw `modelId`), keeps the **worst** bucket per tier
(`if (!existing || remainingFraction < existing.remainingFraction)`, `:154`), and
renders one row per tier: name, progress bar, `NN%`, `Resets: <countdown>`
(`:83-105`). That is a limit card.

Ingest into `limit_readings` therefore needs no arithmetic beyond a multiply:

| Column | Source | Note |
| :--- | :--- | :--- |
| `source` | `'gemini'` | |
| `window_key` | bucket's model tier (`pro`/`flash`/`flash-lite`), else `modelId` | one row per tier, keeping the lowest `remainingFraction` — the same merge `ModelQuotaDisplay.tsx:149-171` and openusage both do |
| `window_minutes` | `NULL` | nullable in SCHEMA_V14. The daily mechanic is undocumented and `resetTime` is authoritative; writing `1440` would be a guess dressed as a reading |
| `used_pct` | `(1 - remainingFraction) * 100` | the vendor's own figure, scaled. Never derived from counts |
| `resets_at` | `resetTime` (ISO) → unix seconds | the same conversion the spec already specifies for Claude |
| `observed_at` | Companion fetch time | |
| `via` | `'live'` | `'logs'` is impossible; see [No local state](#no-local-state-so-vialogs-is-out) |
| `plan` | `currentTier.id` from `loadCodeAssist` | `free-tier` / `legacy-tier` / `standard-tier`, or a server-supplied string (`types.ts:150-156`) |

Two implementation notes for whoever builds it:

- **Round to integer percent at ingest.** The spec's primary key is
  `(source, window_key, resets_at, used_pct)` and its "≤101 rows per window per
  epoch" bound depends on integer resolution. `remainingFraction` is a float; an
  unrounded `used_pct` turns the append-only fill-curve into unbounded growth.
- **`remainingAmount` is a real count — do not store it in v1.** It is an int64
  as a string, and `config.ts:2333-2338` reconstructs an absolute limit from it:
  `limit = Math.round(remaining / bucket.remainingFraction)`. When the server
  omits it, the CLI falls back to a normalized scale of 100 (`:2340-2342`) —
  i.e. the CLI treats the fraction as the load-bearing field and the count as a
  bonus. v1 has no column for counts, and #110's unresolved
  "how do credits/counts render" question is the right place to decide that.
  Note it as the upgrade path, not as scope.

The `project` field is required, so the Companion needs a `loadCodeAssist` call
first to obtain `cloudaicompanionProject` — which conveniently also yields
`currentTier` for the `plan` column
(`LoadCodeAssistResponse`, `types.ts:81-87`).

## Auth mode decides everything

`retrieveUserQuota` is a method on `CodeAssistServer`, and `CodeAssistServer` is
constructed for exactly two auth types
(`packages/core/src/code_assist/codeAssist.ts:15-40`):

```ts
  if (
    authType === AuthType.LOGIN_WITH_GOOGLE ||
    authType === AuthType.COMPUTE_ADC
  ) {
    ...
    return new CodeAssistServer(...);
  }

  throw new Error(`Unsupported authType: ${authType}`);
```

`getCodeAssistServer` returns `undefined` for anything else (`:58-61`), and
`refreshUserQuota` bails on that (`config.ts:2307-2311`). So:

| Auth mode | Live quota surface | Card |
| :--- | :--- | :--- |
| Personal Google login (`LOGIN_WITH_GOOGLE`) | **Yes** — `retrieveUserQuota` | **Yes, `via='live'`** |
| Google Workspace / Code Assist licence (same OAuth path) | **Yes** — same RPC | **Yes, `via='live'`** |
| Compute ADC (`COMPUTE_ADC`) | Yes by code path | Edge case; treat as the OAuth path |
| Gemini API key (`USE_GEMINI`) | **No** — no `CodeAssistServer` at all | **No** |
| Vertex AI (`USE_VERTEX_AI`) | **No** | **No** |

The API-key and Vertex exclusion is doubly determined, which is a comfort: the
code has no endpoint for them, *and* the map already rules that BYO-key sources
with no vendor subscription quota get no card. An API-key user's ceiling is a
Cloud project quota against a billing account, not a subscription window —
ai.google.dev states it outright: *"Rate limits are applied per project, not per
API key."* Vertex PAYG is dynamic shared quota, i.e. not a window at all.

Per map decision 9 ("a missing card reads as unsupported, which is a lie"),
Gemini still gets a card in these modes — in a disabled state whose copy says the
quota belongs to an API project, not to a plan we can gauge. Confidence: **high**.

## No local state, so via='logs' is out

The quota the CLI displays is held in memory and thrown away:
`private modelQuotas: Map<...>` at `packages/core/src/config/config.ts:853-858`
alongside `lastRetrievedQuota` and `lastQuotaFetchTime`, all cleared together at
`:1852-1854`. Nothing writes it to disk. Grepping `packages/core/src` for a quota
cache path finds nothing.

Confirmed against this machine. Across all 48 session artifacts under
`~/.gemini/tmp/*/chats/` (both the older `session-*.json` object form and the
newer `session-*.jsonl` append form, whose first line carries a `kind` header and
whose later lines alternate message records with `$set` patches), the complete
union of keys matching `token|usage|quota|limit|count|model` is:

```
limit, model, model_added_chars, model_added_lines,
model_removed_chars, model_removed_lines, tokens, userMessageCount
```

The only `limit` in that list is `toolCalls[].args.limit` — a `read_file`
argument. `tokens` is the per-message usage block
(`cached, input, output, thoughts, tool, total`) the adapter already reads.
Session-level keys are `sessionId, projectHash, startTime, lastUpdated, messages,
model, tokens, userMessageCount`; `~/.gemini/tmp/*/logs.json` holds
`message, messageId, sessionId, timestamp, type`. **No window, no reset, no
allowance, anywhere in the artifacts the scan reads.** Confidence: **high** —
this is the same negative result from two independent directions.

Nor does quota ride along on generation responses. `refreshUserQuotaIfStale` is
called *after* requests from `packages/core/src/core/loggingContentGenerator.ts:422,573`
as a separate RPC; `remainingFraction` appears nowhere in the content-generation
path. There is no header or metadata to sniff.

## Local derivation is feasible and should be refused

It has to be addressed on its merits, because the ticket asks for it and the
plumbing genuinely exists. `src-tauri/src/adapters/gemini.rs:165-194` writes one
event per message with `api_calls: 1`, and `SUM(api_calls)` is already the
Requests metric (`src-tauri/src/queries.rs:189`). "Gemini requests today" is a
`queries::summary` call with `tools: ["gemini"]` and today's bounds. No new
storage. It would take an afternoon.

Five reasons it would be a lie, four of them measured on this machine's corpus:

1. **The denominator's unit is undocumented at the granularity that matters.**
   200 `user` messages produced **1,559** `gemini` messages here — 7.8 model
   responses per prompt, because each tool-loop round trip is its own message.
   Google's docs say "model requests," which is *probably* the round trip, but no
   primary source says whether the daily counter increments per model round trip
   or per user prompt. Guessing wrong is an 8x error, in the direction that tells
   a user they are out of quota when they have 87% left.
2. **The numerator systematically misses exactly the attempts the cap counts.**
   1,558 of 1,559 `gemini` messages carried a `tokens` block (99.94% — excellent
   capture of *successful* turns), and the adapter `continue`s past any message
   without one (`gemini.rs:135`). But the corpus also holds **9 `error`
   messages**, which produce no event. A 429'd or refused request writes no usage
   block at all, so the count is successful token-bearing turns while the cap
   counts attempts — diverging worst precisely when the gauge matters.
3. **There is no reset anchor in the data.** `queries.rs:628` buckets by *local*
   day. Google's day boundary for Code Assist is unstated (below). A gauge whose
   window is off by hours is a gauge that resets at the wrong time every day.
4. **`~/.gemini/tmp/` is a temp path.** Ledger rows survive, but the numerator is
   only as complete as what got scanned before something cleaned that directory.
5. **The published cap is neither uniform nor demonstrably stable.** Google
   already moved the Gemini API numbers off their docs page into AI Studio
   (below). A hardcoded table would rot, which map decision 2 exists to prevent.

This is the Claude-derivation failure exactly: a number that *looks* exact and
drifts silently, in an app whose credibility rests on *Where the numbers bend*.
Recommend an explicit line on the map's Out of scope list — *"Deriving Gemini
CLI's daily request allowance locally from ledger `api_calls`"* — citing this
document, so the idea does not get re-proposed on the strength of "but the cap is
published." It is published. It is still not derivable honestly, and it is moot
anyway now that the endpoint hands us the fraction.

## The published caps, and what they do not say

Gemini CLI's own doc, `docs/resources/quota-and-pricing.md` at the commit above,
verbatim:

| Authentication method | Tier / Subscription | Maximum requests per user per day |
| :-------------------- | :------------------ | :-------------------------------- |
| **Google account**    | Gemini Code Assist (Individual) | 1,000 requests |
|                       | Google AI Pro       | 1,500 requests |
|                       | Google AI Ultra     | 2,000 requests |
| **Gemini API key**    | Free tier (Unpaid)  | 250 requests |
|                       | Pay-as-you-go (Paid)| Varies |
| **Vertex AI**         | Express mode (Free) | Varies |
|                       | Pay-as-you-go (Paid)| Varies |
| **Google Workspace**  | Code Assist Standard| 1,500 requests |
|                       | Code Assist Enterprise | 2,000 requests |
|                       | Workspace AI Ultra  | 2,000 requests |

Same page, on the per-minute limit: *"Requests are limited per user per minute
and are subject to the availability of the service in times of high demand."*
**No number is given.** The API-key row is further qualified as *"Model requests
to Flash model only."*

Corroboration is partial. Google Cloud's quota page —
`https://docs.cloud.google.com/gemini/docs/quotas`, where
`developers.google.com/gemini-code-assist/resources/quotas` now 301s — confirms
only two rows: *Maximum requests per user per day*, Standard **1500**,
Enterprise **2000**. It carries **no** rows for Individual free, AI Pro, or AI
Ultra. So the 1,000 / 1,500 / 2,000 individual figures rest on gemini-cli's own
doc alone. Confidence: **medium** on the individual-tier numbers, **high** on
Standard/Enterprise.

The Gemini API free-tier figure is worse. gemini-cli's doc says 250 requests/day
and links `https://ai.google.dev/gemini-api/docs/rate-limits`, but that page no
longer publishes numbers: *"Rate limits depend on a variety of factors (such as
your usage tier) and can be viewed in Google AI Studio."* A cap that moved from
a docs page into a console is a cap that answers "is it published and stable?"
with **no**. Recorded because the ticket asks, not because it changes the
verdict — the API-key mode gets no card either way.

### Reset semantics

- **Code Assist / Google login (the mode that gets a card): undocumented.**
  Neither gemini-cli's doc nor Google Cloud's quota page says whether the daily
  allowance is a calendar day in some timezone or a rolling 24 hours. Cloud's
  page goes exactly this far and no further: *"Once the maximum number of
  requests per day is reached, no further requests can be made through these
  interfaces to any model until the quota resets."*
- **Gemini API key (no card): documented as a calendar day, US Pacific.**
  ai.google.dev: *"Requests per day (RPD) quotas reset at midnight Pacific
  time."* It is tempting to assume Code Assist matches. Do not write that
  assumption into the schema.

This is why `resets_at` must come from the server's `resetTime` and
`window_minutes` must stay `NULL`. The reading is honest without us ever knowing
the mechanic — the vendor tells us the instant, and the card counts down to it.
That is strictly better than the published-cap-plus-timezone-guess design the
ticket contemplated, and it is the reason the undocumented mechanic is a footnote
rather than a blocker. Confidence: **high** that `resetTime` is server-supplied
and authoritative; **could not determine** the underlying mechanic.

## Why openusage and tokscale have no Gemini CLI provider

The surface is not missing. The credential adapter is.

openusage's **Antigravity** provider already speaks these exact RPCs.
`Sources/OpenUsage/Providers/Antigravity/AntigravityUsageClient.swift:24-31`:

```swift
    "https://daily-cloudcode-pa.googleapis.com",
    "https://cloudcode-pa.googleapis.com"
    static let fetchModelsPath = "/v1internal:fetchAvailableModels"
    static let loadCodeAssistPath = "/v1internal:loadCodeAssist"
    static let retrieveQuotaPath = "/v1internal:retrieveUserQuota"
    static let quotaSummaryPath = "/v1internal:retrieveUserQuotaSummary"
    static let googleOAuthURL = "https://oauth2.googleapis.com/token"
```

Same host, same `v1internal`, same `retrieveUserQuota`. What differs is only
*whose* credential it presents: `AntigravityAuthStore.swift:5,16` reads the macOS
Keychain item `service: gemini, account: antigravity`. Its provider doc describes
merging per-model buckets "by keeping each pool's worst remaining fraction" and
notes *"Quotas are reported as a fraction (full = 0% used), so there are no token
or dollar spend tiles"* — independent confirmation of both the wire format and
the merge rule read out of gemini-cli's source.

A search of openusage for `gemini` returns 20 files, every one Antigravity or
model-pricing; its provider directory holds Antigravity, Claude, Codex, Copilot,
Cursor, Devin, Grok, OpenCode, OpenRouter, Pi, ZAI — no Gemini. tokscale has
`crates/tokscale-core/src/sessions/gemini.rs`, a session **log** parser for
usage, and no limits provider at all.

So the answer to the ticket's opening question is the second option: **nobody
built it**, and a third party has already proven the endpoint is reachable from a
Google credential lifted off local disk. That is corroboration, not permission —
but it does retire the worry that the absence signalled an unreachable surface.

One lead this turns up: **`retrieveUserQuotaSummary` exists on the same host and
gemini-cli does not use it** (grep for it across `packages/` returns nothing).
openusage calls it *first*, describing it as "the only endpoint that reports the
merged pools and the weekly windows," falling back to `retrieveUserQuota` for
older builds. If it answers for Code Assist accounts, it may hand us merged
buckets and additional windows for free. Unverified — worth one probe during
implementation, not worth a design bet.

## The credential collision

This is the real cost, and it is not #112's to settle.

Reading the credential is *easy* here — easier than Claude. Gemini CLI writes its
OAuth document as plaintext JSON at `~/.gemini/oauth_creds.json`, mode `0600`
(`OAUTH_FILE = 'oauth_creds.json'` at `packages/core/src/config/storage.ts:22`,
written by `cacheCredentials` at `packages/core/src/code_assist/oauth2.ts:799-810`).
The field names present on this machine are `access_token`, `expiry_date`,
`id_token`, `refresh_token`, `scope`, `token_type`. No Keychain, no ACL prompt,
no `security` invocation.

The problem is lifetime. **Google access tokens live about an hour** — hence the
`expiry_date` field and the CLI's own re-caching. ADR-0019 bound 1 is explicit:
the Companion *"reads the credential document and never writes or refreshes it —
never spends the refresh token."* Claude survives that bound because its OAuth
access token is long-lived. Gemini does not.

Two honest resolutions, and the choice is an ADR-level one:

1. **Keep bound 1 exactly.** Read `access_token`; if `expiry_date` has passed,
   render the card unavailable and point at `gemini`, per bound 4. Correct,
   cheap, and the card would be live only within an hour of the user last running
   Gemini CLI — for most users, most of the time, a permanently unavailable card.
2. **Amend bound 1 for the Google family.** Let the Companion exchange the
   refresh token at `https://oauth2.googleapis.com/token` and cache the result in
   TokenLedger's *own* file, never writing `~/.gemini/oauth_creds.json`. This is
   openusage's exact pattern (its doc: *"An expired token is refreshed
   automatically (OpenUsage never writes back to Antigravity's own keychain
   item)"*), and its safeguard is worth copying too: bind the cache to a one-way
   fingerprint of the current refresh credential so a logout or account change
   cannot reuse the previous account's access token. The property bound 1 exists
   to protect — never corrupting the source's own session, the tokscale failure —
   survives intact. What is spent is a refresh token, which the vendor's own
   client spends hourly.

Option 2 is the one that ships a working card, and it is the same decision #110
(Antigravity) needs, on the same host, against the same token lifetime. **Decide
it once — on #110 or in a new ADR amending 0019 — and let #112 inherit it.** If
it is refused, #112 degrades to option 1 rather than to no card.

Refresh cadence, by contrast, is settled and comfortable: Gemini CLI's own floor
is 30 seconds (`refreshUserQuotaIfStale(staleMs = 30_000)`,
`config.ts:2372-2379`), triggered after requests and on `/stats` and `/model`
(`statsCommand.ts:67`, `modelCommand.ts:57`). ADR-0019 bound 3 — page-open or
manual only, never a timer — is strictly more conservative than the vendor's own
client. No tension.

Failure modes for bound 4 are well-signposted in the source: 429/499 carrying a
`QuotaFailure` whose `quotaId` contains `PerDay` or `Daily` is a
`TerminalQuotaError` (daily exhausted), versus `RATE_LIMIT_EXCEEDED` /
`RetryInfo.retryDelay` for the per-minute limit
(`packages/core/src/utils/googleQuotaErrors.ts:205-330`). 403 with
`VALIDATION_REQUIRED` is its own state. A Companion can map these to card states
without inventing vocabulary.

## Recommendation

1. `source-catalog.json`: `gemini` gains `capabilities.limits: "live"`.
2. A `gemini-export` Companion on the `antigravity-export` template: read
   `~/.gemini/oauth_creds.json` → `loadCodeAssist` (for
   `cloudaicompanionProject` and `currentTier.id`) → `retrieveUserQuota` → write
   an Export Artifact of Limit Readings shaped as the table above. Probe
   `retrieveUserQuotaSummary` first and fall back, mirroring openusage.
3. One row per model tier, worst-fraction-wins, `window_minutes = NULL`,
   `used_pct` rounded to integer percent, `resets_at` from the server's
   `resetTime`.
4. API-key and Vertex users get a card in a disabled state explaining that the
   quota is an API project's, not a plan's — never a gauge.
5. Block on the ADR-0019 bound-1 amendment, jointly with #110.
6. Add the local-derivation refusal to the map's Out of scope list.

Sequencing: this lands *after* v1's Claude Companion, because it reuses that
Companion's Export Artifact ingest path wholesale and because the credential
question is cleaner to settle once #110 has framed it.

## What I could not determine

- **No live probe was performed.** Every claim about the response body is read
  from gemini-cli's own type declarations and its two consumers, corroborated by
  openusage's independent implementation of the same RPC. High confidence on
  shape; the actual bytes on the wire are unverified. Verify before shipping.
- **Whether Code Assist's daily window is a calendar day (and in which timezone)
  or a rolling 24 hours.** No primary source states it. Mitigated by design:
  `resetTime` comes from the server, so the card never needs to know.
- **The unit of `remainingAmount`, and what `tokenType` means.** `tokenType` is
  declared in `BucketInfo` but referenced nowhere in gemini-cli — every other
  `tokenType` hit in the repo is an unrelated OAuth field. The prose docs say
  "model requests," so requests is the likely unit (**medium** confidence), and
  it is irrelevant while only `remainingFraction` is stored.
- **Whether `retrieveUserQuotaSummary` answers for Code Assist accounts**, and
  what windows it returns. One probe settles it.
- **Whether Antigravity and Gemini CLI on the same Google account draw from the
  same buckets.** They hit the same RPC on the same host with different
  credentials and different documented windows (Antigravity: rolling 5-hour plus
  weekly; Gemini CLI: per-day). If the buckets overlap, two cards would show one
  allowance twice. Needs a probe with both signed in — and it is a good reason to
  land #110 and #112 in that order.
- **The per-minute cap number**, for any tier. Documented to exist, never
  quantified. If the endpoint returns a per-minute bucket, its `window_key`
  should carry it; unverified.
- **Whether the individual-tier caps hold in practice.** There is no observation
  base here comparable to the 22,240-observation Codex corpus, and no reachable
  history — the CLI keeps quota only in memory, so nothing accumulates. The first
  weeks of `limit_readings` rows will be the evidence base.

## Sources

- `google-gemini/gemini-cli@659c7aacd96f6632f19e2fac0796db83a2f97e6b` (main, 2026-08-11):
  `packages/core/src/code_assist/server.ts:73-74,367-374,415-443,524-534`;
  `packages/core/src/code_assist/types.ts:66-87,93-104,150-156,250-265`;
  `packages/core/src/code_assist/codeAssist.ts:15-62`;
  `packages/core/src/code_assist/oauth2.ts:799-810`;
  `packages/core/src/config/config.ts:853-858,1852-1854,2035-2144,2306-2380`;
  `packages/core/src/config/storage.ts:22`;
  `packages/core/src/core/loggingContentGenerator.ts:422,573`;
  `packages/core/src/utils/googleQuotaErrors.ts:205-330`;
  `packages/core/src/billing/billing.ts`;
  `packages/cli/src/ui/components/QuotaDisplay.tsx:33-62`;
  `packages/cli/src/ui/components/ModelQuotaDisplay.tsx:108-219`;
  `packages/cli/src/ui/commands/statsCommand.ts:64-80`;
  `packages/cli/src/ui/commands/modelCommand.ts:57`;
  `docs/resources/quota-and-pricing.md`
- `google-gemini/gemini-cli@69188c8538af` — "Add usage limit remaining in /stats (#13843)", 2025-11-27 (GitHub blame of `types.ts:249-265`)
- https://docs.cloud.google.com/gemini/docs/quotas (301 target of `developers.google.com/gemini-code-assist/resources/quotas`)
- https://ai.google.dev/gemini-api/docs/rate-limits
- `robinebers/openusage` (HEAD, 2026-08-12): `Sources/OpenUsage/Providers/Antigravity/AntigravityUsageClient.swift:24-31`; `.../AntigravityAuthStore.swift:5,16,19`; `docs/providers/antigravity.md`; provider directory listing
- `junhoyeo/tokscale` (HEAD): repository tree — `crates/tokscale-core/src/sessions/gemini.rs`, no limits provider
- This machine: 48 session artifacts under `~/.gemini/tmp/*/chats/`, plus `~/.gemini/tmp/*/logs.json`, `~/.gemini/oauth_creds.json` (field names only). Corpus counts: 200 `user`, 1,559 `gemini` (1,558 with a `tokens` block), 59 `info`, 9 `error` messages.
- This repo: `src-tauri/src/adapters/gemini.rs:135,165-194`; `src-tauri/src/queries.rs:189,628`; `docs/adr/0019-live-limits-are-fetched-by-a-companion-never-the-app.md`; issue #103 (map), #110, #112
