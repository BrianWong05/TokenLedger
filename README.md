# TokenLedger

A macOS desktop app (Tauri v2) that tracks token usage and estimated cost
across the AI coding tools on your machine — **Claude Code**, **Codex CLI**,
**Gemini CLI**, and **Hermes** — by parsing each tool's local logs into a
normalized SQLite ledger and showing a dark-themed dashboard: totals,
estimated cost, cache hit rate, a trend chart, and breakdowns by tool,
model, project, and date range.

## Screenshot

<!-- TODO: add docs/screenshot.png of the dashboard -->
![TokenLedger dashboard](docs/screenshot.png)

## What it does

- **Zero-effort tracking** — reads local logs automatically, no manual entry
  and no API keys. Scans on launch and on a configurable timer (off / 30s /
  60s, default 30s).
- **Per-event token detail** — input, output, cache write (5m / 1h TTL
  split), and cache read, normalized so the four categories are mutually
  exclusive across all four sources.
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

The database lives at `<app data dir>/tokenledger.db`
(`~/Library/Application Support/com.brianwong.tokenledger/tokenledger.db` on
macOS), in WAL mode.

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

## License

MIT
