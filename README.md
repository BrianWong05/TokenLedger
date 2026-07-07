# TokenLedger

A macOS desktop app (Tauri v2) that tracks token usage and estimated cost
across the local AI coding tools on this machine: Claude Code, Codex CLI,
Gemini CLI, and Hermes. It parses each tool's local logs into a normalized
SQLite ledger and shows a dark-themed dashboard.

Design spec: `docs/superpowers/specs/2026-07-07-usage-tracker-design.md`.

## Development

```bash
npm install
npm run tauri dev    # run the desktop app
npm run build        # build the frontend
npm test             # frontend unit tests (vitest)
cargo test --manifest-path src-tauri/Cargo.toml   # Rust core tests
```
