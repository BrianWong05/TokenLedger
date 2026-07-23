# TokenLedger

A macOS desktop app (Tauri v2) that tracks token usage and estimated cost
across the AI coding tools on your machine — **Claude Code**, **Codex CLI**,
**Gemini CLI**, **Hermes**, **Grok**, **Google Antigravity**, and **pi** — by
parsing each tool's local logs into a normalized SQLite ledger and showing a
dark-themed dashboard: totals, estimated cost, cache hit rate, a trend chart,
and breakdowns by tool, model, project, and date range.

## Screenshot

<!-- TODO: add docs/screenshot.png of the dashboard -->
![TokenLedger dashboard](docs/screenshot.png)

## What it does

- **Zero-effort tracking** — reads local logs automatically, no manual entry
  and no API keys. Scans on launch and on a configurable timer (off / 30s /
  60s, default 30s).
- **Per-event token detail** — input, output, cache write (5m / 1h TTL
  split), and cache read, normalized so the four categories are mutually
  exclusive across every source.
- **Estimated cost** from public API list prices (LiteLLM's pricing
  database), with a bundled offline snapshot and user-editable per-model
  price overrides for self-hosted models (entered as `$ / 1M tokens`).
- **Durable history** — the SQLite database is a permanent ledger. It
  outlives the source logs (Claude Code prunes transcripts after ~30 days by
  default), so once ingested, history is retained even after the originals
  are gone.

Cost is labeled *at API list prices — not billed*: every source here is
subscription, free-tier, or self-hosted, so the number is an estimate, not
an invoice.

## Data sources

| Tool | Source | Notes |
|---|---|---|
| Claude Code | `~/.claude/projects/**/*.jsonl` | Incremental byte-offset resume; worktrees rolled up to the parent repo |
| Codex CLI | `~/.codex/sessions/**/rollout-*.jsonl` | Cumulative snapshots deltaed; duplicate snapshots self-correct |
| Gemini CLI | `~/.gemini/tmp/*/chats/session-*.json` | Replace-per-file on change; project path via `~/.gemini/projects.json` |
| Hermes | `~/.hermes/state.db` | Opened read-only; live session rows upserted |
| Grok | `~/.grok/sessions/**/updates.jsonl` | Cumulative context counter; one Turn = one Request |
| Google Antigravity | `~/.gemini/antigravity{,-cli}/conversations/*.db` | Protobuf usage blobs; IDE + CLI count as one Source |
| pi | `~/.pi/agent/sessions/**/*.jsonl` | Session tree: all branches counted, forks/clones deduplicated (see below) |

The database lives at `<app data dir>/tokenledger.db`
(`~/Library/Application Support/com.brianwong.tokenledger/tokenledger.db` on
macOS), in WAL mode.

### pi

pi Sessions are read from `~/.pi/agent/sessions` and, when those environment
variables are visible to TokenLedger, from `PI_CODING_AGENT_SESSION_DIR` and
`<PI_CODING_AGENT_DIR>/sessions` as well (equivalent roots are de-duplicated).
A missing pi installation is simply an empty Source, not an error.

A pi Session is a **tree**, not a flat transcript, and TokenLedger honors its
shape:

- **Every branch counts.** Usage on abandoned branches is real usage and stays
  in the Ledger; each Request's context is attributed along its *own* ancestor
  path, so a sibling branch never leaks into it.
- **Compaction and summaries** are counted. A built-in compaction is one
  Request that inherits the branch's active Model; afterwards, descendants see
  the summary and retained tail in place of the superseded prefix. Pre-compaction
  history is kept permanently.
- **Forks and clones don't double-count.** Copied history is deduplicated and
  keeps its original Project and Session; only genuinely new work in the child
  is attributed to the child.
- **Unattributed Usage.** pi reports usage on tool results and
  extension-provided summaries with no trustworthy Model. Those tokens are
  counted but carry no Model: they show as an "Unattributed usage" row, are
  excluded from Pricing, and make a mixed Cost *Partial* (or *unavailable* when
  a selection is entirely Unattributed) — never `$0`. Each such block counts as
  one Request, which is a **lower bound** when a block aggregates several hidden
  calls pi does not separate.
- **Privacy.** Prompts, responses, thinking, images, tool arguments, and
  tool-result bodies are read only transiently to estimate context size and are
  **never** persisted; only raw tool names, estimated sizes, and call counts
  reach the Ledger.

pi's own token totals should match TokenLedger's for the same discovered
Session corpus (see the parity check below), while **Cost may intentionally
differ**: TokenLedger ignores pi's logged cost and reprices everything through
its own Override and List Price rules.

## Requirements

- macOS (Apple Silicon)
- [Rust](https://rustup.rs/) (stable, 2021 edition)
- Node.js 18+ and npm
- Tauri v2 prerequisites (Xcode Command Line Tools)

## Build & run

```bash
# install frontend deps
npm install

# run in development (hot-reload frontend + Rust core)
npm run tauri dev

# build a release .app bundle
npm run tauri build
```

## Development

```bash
# Rust core: unit + adapter tests
cargo test --manifest-path src-tauri/Cargo.toml

# Frontend logic tests (date-range + formatting)
npx vitest run

# Type-check + production frontend build
npm run build
```

## Verifying Claude totals

TokenLedger and [`ccusage`](https://github.com/ryoppippi/ccusage) both read
Claude Code's transcripts and bucket in local time, so their token totals
should match closely:

```bash
npx ccusage@latest --json
```

Token categories line up; **cost will differ** — ccusage uses flat cache
pricing while TokenLedger prices 5-minute and 1-hour cache writes
separately.

## Verifying pi totals

An opt-in test independently sums the canonical token categories over your real
pi Sessions — deduplicating copied fork/clone history exactly as the Ledger
does — and asserts the Ledger's pi totals match. It reads local Session data,
so it is `#[ignore]`d and never runs by default (no Session content is
committed):

```bash
cargo test --manifest-path src-tauri/Cargo.toml pi_real_log_parity -- --ignored --nocapture
```

Token totals should match; Cost may differ, because TokenLedger reprices
everything through its own Override and List Price rules rather than trusting
pi's logged cost.

## License

MIT
