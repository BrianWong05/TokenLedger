# Limits page — live rate-limit windows — design

Date: 2026-07-12
Status: approved for planning

## Goal

Wire the Limits page (`src/overview/Limits.tsx`, currently a static design
mockup with hard-coded figures) to real quota data: for each of
TokenLedger's Sources that has a rate-limited plan, read the tool's local
credentials, call the vendor's own usage endpoint, and render rolling limit
windows (used %, reset time) as the mockup's bars. Mechanism follows
mm7894215/TokenTracker (`src/lib/usage-limits.js` et al.), ported to this
app's Rust-backend architecture.

## Scope

Providers in v1 — the Ledger Sources with quotas:

| Provider | Credentials (local) | Endpoint | Windows (bar labels) |
|---|---|---|---|
| Claude | macOS keychain `Claude Code-credentials` (`security find-generic-password -s … -w`) → `claudeAiOauth.accessToken`; Linux/Win: `~/.claude/.credentials.json` | GET `https://api.anthropic.com/api/oauth/usage`, header `anthropic-beta: oauth-2025-04-20` | `5h`, `7d`, `Opus` + any `weekly_scoped` rows labeled by model display name |
| Codex | `~/.codex/auth.json` (honors `$CODEX_HOME`): `tokens.access_token`, account id from field or JWT claim `chatgpt_account_id` | GET `https://chatgpt.com/backend-api/wham/usage`, header `ChatGPT-Account-Id` | `5h`, `7d` — classified by `limit_window_seconds` (18000 / 604800), never by slot position |
| Gemini | `~/.gemini/oauth_creds.json` | POST `https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist` (tier, project) then `…:retrieveUserQuota` | `Pro`, `Flash`, `Lite` — used = (1 − remainingFraction) × 100, lowest-remaining bucket per family |
| Grok Build | `~/.grok/auth.json` → first entry with a `key` | GET `https://cli-chat-proxy.grok.com/v1/billing` | `Month` (used/monthlyLimit), `Extra` (onDemandUsed/onDemandCap) |
| Antigravity | running IDE: `ps` → language_server with Antigravity markers + `--csrf_token` flag; `lsof` → listening ports | probe ports with POST `GetUnleashData`; then `RetrieveUserQuotaSummary` (fallbacks `GetUserStatus`, `GetCommandModelConfigs`) on `/exa.language_server_pb.LanguageServerService/` | `Cl 7d`, `Cl 5h`, `Gm 7d`, `Gm 5h` (the vendor's own quota buckets: `3p-weekly`, `3p-5h`, `gemini-weekly`, `gemini-5h`) |

Hermes gets **no card** — self-hosted, no quota.

## Non-goals (deferred)

- Tools TokenLedger doesn't track (Cursor, Kimi, Kiro, Copilot, ZCode,
  OpenCode Go from the example).
- Codex credit/spark windows beyond `5h`/`7d`; Claude `extra_usage`.
- Notifications/alerts on nearing a limit; menu-bar/widget surfaces.
- Persisting limits history in the Ledger (limits are point-in-time
  reads, not Usage Records).
- The mockup's fake "Connect" buttons (real connecting happens in the
  tool's own terminal login; cards show hint text instead).

## Architecture

**Approach chosen:** full Rust-backend port (Approach A). All credential
reads, subprocess probes, and vendor HTTP stay in `src-tauri`; the webview
only ever receives percentages, labels, and error strings. Rejected:
frontend-TS port via tauri plugins (`http`/`fs`/`shell` — OAuth tokens
would transit the JS context, three new broad capabilities); Node sidecar
(ships a runtime).

New module family `src-tauri/src/limits/`, mirroring `adapters/`:

```
limits/mod.rs         orchestrator + cache + LimitsSnapshot types
limits/claude.rs      keychain/creds-file + oauth/usage + 429 cooldown
limits/codex.rs       auth.json + token refresh + wham/usage
limits/gemini.rs      oauth_creds.json + token refresh + retrieveUserQuota
limits/grok.rs        auth.json + v1/billing
limits/antigravity.rs process/port detect + local quota API
```

One new Tauri command:

```rust
#[tauri::command]
async fn limits(state: State<AppState>, force: bool) -> Result<LimitsSnapshot, String>
```

Providers run in parallel (`std::thread::scope`, 5 threads), each with a
15 s `ureq` timeout, so the command is bounded ≈15 s worst case. `ureq` is
already a dependency (pricing).

## Data contract

Normalization happens in Rust — the UI has exactly one renderer (unlike
the example, which returns provider-specific shapes and normalizes in JS):

```rust
struct LimitsSnapshot { fetched_at_ts: i64, tools: Vec<ToolLimits> }
struct ToolLimits {
    source: String,             // "claude" | "codex" | "gemini" | "grok" | "antigravity"
    configured: bool,           // false → "Not connected" card
    error: Option<String>,      // Some → error card with actionable text
    plan: Option<String>,       // e.g. "Pro", "Plus" → card title "Claude Pro"
    windows: Vec<LimitWindow>,  // the bars
    stale: bool,                // served from disk cache (render "cached · Xh" badge)
    cached_at_ts: Option<i64>,  // epoch seconds of the data
}
struct LimitWindow { label: String, used_percent: f64, resets_at_ts: Option<i64> }
```

All timestamps crossing IPC are epoch seconds (matching the repo's
`Filters.startTs` convention); vendor ISO strings are parsed in Rust.

Plan labels: Claude `subscriptionType` from the keychain payload; Codex
JWT `chatgpt_plan_type`; Gemini tier from `loadCodeAssist`
(`standard-tier` → "Paid"); Antigravity `account_plan` when the quota
summary carries one; Grok none.

## Caching & failure policy

One disk cache file `limits-cache.json` in the app data dir (next to the
DB), holding per-provider last-good `ToolLimits` + `cached_at`, plus
Claude's `retry_at` cooldown. (The example uses three files; one is
enough.)

- **In-memory**: whole snapshot cached 2 min in `AppState`
  (`Mutex<Option<(Instant, LimitsSnapshot)>>`). `force: true` (the ⟳
  button) bypasses this TTL only.
- **Claude fresh window**: a success within 10 min is served from disk
  without calling out — refresh-spam cannot burn OAuth-usage requests.
- **Claude 429**: record `retry_at` from `Retry-After` (default 5 min,
  cap 60 min); during cooldown never call out — serve cache or an
  accurate "retry in ~Nm" error. `force` does NOT bypass the cooldown.
- **Stale fallback**: any provider whose live fetch fails serves its
  last-good cache with `stale: true` (max age 7 days); Antigravity does
  the same when the IDE isn't running (with an explanatory note when no
  cache exists).
- **Codex 401/403/404** from wham = "no usage data for this auth state"
  → neutral empty windows, not an error card. UI rule: a card with
  `configured: true`, no error, and zero windows renders a muted
  "No usage data" line where the bars would be.
- **Codex token refresh**: if `last_refresh` > 8 days and a refresh token
  exists, POST `https://auth.openai.com/oauth/token` and persist back to
  `auth.json` before fetching (example's issue #52). Refresh-token-dead →
  error card telling the user to re-run `codex`.
- **Gemini token refresh**: if `expiry_date` passed, POST
  `https://oauth2.googleapis.com/token` with the OAuth client id/secret
  regex-scraped from the installed gemini-cli `oauth2.js` (example's
  candidate path list), falling back to the public Gemini CLI client
  constants; persist the refreshed token back to `oauth_creds.json`.
- A provider never panics the snapshot: every failure becomes
  `{configured: true, error}` or a stale fallback. One dead vendor cannot
  blank the page.
- `configured: false` only on genuine absence (no creds and no install
  evidence). Gemini: a bare `~/.gemini/settings.json` is NOT evidence
  (Antigravity also writes under `~/.gemini`).

## Frontend

- `src/types.ts`: `LimitsSnapshot`, `ToolLimits`, `LimitWindow`.
- `src/api.ts`: `fetchLimits(force = false)` → `invoke('limits', { force })`.
- `src/overview/Limits.tsx`: drop the `RAW` mock and design-time
  connect/refresh simulation; render `ToolLimits[]` through the existing
  card states — bars / not-connected / error / "cached · Xh" badge (now
  driven by `stale` + `cached_at`). Keep Used/Left toggle and
  All/Connected filter client-side. Fetch on mount + every 5 min while
  mounted; ⟳ calls `fetchLimits(true)`. Footer shows real
  "Last checked X ago · auto-refreshes every 5 min".
- Tool identity (icon, color, name) keyed by `source`, reusing the
  mockup's icons for the 5 real providers.
- **Real nav**: `main.tsx` renders a small shell owning `nav` state and
  switching between `Overview8b` and `Limits`; both components' top bars
  become controlled (`nav`, `onNav` props). Other nav items stay
  decorative.

## Security notes

- Secrets (OAuth tokens, keychain payloads) never cross the Tauri IPC
  boundary; they live and die inside the Rust command.
- Reading the Claude keychain item triggers a one-time macOS permission
  prompt for the app — expected, same as the example.
- Token refreshes write back only to the tool's own credential files
  (`auth.json`, `oauth_creds.json`), matching each CLI's own behavior.

## Testing

- Rust unit tests per provider on the pure normalizers (fixture JSON →
  expected `ToolLimits`), matching the `adapters/` test convention.
  Must-cover edges: Codex weekly-window-in-primary-slot (free tier),
  Claude `weekly_scoped` extraction + Opus dedup, Gemini
  lowest-remaining-bucket-per-family, Grok missing `config`, Antigravity
  quota-summary → family rows.
- Keychain / `ps` / `lsof` / HTTP go behind injectable function
  parameters so normalizers test pure (the example does the same with
  `securityRunner` / `commandRunner` / `fetchImpl`).
- Frontend: existing vitest setup; a test for the cached-age label and
  window-to-bar mapping.
- End-to-end verification: run the app on this machine (real Claude /
  Codex / Gemini credentials present) and confirm live bars, then pull
  network to confirm stale-cache behavior.
