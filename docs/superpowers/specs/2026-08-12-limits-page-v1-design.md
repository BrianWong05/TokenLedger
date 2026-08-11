# Limits Page v1 Design

The destination of wayfinder map
[#103](https://github.com/BrianWong05/TokenLedger/issues/103). Every decision
below was settled on that map's tickets; this document assembles them so a
build session works from one place without rediscovering anything. Where a
detail here seems surprising, the ticket linked for it holds the full
reasoning.

## Goal

A fourth sidebar tab — **Limits** — showing one live card per Source with a
vendor quota: how much of each rolling window is used, when it resets, and
whether the pace is comfortable, for **Claude and Codex** in v1. Every
reading is captured to a durable series from day one; the page renders only
"now".

## Vocabulary

`CONTEXT.md` is normative: a **Limit** is a rolling-window vendor quota (a
window with a ceiling that fills and resets — never "rate limit", "quota",
"allowance"); a **Limit Reading** is one observation of one Limit, holds no
tokens, and lives beside the Ledger, never in it; the tab is **Limits** /
**限額**. A Companion fetches live Limits, never the app —
[ADR-0019](../../adr/0019-live-limits-are-fetched-by-a-companion-never-the-app.md)
binds the whole feature and its four bounds are restated in §Acquisition.

## Scope

v1 is Claude (live fetch via Companion) + Codex (passive, from logs). Out of
scope, per the map: BYO-key/local Sources (no vendor window exists), spend
caps, context windows, locally-derived utilization, credential writing or
refreshing, the Menu Bar Extra gauge, history rendering, notifications, and
Codex credits (entire local corpus is `has_credits:false`; a balance is not a
window — it gets its own shape if it ever shows up non-zero, per
[#108](https://github.com/BrianWong05/TokenLedger/issues/108)).

## The page in the shell

A fourth tab after Pricing, lazy-loaded exactly like Pricing
(`lazy(() => import('./limits/LimitsPage'))`), nav icon lucide *gauge*, label
`Limits` / `限額`. It ignores the Overview's date window and Source selection
entirely — it is *now*, not a range. Toolbar: page title, muted note
("Vendor windows, not your spend · checked when you open this page or press
Refresh"), a **Left/Used** segmented toggle, and **Refresh**.

Cards render in a responsive grid (`repeat(auto-fill, minmax(340px, 1fr))`),
ordered by `source-catalog.json` order. No reordering
([#106](https://github.com/BrianWong05/TokenLedger/issues/106)).

## Card anatomy (settled by prototype — Variant B, "stat tiles")

Reference implementation: branch
[`prototype/limits-page`](https://github.com/BrianWong05/TokenLedger/tree/prototype/limits-page),
`src/limits/` (commit a78fb75). Rewrite it properly; do not promote prototype
code.

**Header**: source mark (18px), source label, plan pill (`rateLimitTier` for
Claude, `plan_type` for Codex; accent-subtle background), freshness
right-aligned (§Freshness).

**One row per window**: a large tabular numeral of the framed percentage
(26px, right-aligned, min-width 62px) with a small `%`; then label line
(bold window label, resets-in right-aligned, "Resets in 1d 6h" —
largest-two-units formatting); then a thin 6px bar.

**The bar**: fill anchored left showing the framed figure. Fill tone follows
**scarcity (% left) alone**, whatever the framing: >50 `--success`, 21–50
`--warning`, ≤20 `--danger`; the numeral tints to match. A 0%-left window
replaces its figures with "used up · resets in {t}".

**The tick is time, not a forecast**
([#107](https://github.com/BrianWong05/TokenLedger/issues/107) — there is no
forecast anywhere on this page): a neutral 2px marker (`--text-secondary`,
never tinted) at `resetsIn ÷ duration` under Left framing (4 days left of a
7-day window → 4/7 across), at the elapsed fraction under Used. Fill vs tick
reads pace at a glance. Hover title: "now — {t} until reset" /
「現在 — 距重置還有 {t}」. A window with no scheduled reset (e.g. a Claude
session window with no active session) renders no tick.

**Left/Used**: a page-level toggle (not a Settings preference), default Left,
last choice persisted (one stored key). Flips numeral, fill, and tick; colors
never flip.

**Window labels**: `five_hour` → "Session" / 「時段」; `seven_day` →
"Weekly" / 「每週」; per-model windows are **discovered from the response's
`seven_day_*` keys**, never a fixed list — label is the capitalized key tail
("Opus", and an unseen `seven_day_zephyr` renders as "Zephyr") with a muted
"Weekly" sub-label. Codex windows label by classified duration (§Ingest). A
window the vendor does not report renders no bar — an absent Capability is
unknown, never zero.

## Card states

Four resolved visuals (prototype has all of them staged):

1. **Live** — bars plus freshness line.
2. **Not signed in** (map decision 9: a missing card reads as "unsupported",
   which is a lie) — card at reduced opacity, grayscale mark, no plan pill,
   "Not signed in" + "Sign in with the `claude` CLI, then check again." +
   ghost **Check again** button. Trigger: Keychain exit 44 / no credential
   document, or an HTTP 401/403 (ADR-0019: report and point at the Source's
   own CLI; never attempt repair).
3. **Error** — distinct from signed-out
   ([#105](https://github.com/BrianWong05/TokenLedger/issues/105): exit 44
   means not signed in; **any other non-zero is a failure and must not be
   conflated**): danger-tinted title "Couldn't check", the raw detail line in
   monospace, **Retry**. Trigger: unexpected `security` exit, network
   unreachable, malformed response.
4. **Nothing recorded** (Codex analogue of state 2) — a `logs` Source with no
   readings at all: same muted treatment, "No Codex activity recorded yet".

**The opt-in empty state** (map decision 8) replaces the whole card area on
first visit while live checks are disabled: title "See how much of your plan
is left", body disclosing exactly what enabling does (reads the sign-in
Claude Code keeps in your Keychain, asks Anthropic read-only how much of your
quota is used), bounds line ("Only when you open this page or press Refresh —
never on a timer. Your sign-in is never changed, refreshed, or sent anywhere
else."), one accent button **Enable live limit checks**. Enabling persists a
single boolean; no credential is read before it. Full EN/zh-Hant copy is in
the prototype's `prototype.strings.ts`. The empty state gates only `live`
acquisition — Codex readings flow from ordinary scans regardless (decision 6),
so after enabling, the Codex card may already have history.

## Freshness

Per card: `via: 'live'` shows "checked just now / {t} ago" (age of the
fetch); `via: 'logs'` shows "from your logs · last request {t} ago", turning
amber ("no requests in {t} — figures are that old") past 24h. Staleness of
the *figures* is judged per bar against that bar's own `resets_at`
([#104](https://github.com/BrianWong05/TokenLedger/issues/104)): a reading
whose `resets_at` is still in the future stands however old it is; a reading
from an **expired** epoch renders the bar full/unused (nothing was used
since — no request could have gone unlogged) with a tick derived from the
next reset when the window is periodic (`resets_at + n·duration`), and no
tick for a session-anchored window ([#107](https://github.com/BrianWong05/TokenLedger/issues/107)).

## Acquisition

### Codex — passive, from logs (map decision 10)

The scan already reads `token_count` events in
`~/.codex/sessions/**/rollout-*.jsonl`; the same payload carries
`rate_limits`. The adapter additionally emits Limit Readings. **Ingest rules
(normative, from [#104](https://github.com/BrianWong05/TokenLedger/issues/104)):**

- Filter `limit_id == "codex"` — a `"premium"` record is the fingerprint of a
  refused 429 carrying an empty snapshot.
- Classify windows by `window_minutes` against the canonical set
  300/1440/10080/43200/525600 within ±5% (Codex's own labeller does this);
  an unrecognised duration keeps its raw minutes rather than being treated as
  corrupt. `window_key` is `w{canonical minutes}`. Never key off the
  `primary`/`secondary` slot — the slot carries no window meaning.
- A null slot is a window that does not exist — no Reading, no bar.
- Skip forked/subagent replay files for Readings
  (`forked_from_id` / `parent_thread_id` / `thread_source == "subagent"`) —
  replays rewrite envelope timestamps; the content-keyed PK absorbs most, the
  guard keeps `observed_at` honest.
- `observed_at` is the envelope timestamp — never the filename date (123 of
  212 local files have a name-date differing from their first observation's
  UTC date).

### Claude — a Companion, never the app (ADR-0019)

Pressing **Refresh** (or opening the page) with live checks enabled runs a
Companion sidecar — the `antigravity-export` plumbing is the template
(`externalBin` entry, scoped `shell:allow-execute` capability, an
`export_antigravity`-style command handing stdout to the frontend). The
Companion:

1. **Acquires the token** per
   [#105](https://github.com/BrianWong05/TokenLedger/issues/105): on macOS,
   `security find-generic-password -s <service> -w` — **no `-a`** (real
   accounts contain `@` and break `-a` lookups), **never** in-process
   `SecItem`/keyring (that path re-prompts per release on ad-hoc-signed
   builds; the `security` route never prompts). Probe the known service names
   (`Claude Code-credentials` and hash-suffixed variants — the derivation has
   changed across versions). Elsewhere, read `~/.claude/.credentials.json`.
   Exit 44 / absent document → signed-out. **Read-only**: never write or
   refresh the credential, never spend the refresh token (tokscale rewrote
   the file and broke Claude Code's own sign-in).
2. **Fetches** `GET https://api.anthropic.com/api/oauth/usage` with
   `Authorization: Bearer <claudeAiOauth.accessToken>` and
   `anthropic-beta: oauth-2025-04-20`. Response: named windows
   (`five_hour`, `seven_day`, `seven_day_<model>`…) each with `utilization`
   and `resets_at` (ISO-8601). The credential document also carries
   `subscriptionType` and `rateLimitTier` — the plan label.
3. **Writes an Export Artifact** of Limit Readings (JSON, `schema` field,
   rename-write — ADR-0018 conventions) and prints the readings on stdout;
   the page renders stdout immediately, the next scan ingests the artifact
   with `via='live'`. 401/403 → signed-out card; any other failure → error
   card with the Companion's stderr line.

**Fetch policy** (map decision 5): page open + manual refresh only, ≥60s
between calls per provider, never on the 30s scan timer, never in the
background. For Codex, Refresh just triggers an ordinary scan.

## Storage ([#108](https://github.com/BrianWong05/TokenLedger/issues/108))

```sql
-- SCHEMA_V14
CREATE TABLE IF NOT EXISTS limit_readings (
  source         TEXT NOT NULL,
  window_key     TEXT NOT NULL,
  window_minutes INTEGER,
  used_pct       REAL NOT NULL,
  resets_at      INTEGER NOT NULL,
  observed_at    INTEGER NOT NULL,
  via            TEXT NOT NULL,
  plan           TEXT,
  PRIMARY KEY (source, window_key, resets_at, used_pct)
);
DELETE FROM scanned_files;
PRAGMA user_version = 14;
```

`INSERT OR IGNORE` — the PK is the reading's **content**, so the table holds
the append-only fill-curve at integer resolution (≤101 rows per window per
epoch; the 22,240-observation local corpus collapses to a few hundred rows)
and re-scans, per-request repeats, and fork replays are all absorbed.
`used_pct` stores the vendor's own figure unconverted. Claude's ISO
`resets_at` converts to unix seconds at ingest. The `DELETE FROM
scanned_files` is the established backfill pattern (V2–V5, V12): the next
scan re-parses every log once and Codex history back-fills retroactively —
idempotent, so the dev-DB hazard cannot consume it.

**Display derivation**: per (source, window_key), the newest epoch is
`max(resets_at)`; within it the current state is `max(used_pct)`
(`used_percent` is effectively monotonic within an epoch). "Newest valid
Reading" in `CONTEXT.md` terms.

## Catalog

`source-catalog.json` gains `capabilities.limits: "logs" | "live"` (absent =
no card): `claude: "live"`, `codex: "logs"`. Rust `Capabilities` gains
`limits: Option<String>` (serde default `None`); the TS type mirrors it. The
enum drives card existence, the opt-in gate (only `live` sits behind it), and
the freshness copy. Vendor URLs stay in the Companion, never in the data
file.

## i18n and theme

Fold the prototype's `prototype.strings.ts` into `src/lib/strings/limits.ts`
and register it in `lib/i18n.tsx`'s dictionaries — keys are drafted for both
languages, including the opt-in disclosure. DS tokens only; light/dark comes
free.

## The privacy rewrite

README's privacy section currently promises: *"Nothing about you leaves the
machine. The app makes exactly three outbound requests, all of them fetches
of public data…"* — this feature breaks that sentence, and the fix is to
state the trade, not bury it. Replacement paragraph:

> Nothing about you leaves the machine by default. The app itself makes
> exactly three outbound requests, all fetches of public data: LiteLLM's
> price list and OpenRouter's model list for pricing, and the GitHub release
> manifest for updates. One optional feature reaches further and asks first:
> enabling **live limit checks** on the Limits tab runs a separate companion
> process that presents your Claude Code sign-in to `api.anthropic.com` —
> read-only, only when you open that page or press Refresh, never on a
> timer — to ask how much of your quota is used. Until you press that button,
> no credential is read and no authenticated request exists. The companion
> never writes or refreshes your sign-in
> ([ADR-0019](docs/adr/0019-live-limits-are-fetched-by-a-companion-never-the-app.md)).

First-run dialog: the body stays; the footnote gains the same honesty in one
line —

> EN: "Scans are local file reads — nothing is uploaded. Live limit checks
> are separate, optional, and asked about on the Limits tab before anything
> runs."
> zh-Hant: 「掃描只是本機檔案讀取 — 不會上傳任何東西。即時限額查詢是另一回事：
> 可選，且會在「限額」分頁先徵求同意才執行。」

## Test obligations (repo style)

**Rust, fixture-driven (adapter tests beside the code):**
- Codex `rate_limits` capture: a fixture Session with the real nine-field
  block yields Readings; `limit_id:"premium"` lines yield none; a null
  `secondary` yields no Reading; window classification maps 300→`w300`,
  10080→`w10080`, 10081 (within ±5%)→`w10080`, and an unrecognised 4321
  keeps `w4321`.
- Fork guard: a `thread_source:"subagent"` fixture contributes no Readings.
- Dedup: scanning the same fixture twice leaves the row count unchanged;
  two requests at the same `used_percent` insert one row.
- Migration: V13→V14 creates the table, clears `scanned_files`, stamps
  `user_version 14` (pattern of the existing migration tests in `db.rs`).
- Export Artifact: a Claude limits export parses into `via='live'` rows; an
  unrecognised `schema` warns instead of parsing (ADR-0015/0018 rule).

**TypeScript (component tests, jsdom, existing conventions):**
- Card states: live / not-signed-in / error / nothing-recorded, opt-in empty
  state gating and its enable flow.
- Tick math: Left 4d-of-7 → 4/7; Used flips; no tick without a scheduled
  reset; expired-epoch renders full/unused.
- Scarcity tones at 51/50/21/20; Left/Used flips figures but never colors.
- Unknown `seven_day_zephyr` renders as "Zephyr".
- Catalog gating: a source without `capabilities.limits` gets no card.

## Suggested build order

1. SCHEMA_V14 + Codex capture + dedup (tests prove backfill).
2. Catalog enum + tab skeleton + cards from stored Readings (Codex works
   end-to-end with no network).
3. Claude Companion + Export Artifact + opt-in gate.
4. Privacy rewrite + first-run footnote.
