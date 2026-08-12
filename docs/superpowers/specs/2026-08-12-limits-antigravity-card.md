# Limits: the Antigravity Card (v1 addendum)

The destination of wayfinder map
[#127](https://github.com/BrianWong05/TokenLedger/issues/127). This is an
**addendum** to the
[Limits Page v1 design](2026-08-12-limits-page-v1-design.md): everything that
document establishes — card anatomy, states, freshness, Left/Used framing,
SCHEMA_V14, fetch policy, the Companion pattern — applies unchanged. Only the
Antigravity deltas are specified here. Deep detail lives in
[the Antigravity research](https://github.com/BrianWong05/TokenLedger/issues/110)
(findings on `research/antigravity-live-limits`),
[ADR-0020](../../adr/0020-a-companion-may-exchange-a-google-refresh-token.md),
and [the consent resolution](https://github.com/BrianWong05/TokenLedger/issues/129).

## Scope

One new live card: **Antigravity** (`capabilities.limits: "live"`). Ruled out
on the map: the Gemini CLI card (no gaugeable plan window — API-key/Vertex
quota is per-project billing; joins the BYO-key no-card sources) and with it
the shared-allowance probe and any merged-card shape. The Grok card is
separate, plain build issue
[#126](https://github.com/BrianWong05/TokenLedger/issues/126).

## The card

Four bars — **two pools × two windows** — structurally identical to Claude's
card, no new presentation primitives:

```
[icon] Antigravity                                   [Pro]
       checked 2m ago
  Gemini · Session        ▓▓▓▓▓▓░░░░  58%   resets in 2h 14m
  Gemini · Weekly         ▓▓▓░░░░░░░  31%   resets in 4d 2h
  Other models · Session  ▓░░░░░░░░░  12%   resets in 2h 14m
  Other models · Weekly   ▓▓░░░░░░░░  18%   resets in 4d 2h
```

- The pool is a genuine second axis (unlike every other source), so it is part
  of the row label: `gemini` → **"Gemini"**, `3p` → **"Other models"** /
  「其他模型」 (its real meaning is *non-Gemini* — Claude, GPT-OSS — so
  labelling it "Claude" would rot; per the research's recommendation). An
  unknown pool prefix renders raw, mirroring the `seven_day_zephyr` rule.
- Plan pill: `UserTier.name` from `loadCodeAssist` (`paidTier.name` preferred
  over `currentTier.name`), **normalised to one word** the way openusage does —
  strip the "Google AI " prefix / pull the tier word ("Google AI Pro" → "Pro").
  Do not use the Windsurf-inherited `planStatus.planInfo.planName` (reads "Pro"
  for every paid tier).
- Everything else — scarcity tones, time tick, Left/Used, freshness line,
  signed-out/error states — is v1 machinery, untouched.

## Acquisition: the `antigravity-limits` Companion

A new bin on the `claude-limits.rs` template (`src-tauri/src/bin/`), wired the
same way: `externalBin` entry, scoped `shell:allow-execute` capability,
`checkLive('antigravity')` routing, ≥60s floor via `lastCheckKey('antigravity')`,
behind the same opt-in gate.

**1. Credential** — macOS Keychain item `service=gemini account=antigravity`,
read via `/usr/bin/security find-generic-password -s gemini -a antigravity -w`
(never in-process `SecItem`; here `-a` **is** used — the account is the literal
string `antigravity`, not a user name, and the generic service name `gemini`
needs the disambiguation; #105's drop-`-a` rule was specific to Claude's item).
Exit 44 → signed-out card. The value is a `go-keyring-base64:`-prefixed base64
wrapper around JSON:

```
token.access_token / token.token_type / token.refresh_token
token.expiry   (RFC3339)
auth_method
```

Do **not** read `~/.gemini/oauth_creds.json` — that is gemini-cli's document,
a different OAuth client against a different quota.

**2. Token** — if `token.expiry` is in the future, use the cached
`access_token` as-is. Otherwise exchange per ADR-0020, exactly within its
bounds: `POST https://oauth2.googleapis.com/token`
(`grant_type=refresh_token`, hardcoded client pair) and nowhere else; the
minted token lives only in process memory (no cache file, no Keychain write);
the Keychain item is never written. The exchange is verified non-rotating —
the response carries no replacement refresh token. Client id (verified
verbatim in Antigravity's `language_server` binary):
`1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com`.
The secret ships in the same binary (Google installed-app pairs are public
identifiers, not keys — ADR-0020 bound 3); take it from openusage's
`AntigravityUsageClient.swift` and verify against the local binary at build
time. A failed exchange (revoked/expired grant) renders the signed-out card
pointing at Antigravity, per ADR-0019 bound 4.

**3. Fetch** — production host only, `cloudcode-pa.googleapis.com` (never the
`daily-` canary — research §6 documents openusage's ordering as a latent bug):

1. `POST /v1internal:loadCodeAssist` → `cloudaicompanionProject` + tier names.
2. `POST /v1internal:retrieveUserQuotaSummary` with
   `{"project": "<cloudaicompanionProject>"}` (the descriptor marks `project`
   REQUIRED; openusage omits it and may be silently falling through — pass it).

Only the summary endpoint. **No legacy `retrieveUserQuota` fallback**: its
shape cannot distinguish absent-from-exhausted, and a card that invents
"100% used" is worse than "no data". If the account predates the summary RPC,
the honest card is the error state.

**4. Artifact** — same `LimitsExport` shape and rename-write conventions,
`source: "antigravity"`, `via: "live"`.

## Bucket → Limit Reading mapping

Match **exact `bucket_id` strings only** — never infer pool or window from
`display_name` or the `window` string (both server-side vocabulary):

| `bucket_id` | `window_key` | `window_minutes` |
|---|---|---|
| `gemini-5h` | `gemini:w300` | 300 |
| `gemini-weekly` | `gemini:w10080` | 10080 |
| `3p-5h` | `3p:w300` | 300 |
| `3p-weekly` | `3p:w10080` | 10080 |

- `used_pct = round((1 − remaining_fraction) × 100)` — integer, preserving the
  PK's ≤101-rows-per-epoch bound. `resets_at` from `reset_time` → unix seconds.
- **No row, no bar** (all four collapse to v1's "an absent Capability is
  unknown, never zero"):
  - missing `reset_time` — a rolling window that has not started; fabricating
    an anchor would corrupt the `max(resets_at)` epoch derivation;
  - a bucket carrying `remaining_amount` instead of `remaining_fraction` — a
    count with no denominator is a figure, not a bar (v2 question); log it to
    stderr, since it is the one wire change that would silently empty the card
    (openusage reads it as "No data" with no hint);
  - `disabled: true` — the pool exists but is off for this account;
  - an unrecognised `bucket_id` — skip and log to stderr (a future
    `gemini-image-5h` must not silently join the Gemini pool).

**Storage**: no schema change — SCHEMA_V14's `window_key` is TEXT and the
content-keyed PK, display derivation (including the reset-jitter band), and
`INSERT OR IGNORE` all apply per `(source, window_key)` unchanged.

## Frontend deltas

- `source-catalog.json`: `antigravity` gains `capabilities.limits: "live"`.
- `limits.derive.ts` window labels: split `window_key` on `:`; a pool prefix
  maps `gemini` → "Gemini", `3p` → "Other models"/「其他模型」, unknown → raw;
  the remainder classifies through the existing rules (`w300` → Session,
  `w10080` → Weekly).
- **Consent** ([the consent resolution](https://github.com/BrianWong05/TokenLedger/issues/129),
  verbatim): `LIVE_ENABLED_KEY` bumps to `tl.limits.liveEnabled.3` (old key
  orphaned, no migration — every user re-consents once) and `optinBody` /
  `optinBounds` are replaced with the resolution's EN + 繁體中文 strings.
  `optinTitle` and `optinButton` unchanged.

## The privacy rewrite, widened

README's live-limits paragraph (from v1) names only Claude. Replace its
feature sentence with:

> One optional feature reaches further and asks first: enabling **live limit
> checks** on the Limits tab runs a separate companion process that presents
> your sign-ins to their own vendors — Claude Code's to `api.anthropic.com`,
> Antigravity's to Google — read-only, only when you open that page or press
> Refresh, never on a timer. Google sign-ins work on hourly passes, so the
> companion first gets a fresh pass from Google the same way Antigravity
> itself does; the pass is used once and never stored, and your saved sign-in
> is never altered
> ([ADR-0019](docs/adr/0019-live-limits-are-fetched-by-a-companion-never-the-app.md),
> [ADR-0020](docs/adr/0020-a-companion-may-exchange-a-google-refresh-token.md)).

The first-run footnote from v1 is already vendor-neutral — no change.

## Test obligations (repo style)

**Rust, fixture-driven:**
- Envelope decode: a `go-keyring-base64:` fixture yields the token triple;
  a malformed envelope is a failure (error card), never signed-out.
- Expiry gate: future `token.expiry` → no exchange; past → exchange path.
- Mapping: a four-bucket summary fixture yields four Readings with
  pool-prefixed keys; unknown `bucket_id`, `disabled`, `remaining_amount`, and
  missing `reset_time` buckets each yield none (and the amount case writes the
  stderr note); `used_pct` rounds to integer.
- Artifact: an antigravity export parses into `via='live'` rows; unrecognised
  `schema` warns (ADR-0015/0018 rule).

**TypeScript:**
- `gemini:w300` renders "Gemini · Session" (and zh-Hant); unknown pool prefix
  renders raw; the four-bar card renders from stored readings.
- Consent bump: a stored `liveEnabled.2` shows the opt-in again; enabling
  writes `.3`; the new copy keys exist in both languages.

## Suggested build order

1. Catalog + derive split + consent bump/copy (page renders a disabled
   Antigravity card end-to-end, no network).
2. `antigravity-limits` bin: envelope → token → `loadCodeAssist` →
   `retrieveUserQuotaSummary` → artifact, with the mapping fixtures.
3. Wire `checkLive('antigravity')` + verify the shell capability by clicking
   **Enable** in the running app (the v1 wiring shipped unverified — cover
   both this card and Claude's while there).
4. README privacy paragraph.
