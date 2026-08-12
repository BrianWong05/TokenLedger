# TokenLedger

A desktop app (Tauri v2) for macOS, Windows, and Linux that tracks token usage
and estimated cost across the AI coding agents and assistants on your machine —
**Claude Code**, **Codex CLI**, **Gemini CLI**, **Hermes**, **Grok Build**,
**Google Antigravity**, **Goose**, **OpenCode**, **Cline**, **Kilo**, **Zed**, **pi**, **WorkBuddy**, and **CodeBuddy** — by parsing each tool's local logs into a
normalized SQLite ledger.

**Status: 0.1.0.** Driven daily on macOS. Windows and Linux build and pass the
full test suite in CI (run on demand), but have had no real-world use yet —
expect rough edges there, and please report them.

![TokenLedger's Overview](docs/screenshot.png)

## What it does

- **Zero-effort tracking** — reads local logs automatically, no manual entry
  and no API keys. Scans on launch and on a configurable timer (off / 10s /
  30s / 60s / 5m, default 30s, or any whole number of seconds from 5s to 24h).
  Off stops only this window's own re-reads — background capture carries on,
  and Rescan still works.
- **Per-request token detail** — input, output, cache write (5m / 1h TTL
  split), and cache read, normalized so the four categories are mutually
  exclusive across every source.
- **Where the context went** — not just how many tokens, but what they were:
  messages, system prompt, and reasoning, with tool calls, subagents, MCP
  servers, and skills broken out beneath them, down to which tools and which
  Bash commands. Reported for Claude Code, Codex, and pi; the sub-figures are
  size estimates, labeled as such, while the headline splits are exact.
- **Estimated cost** from public API list prices (LiteLLM's pricing
  database), with a bundled offline snapshot and user-editable per-model
  price overrides for self-hosted models (entered as `$ / 1M tokens`).
- **Durable history** — the SQLite database is a permanent ledger. It
  outlives the source logs (Claude Code prunes transcripts after ~30 days by
  default), so once ingested, history is retained even after the originals
  are gone.
- **Always at hand** — a Menu Bar Extra carries today's tokens and cost beside
  its icon, and the panel behind it reads out a window you pick (today,
  yesterday, or the trailing 30 days). Closing the window leaves TokenLedger
  running so it keeps capturing.

Cost is labeled *at API list prices — not billed*: every source here is
subscription, free-tier, or self-hosted, so the number is an estimate, not
an invoice.

## In the app

| Tab | What's on it |
|---|---|
| **Overview** | Everything above, over a date window and source selection you choose — plus **Export** (the whole selected window written to one sectioned CSV), **Trend** (enlarges into its own window, bucket size, and per-bucket CSV export), **Activity** (a 12-month heatmap that enlarges into a rotatable 3D landscape), and **Profile** (a portrait of the whole ledger). Activity and Profile deliberately ignore the date window and source selection. |
| **Pricing** | Every model seen in the ledger with its resolved list price, the catalog it came from, and any override you've set |
| **Settings** | Theme (system / light / dark), language (English / 繁體中文), display currency at a fixed exchange rate, launch at login, auto-update checks, the scan interval, and **Custom range presets** — up to four of your own beside the four that ship, each a rolling *last N days* or a completed calendar period. Add one, reorder them by dragging (or with the arrows), retype a rolling one's day count, remove any; each row shows the dates it currently resolves to, and the picker lists them in the order you put them in |

## Install

Grab the build for your platform from
[Releases](https://github.com/BrianWong05/TokenLedger/releases/latest). Whichever
one you take, it updates itself from then on.

| Platform | Download | Before it runs |
|---|---|---|
| macOS (Apple Silicon) | `.dmg` | Not notarized yet: Gatekeeper says the app "is damaged and can't be opened" → **right-click the app → Open → Open** |
| Windows | `-setup.exe` | Not code-signed yet: SmartScreen says "Windows protected your PC" → **More info → Run anyway** |
| Linux | `.AppImage` | `chmod +x`, and the system tray needs `libayatana-appindicator3-1` (plus the AppIndicator extension on stock GNOME) |

Two things worth knowing before you install:

- **WSL is not scanned.** The Windows build reads the logs under your Windows
  home. Coding tools run inside WSL write to the Linux home instead, so a
  WSL-only setup shows an empty ledger.
- **The Menu Bar Extra is how you get back to the app.** Closing the window
  leaves TokenLedger running so it keeps capturing; on a Linux desktop with no
  system tray at all, that resident presence has nowhere to live.

## Privacy

TokenLedger reads your prompts, the models' responses, thinking, images, tool
arguments, and tool-result bodies — it has to, because sizing them is how the
context breakdown works. **None of that content is written to the database.** It
is measured and discarded.

What the database does hold:

- token counts, request counts, and byte-derived size estimates
- model names, session identifiers, and dates
- absolute paths you already know: the working directory a request came from,
  and the log file it was read out of
- the *names* of tools, MCP servers, skills, and subagents invoked
- for Claude Code's Bash drill-down only, a two-word command signature — the
  executable plus its first non-flag argument. `git commit`, `npm install`, and
  yes, `cat ~/clients/acme/prod.env`. That second word is a real exposure and it
  is deliberate; see
  [ADR-0011](docs/adr/0011-the-ledger-persists-names-never-content.md) for the
  trade-off and its bounds.

Nothing about you leaves the machine by default. The app itself makes exactly
three outbound requests, all fetches of public data: LiteLLM's price list and
OpenRouter's model list for pricing, and the GitHub release manifest for
updates. One optional feature reaches further and asks first: enabling **live
limit checks** on the Limits tab runs a separate companion process that presents
your sign-ins to their own vendors — Claude Code's to `api.anthropic.com`,
Codex's to `chatgpt.com`, Antigravity's to Google — read-only, only when you open
that page or press Refresh, never on a timer. Google sign-ins work on hourly
passes, so the companion first gets a fresh pass from Google the same way
Antigravity itself does; the pass is used once and never stored, and your saved
sign-in is never altered. Until you press that button, no credential is read and
no authenticated request exists
([ADR-0019](docs/adr/0019-live-limits-are-fetched-by-a-companion-never-the-app.md),
[ADR-0020](docs/adr/0020-a-companion-may-exchange-a-google-refresh-token.md)).

## Data sources

| Tool | What it is | Logs read |
|---|---|---|
| [Claude Code](https://claude.com/claude-code) | Anthropic's CLI coding agent | `~/.claude/projects/**/*.jsonl` |
| [Codex CLI](https://github.com/openai/codex) | OpenAI's CLI coding agent | `~/.codex/sessions/**/rollout-*.jsonl` |
| [Gemini CLI](https://github.com/google-gemini/gemini-cli) | Google's CLI coding agent | `~/.gemini/tmp/*/chats/session-*.json` |
| [Hermes](https://github.com/NousResearch/hermes-agent) | Nous Research's self-improving agent | `~/.hermes/state.db` (opened read-only) |
| [Grok Build](https://github.com/xai-org/grok-build) | Coding agent harness and TUI | `$GROK_HOME/sessions/**/updates.jsonl` (fallback `~/.grok/sessions/**/updates.jsonl`) |
| [Google Antigravity](https://antigravity.google) | Google's agentic development platform | `~/.gemini/antigravity{,-cli}/conversations/*.db` |
| [Goose](https://github.com/block/goose) | Block's local coding agent | `~/Library/Application Support/Block/goose/data/sessions/sessions.db` on macOS; `~/.local/share/goose/sessions/sessions.db` on Linux; `%APPDATA%\\Block\\goose\\data\\sessions\\sessions.db` on Windows; `$GOOSE_PATH_ROOT/data/sessions` when overridden (legacy `.jsonl` in the platform data directory) |
| [OpenCode](https://github.com/sst/opencode) | OpenCode CLI | `~/.local/share/opencode/opencode.db` (or `$OPENCODE_DB`), `opencode-<channel>.db`, and legacy `~/.local/share/opencode/storage` (or `$OPENCODE_DATA_DIR`) |
| [Cline](https://cline.bot) | IDE and CLI coding agent | VS Code and editor-server task folders; `~/.cline/data/sessions/*.json` or `$CLINE_DATA_DIR/sessions/*.json` |
| [Kilo](https://kilocode.ai) | Kilo Code CLI | `~/Library/Application Support/kilo/kilo.db` on macOS; `~/.local/share/kilo/kilo.db` on Linux; `%LOCALAPPDATA%\\kilo\\kilo.db` on Windows; `$KILO_DB` when overridden |
| [Zed](https://zed.dev) | Zed Editor's hosted agent | `~/Library/Application Support/Zed/threads/threads.db` on macOS; `~/.local/share/zed/threads/threads.db` on Linux; `%LOCALAPPDATA%\\Zed\\threads\\threads.db` on Windows; XDG and Flatpak data-home branches on Linux |
| [pi](https://github.com/earendil-works/pi) | Agent toolkit — unified LLM API, agent loop, TUI, coding agent CLI | `~/.pi/agent/sessions/**/*.jsonl` |
| WorkBuddy | Desktop AI assistant | `~/.workbuddy/projects/**/*.jsonl` |
| CodeBuddy | CLI, IDE, and VS Code plugin coding agent | `~/.codebuddy/projects/**/*.jsonl` |

Most paths above are under your home directory and are read passively. `GROK_HOME`
and `GOOSE_PATH_ROOT` may point discovery at different roots. The
database lives at `<app data dir>/tokenledger.db` in WAL
mode — `~/Library/Application Support/com.brianwong.tokenledger/` on macOS,
`%APPDATA%\com.brianwong.tokenledger\` on Windows,
`~/.local/share/com.brianwong.tokenledger/` on Linux.

## Where the numbers bend

Every source logs what it wants to, not what a ledger would like. Ten of them
distort something in a way worth knowing before you trust a figure.

- **Grok Build's cost is not trustworthy.** Its logs carry a single running
  token counter with no input/output/cache split, so every delta is booked as
  input. Grok can therefore never contribute to the cache hit rate, and its
  cost is computed at input rates for tokens that were really a mix. The token
  total is sound; the money figure beside it is not.
- **Hermes lands on the day a session started.** It stores usage per session
  rather than per call, timestamped at the session's start — so a session opened
  Monday and worked through Wednesday books all of its tokens on Monday. That
  bends Trend and Activity, not just the totals.
- **Google Antigravity counts as one source, and sees only its SQLite
  Sessions.** Its IDE and CLI keep separate Session databases; both
  are read and reported under a single Antigravity source rather than split.
  Sessions Antigravity stores as encrypted `.pb` files — the IDE's
  current default — are unreadable offline and contribute nothing; see
  `docs/source-evidence/antigravity.md`.
- **Claude Code rolls worktrees up.** A git worktree is attributed to its parent
  repository rather than appearing as a project of its own.
- **Goose has two supported storage shapes.** Modern `usage_ledger` rows are
  one Usage Record each. Legacy `.jsonl` files expose only a session aggregate,
  so they become one Usage Record at the session timestamp with a null Model.
  Goose's cache-write bucket has no TTL, so TokenLedger books it as 5-minute
  Cache write; Goose's logged Cost is ignored and repriced from TokenLedger's
  rates.
- **OpenCode, Kilo, and Zed book at Session granularity.** Their supported
  Artifacts prove Session totals but not a trustworthy timestamp for every
  Request, so each usage-bearing Session becomes one Usage Record at the
  Session's updated timestamp. Trend and Activity are honest about that coarse
  timing rather than inventing per-Request points; see the `docs/source-evidence/`
  records for the exact supported shapes.
- **WorkBuddy and CodeBuddy share one parser with two cache conventions.** Their
  transcripts' `inputTokens` include cache reads, so Input is derived by
  subtracting the cache-read figure — preferring the OpenAI-style
  `prompt_cache_hit_tokens`, falling back to the Anthropic-style
  `cache_read_input_tokens`. Subagent transcripts are scanned as extra Records
  in the parent Session, never double-counted. Their logged `credit` is
  ignored and repriced from TokenLedger's rates. See ADR-0016 and
  `docs/source-evidence/workbuddy.md`.

Everything else is exact, or says so when it isn't: a source that cannot
attribute a figure shows "—", never a zero standing in for an unknown.

## Goose

TokenLedger reads Goose's local `sessions.db` and legacy session `.jsonl` files
in the supported platform roots. It never starts Goose, opens a remote
service, authenticates, or reads Goose's raw request/response logs. The modern
database exposes `usage_ledger` rows with timestamps and per-row Model values;
legacy headers are read only from their first line and never retain conversation
content. Missing Model, relative Project paths, and unsupported Context
categories remain unavailable rather than being inferred.

## Cline

Cline is one Source across its VS Code task folders, remote editor-server task
folders, and CLI session snapshots. The scanner reads request usage from
`ui_messages.json`, legacy `claude_messages.json`, and the CLI's JSON session
records; the CLI `sessions.db` index is not treated as usage data. Equivalent
task/session IDs and request IDs deduplicate across surfaces, so an editor task
that is also present in CLI storage is one Usage Record rather than two.

CLI roots follow the documented override order `CLINE_DATA_DIR`, then
`CLINE_SANDBOX_DATA_DIR`, then `~/.cline/data`.
Blank and whitespace-only values are ignored. Cline's cache-write counter has
no TTL, so it is normalized to the 5-minute Cache write category. Missing
Model or relative Project values remain unavailable; no conversation content is
stored, and TokenLedger never starts, controls, or authenticates Cline.

## OpenCode

TokenLedger reads OpenCode's local SQLite database (`opencode.db` and
`opencode-<channel>.db` variants) and legacy JSON storage under the supported
platform roots, overridable with `OPENCODE_DB` and `OPENCODE_DATA_DIR`. The
database's Session totals are booked as one Usage Record per usage-bearing
Session at the Session's updated timestamp, because the supported Artifact does
not prove a trustworthy per-Request timestamp. Unknown or unproven Models
become Unattributed Usage rather than being guessed from the last Model in a
Session. TokenLedger never starts OpenCode, authenticates, or performs any
remote synchronization; no conversation content is stored.

## Kilo

TokenLedger reads Kilo CLI's current `kilo.db` session database from the
supported platform roots, overridable with `KILO_DB`. Usage is aggregated on
the Session row, so each usage-bearing Session becomes one Usage Record at the
Session's updated timestamp. Legacy editor migrations containing only zero-token
rows produce no Usage Records. Unknown or unproven Models become Unattributed
Usage; no conversation content is stored, and TokenLedger never starts,
authenticates, or synchronizes Kilo.

## Zed

TokenLedger reads Zed's native hosted-model thread database
(`threads/threads.db`) from the supported platform roots, including the XDG and
Flatpak data-home branches on Linux. Only thread rows whose serialized provider
is exactly `zed.dev` are persisted; external ACP sessions are excluded before
any Usage Record is written. Zed stores cumulative usage on a whole thread
without a trustworthy per-Request timestamp, so each usage-bearing Session
becomes one Usage Record at the Session's updated timestamp. Unknown Models
become Unattributed Usage; no conversation content is stored, and TokenLedger
never starts, authenticates, or synchronizes Zed. The evidence record in
`docs/source-evidence/zed.md` notes that the genuine private-Artifact gate was
still pending when the adapter landed.

## pi

pi sessions are read from `~/.pi/agent/sessions` and, when those environment
variables are visible to TokenLedger, from `PI_CODING_AGENT_SESSION_DIR` and
`<PI_CODING_AGENT_DIR>/sessions` as well (equivalent roots are de-duplicated).
A missing pi installation is simply an empty source, not an error.

A pi session is a **tree**, not a flat transcript, and TokenLedger honors its
shape:

- **Every branch counts.** Usage on abandoned branches is real usage and stays
  in the ledger; each request's context is attributed along its *own* ancestor
  path, so a sibling branch never leaks into it.
- **Compaction and summaries** are counted. A built-in compaction is one
  request that inherits the branch's active model; afterwards, descendants see
  the summary and retained tail in place of the superseded prefix.
  Pre-compaction history is kept permanently.
- **Forks and clones don't double-count.** Copied history is deduplicated and
  keeps its original project and session; only genuinely new work in the child
  is attributed to the child.
- **Unattributed usage.** pi reports usage on tool results and
  extension-provided summaries with no trustworthy model. Those tokens are
  counted but carry no model: they show as an "Unattributed usage" row, are
  excluded from Pricing, and make a mixed cost *partial* (or *unavailable* when
  a selection is entirely unattributed) — never `$0`. Each such block counts as
  one request, which is a **lower bound** when a block aggregates several hidden
  calls pi does not separate.
- **No system-prompt figure.** pi's logs never reveal it, so it is left unknown
  rather than estimated — the one context category TokenLedger declines to guess
  at for this source.

pi's own token totals should match TokenLedger's for the same discovered
sessions (see the parity check below), while **cost may intentionally differ**:
TokenLedger ignores pi's logged cost and reprices everything through its own
override and list-price rules.

## Development

### Requirements

- macOS (Apple Silicon), Windows, or Linux
- [Rust](https://rustup.rs/) (stable, 2021 edition)
- Node.js 18+ and npm
- Tauri v2 prerequisites — Xcode Command Line Tools on macOS, the MSVC build
  tools on Windows, and on Debian/Ubuntu:
  `libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev patchelf`

### Build & run

```bash
# install frontend deps
npm install

# run in development (hot-reload frontend + Rust core)
npm run tauri dev

# build a release bundle for the current platform
npm run tauri build
```

### Tests

```bash
# Rust core: unit + adapter tests
cargo test --manifest-path src-tauri/Cargo.toml

# Frontend tests (components + date-range/formatting logic)
npm test

# Type-check + production frontend build
npm run build
```

### Verifying pi totals

An opt-in test independently sums the canonical token categories over your real
pi sessions — deduplicating copied fork/clone history exactly as the ledger
does — and asserts the ledger's pi totals match. It reads local session data,
so it is `#[ignore]`d and never runs by default (no session content is
committed):

```bash
cargo test --manifest-path src-tauri/Cargo.toml pi_real_log_parity -- --ignored --nocapture
```

Token totals should match; cost may differ, because TokenLedger reprices
everything through its own override and list-price rules rather than trusting
pi's logged cost.

### Validating a private Source Artifact

A trusted contributor can validate one genuine local Source Artifact without
copying it into the repository or installing the corresponding coding tool.
The ignored workflow reads the selected path in place, runs the production
scan and Ledger queries against a temporary Ledger, and prints one JSON line
containing only normalized aggregate counts, a non-content schema fingerprint,
the selected Source key, and pass/fail status. It never prints the Artifact
path, private identifiers, raw rows, or content.

Set the Source Catalog key and the Artifact or Artifact root, then run:

```bash
TOKENLEDGER_VALIDATION_SOURCE=pi \
TOKENLEDGER_VALIDATION_ARTIFACT=/path/to/private/artifact-root \
cargo test --manifest-path src-tauri/Cargo.toml \
  source_artifact_validation::private_source_artifact_validation -- \
  --ignored --nocapture
```

Supported Source keys are `claude`, `codex`, `gemini`, `hermes`, `grok`,
`antigravity`, `goose`, `opencode`, `cline`, `kilo`, `zed`, and `pi`.
Real-Artifact validation is ignored by default and
is not required by CI; committed synthetic fixtures remain the deterministic
evidence for normal test runs. A production support claim still requires the
genuine Artifact to be corroborated by an upstream schema or writer, or by
several independent genuine samples; this local report does not replace that
gate.

### Reporting a window of the Ledger

In the app, the Overview's **Export** writes the selected window to one CSV —
the Month preset plus Export is a 30-day report. For the same figures without a
GUI (cron, CI, a script), an ignored workflow writes them to a folder of CSVs.
It runs the same queries the Overview does, so Cost, Partial Cost, Unpriced and
Unattributed Usage, and the Unreadable Artifact floor all carry their usual
meaning; the Ledger is opened read-only and never written.

```bash
cargo test --manifest-path src-tauri/Cargo.toml \
  report::tests::ledger_report -- --ignored --nocapture
```

It prints a summary and writes `tokenledger-report-<from>_<to>/` with
`summary.csv`, `by-day.csv`, `by-source.csv`, `by-model.csv`, and
`by-project.csv`. Because a figure must never be mistaken for a total,
`cost_usd` is blank rather than `0` where Cost is unavailable, and a
`cost_basis` column says `exact`, `partial`, or `unavailable` beside it —
with `tokens_basis` doing the same for the "≥" floor.

| Variable | Default |
|---|---|
| `TOKENLEDGER_REPORT_DAYS` | `30` — trailing local days, today included |
| `TOKENLEDGER_REPORT_DB` | the installed app's Ledger for this platform |
| `TOKENLEDGER_REPORT_OUT` | `tokenledger-report-<from>_<to>/` in the repository root |

## Contributing

The domain vocabulary lives in [CONTEXT.md](CONTEXT.md) and the decisions
behind the surprising parts live in [docs/adr/](docs/adr/). Both are worth a
read before changing behavior — several things that look like bugs are
load-bearing.

## License

[MIT](LICENSE)
