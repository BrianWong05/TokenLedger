# Research: does OpenCode have a quota worth gauging?

Resolves [#113](https://github.com/BrianWong05/TokenLedger/issues/113), on the
[Limits map](https://github.com/BrianWong05/TokenLedger/issues/103). Investigated 2026-08-12.

## Verdict

**Out of scope for v1 — no card.** But not for the reason the ticket proposed.

OpenCode **does** sell a subscription with its own vendor quota: **OpenCode Go**, $10/month, with
three rolling windows denominated in dollars — **$12 per 5 hours, $30 per week, $60 per month**. The
ticket's BYO-key premise is wrong, and so is the map's "OpenCode = `opencode.ai/auth`" line.

It fails on a sharper ground: **there is no Limit Reading to be had.** OpenCode publishes no usage
endpoint (verified: the route 404s while its sibling gateway routes 401), and persists no usage or
limit state locally beyond the moment a request is *refused*. A card would therefore have to be
**synthesized** — a hardcoded published cap as denominator, divided into billed spend re-summed from
this machine's logs. That is precisely the construction the map already rejected twice: decision 2
("no hand-maintained table of published vendor limits — it would rot, and a stale limit is worse than
no limit") and the Out-of-scope bullet on deriving Claude utilization locally against a hardcoded
ceiling.

Recommended catalog state: OpenCode gets **no `capabilities.limits` value** (neither `"logs"` nor
`"live"`). Per map decision 9, "a tool with no credentials still gets a card" applies to sources that
*have* a limit contract; OpenCode has a quota but no readable reading, which is the different case.

## The three reasons, in order of weight

**1. The numerator is structurally incomplete, and it fails *reassuring*.** Any local figure is spend
observed on *this* machine. Go is one-subscriber-per-workspace, but nothing stops that subscriber
working from a laptop and a desktop. An undercount biases the bar toward "you have room" while the
account is already refusing requests. For an app whose credibility rests on *Where the numbers bend*,
a gauge whose failure mode is silent over-optimism is the worst available failure mode.
openusage — which ships this exact card — concedes the point in its own user documentation: "the
local figure can be lower than your true account usage, so treat the caps as a guide rather than the
last word."

**2. The denominator rots, and OpenCode says so in writing.** From `docs/go.mdx`: *"Usage limits may
change as we learn from early usage and feedback."* And separately: *"The list of models may change
as we test and add new ones."* A stale cap silently mis-scales every bar on the card. This is
decision 2's prohibition almost verbatim. Note the honest difference from the Claude case: OpenCode
*does* publish its ceiling, so "the number is unknowable" does not apply here — only the rot does.

**3. It is not the money this app counts.** [ADR-0002](../adr/0002-cost-is-estimated-list-price-value-not-billed-spend.md)
settles that ledger cost is estimated list-price *value*, not billed spend. Go's caps are denominated
in **vendor-billed dollars**. The OpenCode adapter
(`src-tauri/src/adapters/opencode.rs`) reads only `tokens.{input,output,reasoning,cache.{read,write}}`
and `modelID` — it reads neither `$.cost` nor `$.providerID`. Feeding this card means teaching the
ledger a second, incompatible money concept (authoritative billed spend, for two specific
`providerID`s) to serve one card. Not a small change, and it cuts against ADR-0002's grain.

**4. Zen is not a quota at all.** Zen is pay-as-you-go credits with an *optional, user-set* monthly
spending limit. Map decision 1 excludes self-imposed spend caps explicitly. Zen contributes nothing
gaugeable even in principle.

## Evidence

### OpenCode Go is a real subscription with real windows

Primary source: OpenCode's own docs source, `packages/web/src/content/docs/go.mdx`, on
`anomalyco/opencode` @ `36b205370d61b1817943358cf3abf9eaaec9289f` (branch `dev`, 2026-08-11).
Rendered at <https://opencode.ai/docs/go/>.

| Fact | Value |
|---|---|
| Price | $5 first month, then $10/month |
| 5-hour limit | $12 of usage |
| Weekly limit | $30 of usage |
| Monthly limit | $60 of usage |
| Denomination | "Limits are defined in dollar value" — request counts vary by model |
| At the limit | "If you reach the usage limit, you can continue using the free models" — soft gate, not a hard block |
| Overflow | Optional **Use balance**: fall back to the Zen credit balance instead of blocking |
| Where usage is visible | "You can track your current usage in the **console**" — and `packages/web/config.mjs` resolves `console` to `https://opencode.ai/auth` in production |
| Stability | "Usage limits may change as we learn from early usage and feedback" |
| Scope | "Only one member per workspace can subscribe to OpenCode Go" |

Confidence: **high.** Vendor's own documentation source, read from the repository rather than the
rendered page.

The windows map cleanly onto the normalized model, if a reading existed:
`(opencode, "session", 300, …)`, `(opencode, "weekly", 10080, …)`, `(opencode, "monthly", ~43200, …)`.

### There is no usage endpoint (the decisive finding)

`docs/go.mdx`'s **Endpoints** section lists only inference and metadata routes —
`/zen/go/v1/chat/completions`, `/zen/go/v1/responses`, `/zen/go/v1/messages`, `/zen/go/v1/models`. No
usage or limits route appears anywhere in the document.

Confirmed by unauthenticated probe on 2026-08-12. The gateway distinguishes "route exists but needs
auth" from "no such route", which makes this decisive rather than suggestive:

```
POST https://opencode.ai/zen/go/v1/chat/completions   (no auth)  -> 401
POST https://opencode.ai/zen/go/v1/usage              (no auth)  -> 404
GET  https://opencode.ai/zen/go/v1/models                        -> 200
GET  https://opencode.ai/zen/v1/usage                            -> 404
GET  https://opencode.ai/zen/go/v1/limits                        -> 404
```

`https://api.opencode.ai/*` is a red herring: it answers **HTTP 200 with the body `Not Found`** for
every path, including `/` and a random nonexistent route. Any 200 from that host means nothing.

`https://opencode.ai/auth` serves `text/html` — it is the web console, a browser surface, not an API.

No credential was used for any probe.

Confidence: **high** for "no usage route on the Go gateway today." The 401-vs-404 contrast rules out
the usual false negative (a route that hides behind auth). Residual uncertainty: an authenticated
session-cookie API behind the console SPA could exist and was not enumerated — see *Not determined*.

### What OpenCode itself knows about your limit: only at refusal

OpenCode's client has no proactive usage figure. It discovers the limit **reactively**, by parsing a
refused request. From `packages/opencode/src/session/retry.ts` (same SHA), the sole limit-bearing
path:

```ts
if (error.data.responseBody?.includes("GoUsageLimitError")) {
  const body = parseJSON(error.data.responseBody)
  const workspace = str(body?.metadata?.workspace)
  const limitName = str(body?.metadata?.limitName)
  const retryAfter = num(error.data.responseHeaders?.["retry-after"])
  …
  const message = `${limitName ? `${limitName} usage limit` : "Usage limit"} reached. It will reset in ${resetIn}. …`
```

So a Go refusal carries **which** limit tripped (`metadata.limitName`), **which** workspace
(`metadata.workspace`), and **when it resets** (the `retry-after` header). It carries no `used`, no
`limit`, and no percentage. A sibling branch handles `FreeUsageLimitError` the same way, as a Go
upsell.

This is a refusal, not a reading — used_pct is implicitly 100 and knowable only at the instant you
are blocked. ADR-0019's posture already says a refused fetch is not a Reading, and map decision on
#104 reached the same conclusion for Codex's `"premium"` refusal snapshots.

It is also not persisted. `retry.ts`'s `policy()` hands the action to a caller-supplied `set()`, which
feeds the transient `session.status` event consumed by
`packages/app/src/pages/session/usage-exceeded-dialogs.tsx` (blob
`d56fa3d1f48c4a0c3d8528bd103cc8d2a3fc1f62`) purely to show a dialog, rate-limited to once per 24h per
reason. Nothing writes a limit snapshot to the database.

Corroborated on this machine: the only `$.error.name` value persisted across the local database is
`MessageAbortedError` (6 rows). No usage-limit error shape is stored.

Confidence: **high** that nothing usable is persisted locally. Caveat: this machine has never used
Go, so no Go refusal could have been observed here — the persistence conclusion rests on reading
OpenCode's source, not on negative local evidence.

### Credential location and shape

From OpenCode's own source, `packages/tui/src/component/dialog-provider.tsx` (same SHA), the
`/connect` flow writes:

```ts
await sdk.client.auth.set({
  providerID: props.providerID,          // "opencode-go"
  auth: { type: "api", key: value, … },
})
```

Which lands in `auth.json` in OpenCode's data directory as:

```
{ "opencode-go": { "type": "api", "key": "<api key>" } }
```

Path resolution mirrors OpenCode itself: `$OPENCODE_DATA_DIR`, else `$XDG_DATA_HOME/opencode`, else
`~/.local/share/opencode`; the file is `auth.json` (mode 600 on this machine). Note the catalog's
existing `OPENCODE_DB` artifact override is a *different* variable and does not move `auth.json`.

Verified on this machine (field names only): `~/.local/share/opencode/auth.json` holds entries
`google`, `github-copilot`, and `openai`. The OAuth entries carry
`{access, refresh, expires, type}` (plus `accountId` for `openai`). There is **no `opencode-go`
entry** — this machine is a BYO-key OpenCode user with no Go subscription, so the Go card would
render empty here regardless.

Confidence: **high** on the shape (vendor source + independent corroboration from openusage's
reader). **Medium** that `type: "api"` / `key` is stable across versions — it is a generic API-key
path shared by every custom provider, so it is unlikely to churn, but it was not read from a real Go
entry.

### What openusage actually fetches: nothing

This is the ticket's central factual correction. **openusage's OpenCode provider makes no network
call at all.** Read at `robinebers/openusage` @ `487cc8f19a9a28676f6924aafa48dee79ad7a7f6` (branch
`main`, 2026-08-11), all six files under `Sources/OpenUsage/Providers/OpenCode/`:
`OpenCodeProvider.swift`, `OpenCodePaths.swift`, `OpenCodeAuthStore.swift`, `OpenCodeGoWindows.swift`,
`OpenCodeUsageMapper.swift`, `OpenCodeUsageScanner.swift`.

`https://opencode.ai/auth` appears **exactly once** in the whole provider, as UI chrome:

```swift
let provider = Provider(
    id: "opencode", displayName: "OpenCode", icon: .providerMark("opencode"),
    links: [ .init(label: "Dashboard", url: "https://opencode.ai/auth") ]
)
```

It is a clickable "Dashboard" hyperlink in a menu-bar popover. There is no `URLSession`, no
`URLRequest`, no `dataTask` anywhere in the directory (grepped). And the destination is right for what
it is: `packages/web/config.mjs` resolves OpenCode's `console` to exactly `https://opencode.ai/auth`,
which is the page the vendor tells you to check your usage on. It is a human destination, not a
machine one. The map's "Per-provider availability" line — *"OpenCode = `opencode.ai/auth`"* — reads a
dashboard link as an endpoint. **The map should be corrected.**

What openusage actually does, and it works today: it reads OpenCode's local SQLite and computes the
windows itself.

- **Discovery**: every `opencode*.db` in the data directory, unioned — OpenCode partitions by release
  channel (`opencode.db` stable, `opencode-next.db` preview). `.db` suffix excludes `-wal`/`-shm`.
- **Query** (`OpenCodeUsageScanner.dataSQL`): from table `message`, rows with
  `json_extract(data,'$.role') = 'assistant'` and `$.providerID IN ('opencode-go','opencode')` and
  `json_type(data,'$.cost') IN ('integer','real')`, selecting
  `time_created, $.cost, $.tokens.total, $.modelID, $.providerID`.
- **Window math** (`OpenCodeGoWindowMath.compute`): rolling 5h from `now`; UTC week starting Monday;
  month anchored to the day-of-month of the **earliest-ever** local `opencode-go` message (calendar
  month as fallback). Reset instants: session = oldest-in-window + 5h; weekly = week end; monthly =
  anchored cycle end.
- **Denominator** (`OpenCodeUsageMapper`): hardcoded `sessionCap = 12`, `weeklyCap = 30`,
  `monthlyCap = 60`.
- **Gating**: the cap meters render only on a *current* Go signal — an `opencode-go` key in
  `auth.json`, or Go spend inside a window. A Zen-only user sees spend tiles and no meters.
- **Export**: the meters are exported with `estimated: true`.

Its own documentation states the intent for the future, and independently confirms no endpoint exists:
*"When OpenCode ships an official usage API, OpenUsage can switch to authoritative numbers"* and
*"If OpenCode's proposed `/zen/go/v1/usage` API ships, the same Go key becomes the bearer token for
authoritative windows."* Two independent parties therefore agree the route is hypothetical — and our
own 404-vs-401 probe confirms it.

Confidence: **high.** Read in full from source at a pinned SHA.

### A prior-art defect worth recording

openusage's SQL selects `COALESCE(json_extract(data,'$.tokens.total'),0)`. **OpenCode does not write
`$.tokens.total`.** Verified against the real database on this machine (OpenCode 1.3.13, Homebrew),
`~/.local/share/opencode/opencode.db`:

- `json_tree` over assistant-message `data` enumerates `$.tokens.input`, `$.tokens.output`,
  `$.tokens.reasoning`, `$.tokens.cache.read`, `$.tokens.cache.write` — and no `$.tokens.total`.
- `SELECT COUNT(*) … WHERE json_extract(data,'$.tokens.total') IS NOT NULL` returns **0**.

So openusage's OpenCode token column is dead and reports 0 for every row, and its spend tiles show
`$X · 0 tokens`. TokenLedger's own adapter already reads the correct leaf fields and sums them, so we
are not exposed — but this is a concrete reason not to port openusage's SQL, and a reminder that
prior-art convergence is not verification. (Not our bug to file; recorded as evidence.)

Confidence: **high** for OpenCode 1.3.13. Unknown whether an older schema ever carried `total`.

### Local schema, confirmed first-hand

`~/.local/share/opencode/opencode.db` (OpenCode 1.3.13):

```sql
CREATE TABLE `message` (
  `id` text PRIMARY KEY, `session_id` text NOT NULL,
  `time_created` integer NOT NULL, `time_updated` integer NOT NULL,
  `data` text NOT NULL, …
);
```

`time_created` is a real column (epoch ms); the rest is JSON in `data`. Assistant-message fields:
`$.role`, `$.time.{created,completed}`, `$.parentID`, `$.modelID`, `$.providerID`, `$.mode`,
`$.agent`, `$.path.{cwd,root}`, `$.cost`, `$.tokens.{input,output,reasoning,cache.{read,write}}`,
`$.finish`, `$.error.{name,data.message}`. Tables: `message`, `part`, `session`, `project`,
`workspace`, `account`, `account_state`, `control_account`, `event`, `event_sequence`, `permission`,
`session_share`, `todo`, `__drizzle_migrations`.

Aggregated by `providerID` over assistant messages on this machine: `opencode` — 177 messages,
`$0.00`; `google` — 96 messages, `$0.00`. `$.cost` is present and integer-typed on 273 rows, and is
`0` on all of them (free models via the OpenCode gateway, plus a BYO Google key). So `$.cost` is
real and populated, and is genuinely `0` when nothing was billed.

Confidence: **high**, but from a single machine that has never subscribed to Go. **No non-zero
`$.cost` row was ever observed.** The claim that `$.cost` carries authoritative billed dollars for
`opencode-go` traffic rests on openusage's assertion, not on evidence seen here.

## The BYO-key branch — flagged, not pursued

Run OpenCode against your own provider keys (as this machine does: `google`, `github-copilot`,
`openai`) and there is no OpenCode-side quota whatsoever. Usage bills against the upstream vendor,
and gauging it means gauging **that vendor**, per account — a different and much larger scope
question. Per the ticket's instruction: flagged, not pursued.

One wrinkle worth surfacing to the map, because it is an attribution question rather than a card
question: **OpenCode traffic on an Anthropic subscription key already consumes the same window the
Claude card gauges.** The Claude card will therefore silently include OpenCode's usage, attributed to
Claude. That is arguably correct — it *is* the Anthropic quota — but it means "the Claude card" is
really "the Anthropic account card", and no card is per-tool once a subscription is shared across
CLIs. Nothing to decide for v1; recorded so it is not rediscovered later. The same holds for
`github-copilot` (Copilot has its own subscription quota, which openusage tracks as a separate
provider) and for `openai` (Codex, already in v1).

## When to revisit

The contract is already fully scoped, so re-opening this is cheap. The trigger is one command:

```sh
curl -s -o /dev/null -w '%{http_code}\n' -X POST -H 'content-type: application/json' -d '{}' \
  https://opencode.ai/zen/go/v1/usage      # 404 today; anything else means revisit
```

If OpenCode ships any usage route, then: credential is the `opencode-go` → `key` string in
`auth.json`, presented as a Bearer token; boundary is the Companion per
[ADR-0019](../adr/0019-live-limits-are-fetched-by-a-companion-never-the-app.md); catalog gains
`capabilities.limits: "live"`; and the three windows map to `window_minutes` 300 / 10080 / calendar
month. The response shape would have to be re-read at that point — it is unknown.

## Could not determine

- **Whether the web console has an undocumented JSON API.** `opencode.ai/auth` is an HTML SPA and
  the vendor docs say usage is visible there, so an authenticated server-side figure demonstrably
  exists. Its transport was not enumerated — that needs a browser session with a logged-in Go
  account, which this machine does not have. If such an endpoint exists it is a session-cookie
  console API, not a documented Bearer API, and depending on it would be a much weaker contract than
  ADR-0019 contemplates.
- **The real `GoUsageLimitError` response body.** Only the fields `retry.ts` reads are known
  (`metadata.workspace`, `metadata.limitName`, and the `retry-after` header). Whether the body also
  carries numeric `used`/`limit` fields that OpenCode simply ignores is unknown, and would need an
  actual Go account driven past its cap. If it does carry numbers, that changes nothing for v1 — it
  is still a refusal, still only at 100%, and still unpersisted.
- **The exact `limitName` values.** Presumably something like session/weekly/monthly, but never
  observed.
- **Whether `$.cost` for `opencode-go` rows is authoritative billed dollars.** Asserted by
  openusage; no non-zero cost row was available to check.
- **Whether the monthly cycle really anchors to first-use day-of-month.** openusage implements it
  that way and attributes it to a legacy `opencode-go` plugin, but OpenCode's docs say nothing about
  when any window resets. Unverified, and it would matter for a reset countdown.
- **Whether `$.tokens.total` existed in an older schema.** Only 1.3.13 was inspected.

## Sources

Primary — OpenCode (`anomalyco/opencode`, branch `dev` @ `36b205370d61b1817943358cf3abf9eaaec9289f`,
2026-08-11; `sst/opencode` now redirects here):

- `packages/web/src/content/docs/go.mdx` — plan, caps, endpoints, "limits may change"
- `packages/opencode/src/session/retry.ts` — `GoUsageLimitError` / `FreeUsageLimitError` handling
- `packages/app/src/pages/session/usage-exceeded-dialogs.tsx` (blob `d56fa3d…`) — transient dialog only
- `packages/tui/src/component/dialog-provider.tsx` — `auth.set({ type: "api", key })`
- `packages/web/config.mjs` — `console` resolves to `https://opencode.ai/auth`
- <https://opencode.ai/docs/go/>, <https://opencode.ai/docs/zen/>

Primary — openusage (`robinebers/openusage`, branch `main` @
`487cc8f19a9a28676f6924aafa48dee79ad7a7f6`, 2026-08-11):

- `Sources/OpenUsage/Providers/OpenCode/` — all six files
- `docs/providers/opencode.md` — the local-observed caveat, and the "proposed API" admission

Local evidence (this machine, 2026-08-12) — OpenCode 1.3.13 via Homebrew
(`/opt/homebrew/Cellar/opencode/1.3.13`):

- `~/.local/share/opencode/auth.json` — entry keys and field names only
- `~/.local/share/opencode/opencode.db` — schema, JSON field paths, per-`providerID` aggregates
- Unauthenticated HTTP probes of `opencode.ai/zen/*` and `api.opencode.ai`

This repo:

- [ADR-0002](../adr/0002-cost-is-estimated-list-price-value-not-billed-spend.md),
  [ADR-0019](../adr/0019-live-limits-are-fetched-by-a-companion-never-the-app.md)
- `src-tauri/src/adapters/opencode.rs`, `src/source-catalog.json`,
  [docs/source-evidence/opencode.md](../source-evidence/opencode.md)
