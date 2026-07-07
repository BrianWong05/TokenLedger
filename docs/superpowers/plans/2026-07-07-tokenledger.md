# TokenLedger Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build TokenLedger, a Tauri v2 macOS desktop app that parses local AI-tool logs (Claude Code, Codex CLI, Gemini CLI, Hermes) into a permanent SQLite ledger and shows a dark-themed dashboard of token usage and estimated cost.

**Architecture:** A small Rust core does all parsing, storage, and aggregation: four source adapters emit normalized `UsageEvent` rows into SQLite (WAL); pricing joins happen at query time via a `RateMap` (user override → exact LiteLLM key → guarded normalized fallback). A React 18 + Recharts frontend renders one dashboard screen over six Tauri IPC commands. The spec at `docs/superpowers/specs/2026-07-07-usage-tracker-design.md` is the source of truth — it encodes verified facts about the real log formats (Codex cumulative snapshots, cached ⊂ input for Codex/Gemini, Claude 5m/1h cache TTL split); when in doubt, the spec wins.

**Tech Stack:** Tauri 2, Rust 2021 (rusqlite bundled, serde, serde_json, dirs, ureq; tempfile as dev-dep), React 18 + TypeScript + Vite, Recharts, vitest. Target: macOS Apple Silicon.

## Global Constraints

- App identifier `com.brianwong.tokenledger`; product name `TokenLedger`.
- DB at `<app_data_dir>/tokenledger.db`; WAL mode; `busy_timeout` 5000ms; `PRAGMA user_version = 1`; migrations in-place — never drop-and-rescan (the DB is a permanent ledger; Claude Code deletes source logs after ~30 days).
- Events are never deleted by scans; the only exception is Gemini's replace-per-file, which fires only when a still-existing file changes. `scanned_files` rows for missing paths may be pruned; events may not.
- Token invariant: `input_tokens`, `cache_read_tokens`, `cache_write_{5m,1h}_tokens` are mutually exclusive. Codex and Gemini adapters subtract cached from raw input (their logs report cached as a subset of input). Total prompt = input + cache_read + cache_write_5m + cache_write_1h.
- Hero "total tokens" = input + output + cache_read + cache_write (all four categories).
- Requests = `SUM(api_calls)`; Hermes writes `api_call_count` per session, all other adapters write 1 per event.
- Cost is never stored on events; it is computed at query time. Lookup order: user override → exact LiteLLM key → normalized fallback with guards. Unknown model → "unpriced", never $0.
- Raw model strings are displayed everywhere; normalization exists only inside price matching.
- All IPC structs use serde `rename_all = "camelCase"`. Frontend computes date-range epoch bounds (start inclusive, end exclusive) in local time; SQL buckets via `'localtime'`.
- UI copy (exact): cost sub-label `at API list prices — not billed`; partial pricing marker `≥ $<amount> · <N> unpriced models`; zero-priced display `unpriced`; footer states `scanning…` / `last scan <relative time>`; override action `set price…` with fields labeled `$ / 1M tokens`.
- English UI, dark theme, single window. Conventional commits after every task.

---

### Task 1: Scaffold

Scaffold the Tauri v2 + React-TS project at the repo root, pin the Rust and
frontend dependencies every later task assumes, set the app identity, and prove
the three build/test gates are green with empty suites. There is no unit test to
write in this task; the deliverable is the working scaffold and the green gates
(`cargo test`, `npm run build`, `npx vitest run`).

**Files:**
- Create (via `create-tauri-app`, then customized below): `package.json`,
  `index.html`, `vite.config.ts`, `tsconfig.json`, `tsconfig.node.json`,
  `.gitignore`, `.vscode/extensions.json`, `public/*`, `src/main.tsx`,
  `src/App.tsx`, `src/App.css`, `src/assets/*`, `src/vite-env.d.ts`,
  `src-tauri/Cargo.toml`, `src-tauri/build.rs`, `src-tauri/tauri.conf.json`,
  `src-tauri/.gitignore`, `src-tauri/capabilities/default.json`,
  `src-tauri/icons/*`, `src-tauri/src/main.rs`, `src-tauri/src/lib.rs`
- Create (authored): `vitest.config.ts`, `README.md`
- Modify: `src-tauri/Cargo.toml`, `package.json`, `src-tauri/tauri.conf.json`
- Test: none (green gates are the empty `cargo test` / `npm run build` /
  `npx vitest run` suites)

**Interfaces:**
- Consumes: nothing (first task).
- Produces (later tasks rely on these, do not rename):
  - Rust crate `tokenledger` with `[lib] name = "tokenledger_lib"`; entry
    `pub fn run()` in `src-tauri/src/lib.rs`, called by `src-tauri/src/main.rs`
    as `tokenledger_lib::run()`.
  - `src-tauri/Cargo.toml` dependencies present after this task:
    `tauri = "2"`, `serde` (derive), `serde_json`, `rusqlite` (feature
    `bundled`), `dirs = "5"`, `ureq = "2"`; dev-dependency `tempfile = "3"`.
  - `package.json` with `recharts` (dep) and `vitest` (devDep); `npm test`
    runs `vitest run`.
  - `vitest.config.ts` with `test.passWithNoTests: true` so `npx vitest run`
    exits 0 until Task 11 adds real tests.
  - App identity: `productName` `TokenLedger`, `identifier`
    `com.brianwong.tokenledger`.

---

- [ ] **Step 1: Scaffold with create-tauri-app and copy into the repo root**

  `create-tauri-app` refuses to scaffold into a non-empty directory (the repo
  already contains `docs/` and `.git/`), so scaffold into a sibling directory
  named `tokenledger` — the name makes the tool auto-derive the crate name
  `tokenledger`, the lib name `tokenledger_lib`, and the identifier
  `com.brianwong.tokenledger` — then copy its contents in (dotfiles included)
  and delete the sibling. The fresh scaffold has no `node_modules`/`target`
  yet, so the copy is small, and `cp -R <dir>/. <repo>/` preserves the
  existing `docs/` and `.git/`.

  ```bash
  cd /Users/brianwong/Project
  rm -rf /Users/brianwong/Project/tokenledger
  npm create tauri-app@latest tokenledger -- --template react-ts --manager npm --yes
  cp -R /Users/brianwong/Project/tokenledger/. /Users/brianwong/Project/usage/
  rm -rf /Users/brianwong/Project/tokenledger
  ```

  Expected: the tool prints `Template created!`. Afterward
  `/Users/brianwong/Project/usage/` contains `package.json`, `index.html`,
  `src/`, `src-tauri/`, `.gitignore`, etc., while `docs/` and `.git/` are
  untouched. Verify the auto-derived names:

  ```bash
  grep -E '^name' /Users/brianwong/Project/usage/src-tauri/Cargo.toml
  grep identifier /Users/brianwong/Project/usage/src-tauri/tauri.conf.json
  ```

  Expected output:
  ```
  name = "tokenledger"
  name = "tokenledger_lib"
    "identifier": "com.brianwong.tokenledger",
  ```

- [ ] **Step 2: Replace `src-tauri/Cargo.toml` with the pinned dependency set**

  Overwrite `/Users/brianwong/Project/usage/src-tauri/Cargo.toml` with exactly:

  ```toml
  [package]
  name = "tokenledger"
  version = "0.1.0"
  description = "TokenLedger — AI usage tracker"
  authors = ["Brian Wong"]
  edition = "2021"

  [lib]
  name = "tokenledger_lib"
  crate-type = ["staticlib", "cdylib", "rlib"]

  [build-dependencies]
  tauri-build = { version = "2", features = [] }

  [dependencies]
  tauri = { version = "2", features = [] }
  tauri-plugin-opener = "2"
  serde = { version = "1", features = ["derive"] }
  serde_json = "1"
  rusqlite = { version = "0.32", features = ["bundled"] }
  dirs = "5"
  ureq = "2"

  [dev-dependencies]
  tempfile = "3"
  ```

- [ ] **Step 3: Replace `package.json` with React 18 pinned + recharts + vitest**

  The current `create-tauri-app` scaffolds React 19; the project targets
  React 18, so pin `react`/`react-dom` and their `@types` to 18. Overwrite
  `/Users/brianwong/Project/usage/package.json` with exactly:

  ```json
  {
    "name": "tokenledger",
    "private": true,
    "version": "0.1.0",
    "type": "module",
    "scripts": {
      "dev": "vite",
      "build": "tsc && vite build",
      "preview": "vite preview",
      "tauri": "tauri",
      "test": "vitest run"
    },
    "dependencies": {
      "react": "^18.3.1",
      "react-dom": "^18.3.1",
      "@tauri-apps/api": "^2",
      "@tauri-apps/plugin-opener": "^2",
      "recharts": "^2.12.0"
    },
    "devDependencies": {
      "@types/react": "^18.3.12",
      "@types/react-dom": "^18.3.1",
      "@vitejs/plugin-react": "^4.6.0",
      "typescript": "~5.8.3",
      "vite": "^7.0.4",
      "@tauri-apps/cli": "^2",
      "vitest": "^3.0.0"
    }
  }
  ```

- [ ] **Step 4: Create `vitest.config.ts`**

  Create `/Users/brianwong/Project/usage/vitest.config.ts` with exactly:

  ```ts
  import { defineConfig } from 'vitest/config';

  export default defineConfig({
    test: {
      passWithNoTests: true,
    },
  });
  ```

- [ ] **Step 5: Replace `src-tauri/tauri.conf.json` with the app identity**

  Set `productName` to `TokenLedger`, pin `identifier` to
  `com.brianwong.tokenledger`, and set the window `title` to `TokenLedger`.
  Window sizing (1100x780 / min 900x640) is finalized in Task 10; leave the
  default 800x600 here. Overwrite
  `/Users/brianwong/Project/usage/src-tauri/tauri.conf.json` with exactly:

  ```json
  {
    "$schema": "https://schema.tauri.app/config/2",
    "productName": "TokenLedger",
    "version": "0.1.0",
    "identifier": "com.brianwong.tokenledger",
    "build": {
      "beforeDevCommand": "npm run dev",
      "devUrl": "http://localhost:1420",
      "beforeBuildCommand": "npm run build",
      "frontendDist": "../dist"
    },
    "app": {
      "windows": [
        {
          "title": "TokenLedger",
          "width": 800,
          "height": 600
        }
      ],
      "security": {
        "csp": null
      }
    },
    "bundle": {
      "active": true,
      "targets": "all",
      "icon": [
        "icons/32x32.png",
        "icons/128x128.png",
        "icons/128x128@2x.png",
        "icons/icon.icns",
        "icons/icon.ico"
      ]
    }
  }
  ```

- [ ] **Step 6: Create `README.md` stub**

  Create `/Users/brianwong/Project/usage/README.md` with exactly:

  ```markdown
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
  ```

- [ ] **Step 7: Install dependencies and verify all three gates are green**

  ```bash
  cd /Users/brianwong/Project/usage
  npm install
  npm run build
  npx vitest run
  cd /Users/brianwong/Project/usage/src-tauri && cargo test
  ```

  Expected:
  - `npm install` → `found 0 vulnerabilities` (a `recharts` v3-migration
    deprecation warning is expected and harmless).
  - `npm run build` → ends with `✓ built in <ms>` (runs `tsc` then
    `vite build`; no type errors).
  - `npx vitest run` → `No test files found, exiting with code 0`.
  - `cargo test` → compiles `rusqlite` (bundled) + `tauri` (~2 min first
    build) and prints three `test result: ok. 0 passed; 0 failed; ...`
    blocks (lib, bin, doc-tests). Exit code 0.

- [ ] **Step 8: Commit**

  ```bash
  cd /Users/brianwong/Project/usage
  git add -A
  git commit -m "chore: scaffold Tauri v2 + React 18 app (TokenLedger)"
  ```


---

### Task 2: types.rs + db.rs

Normalized event/type definitions plus the SQLite ledger layer: schema creation,
in-place migration keyed on `user_version`, WAL + busy-timeout connection open,
and the insert/upsert/replace/file-state/prune helpers every adapter depends on.

**Files:**
- Create: `src-tauri/src/types.rs`
- Create: `src-tauri/src/db.rs`
- Modify: `src-tauri/src/lib.rs` (declare the two new modules)
- Test: `src-tauri/src/db.rs` (`#[cfg(test)]` module inside the file under test)

**Interfaces:**
- Consumes: Task 1 scaffold only — `Cargo.toml` already has `rusqlite` (bundled),
  `serde`, `serde_json`, `dirs`, `ureq`, and `tempfile` (dev-dependency). Lib
  crate is `tokenledger_lib`.
- Produces (later tasks 3–10 rely on these exact signatures — do not rename):

```rust
// types.rs
#[derive(Debug, Clone, PartialEq)]
pub struct UsageEvent {
    pub dedup_key: String,
    pub source: String,            // "claude" | "codex" | "gemini" | "hermes"
    pub timestamp: i64,            // epoch seconds UTC
    pub model: String,
    pub project: Option<String>,   // absolute path (worktrees rolled up)
    pub api_calls: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_5m_tokens: i64,
    pub cache_write_1h_tokens: i64,
    pub source_file: String,
}
#[derive(Debug, Clone, Copy)]
pub struct FileState { pub size: i64, pub mtime: i64, pub byte_offset: i64 }
#[derive(Debug, Default, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SourceScanResult { pub events_inserted: u64, pub lines_skipped: u64, pub error: Option<String> }
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SourceStatus { pub source: String, pub events_inserted: u64, pub lines_skipped: u64, pub error: Option<String> }
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatus { pub sources: Vec<SourceStatus>, pub scanned_at: i64 }

// db.rs
pub fn open_db(path: &std::path::Path) -> rusqlite::Result<rusqlite::Connection>;
pub fn insert_events(conn: &mut Connection, events: &[UsageEvent]) -> rusqlite::Result<u64>;
pub fn upsert_events(conn: &mut Connection, events: &[UsageEvent]) -> rusqlite::Result<()>;
pub fn replace_file_events(conn: &mut Connection, source_file: &str, events: &[UsageEvent]) -> rusqlite::Result<()>;
pub fn get_file_state(conn: &Connection, path: &str) -> rusqlite::Result<Option<FileState>>;
pub fn set_file_state(conn: &Connection, path: &str, state: FileState) -> rusqlite::Result<()>;
pub fn prune_missing_files(conn: &Connection) -> rusqlite::Result<u64>;
```

---

- [ ] **Step 1: Create `src-tauri/src/types.rs`** (complete, final content)

```rust
use serde::Serialize;

#[derive(Debug, Clone, PartialEq)]
pub struct UsageEvent {
    pub dedup_key: String,
    pub source: String,
    pub timestamp: i64,
    pub model: String,
    pub project: Option<String>,
    pub api_calls: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_5m_tokens: i64,
    pub cache_write_1h_tokens: i64,
    pub source_file: String,
}

#[derive(Debug, Clone, Copy)]
pub struct FileState {
    pub size: i64,
    pub mtime: i64,
    pub byte_offset: i64,
}

#[derive(Debug, Default, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SourceScanResult {
    pub events_inserted: u64,
    pub lines_skipped: u64,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SourceStatus {
    pub source: String,
    pub events_inserted: u64,
    pub lines_skipped: u64,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatus {
    pub sources: Vec<SourceStatus>,
    pub scanned_at: i64,
}
```

- [ ] **Step 2: Create `src-tauri/src/db.rs` with imports + the failing test module only** (no
  implementation yet — the tests reference functions that do not exist, so the
  build fails at Step 4). Write exactly this:

```rust
use crate::types::{FileState, UsageEvent};
use rusqlite::{params, Connection, OptionalExtension};

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event(dedup_key: &str, source_file: &str) -> UsageEvent {
        UsageEvent {
            dedup_key: dedup_key.to_string(),
            source: "claude".to_string(),
            timestamp: 1_700_000_000,
            model: "claude-sonnet-4-20250514".to_string(),
            project: Some("/Users/dev/projects/alpha".to_string()),
            api_calls: 1,
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 10,
            cache_write_5m_tokens: 5,
            cache_write_1h_tokens: 2,
            source_file: source_file.to_string(),
        }
    }

    fn temp_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_db(&dir.path().join("test.db")).unwrap();
        (dir, conn)
    }

    #[test]
    fn fresh_db_has_tables_and_user_version() {
        let (_dir, conn) = temp_db();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 1);
        for table in ["events", "scanned_files", "prices", "price_overrides"] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "table {table} missing");
        }
    }

    #[test]
    fn open_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        {
            let mut conn = open_db(&path).unwrap();
            insert_events(&mut conn, &[sample_event("claude:a:1", "f1.jsonl")]).unwrap();
        }
        // Re-opening must migrate cleanly without wiping data.
        let conn = open_db(&path).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 1);
    }

    #[test]
    fn insert_ignores_duplicates_and_counts_inserted() {
        let (_dir, mut conn) = temp_db();
        let first = insert_events(
            &mut conn,
            &[
                sample_event("claude:a:1", "f1.jsonl"),
                sample_event("claude:b:1", "f1.jsonl"),
            ],
        )
        .unwrap();
        assert_eq!(first, 2);
        // One duplicate key + one new: only the new one counts.
        let second = insert_events(
            &mut conn,
            &[
                sample_event("claude:a:1", "f1.jsonl"),
                sample_event("claude:c:1", "f1.jsonl"),
            ],
        )
        .unwrap();
        assert_eq!(second, 1);
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 3);
    }

    #[test]
    fn upsert_replaces_existing_row() {
        let (_dir, mut conn) = temp_db();
        insert_events(&mut conn, &[sample_event("hermes:s1", "state.db")]).unwrap();
        let mut grown = sample_event("hermes:s1", "state.db");
        grown.output_tokens = 999;
        upsert_events(&mut conn, &[grown]).unwrap();
        let out: i64 = conn
            .query_row(
                "SELECT output_tokens FROM events WHERE dedup_key='hermes:s1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(out, 999);
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 1);
    }

    #[test]
    fn replace_file_events_is_scoped_to_source_file() {
        let (_dir, mut conn) = temp_db();
        insert_events(
            &mut conn,
            &[
                sample_event("gemini:s1:m1", "sessionA.json"),
                sample_event("gemini:s2:m1", "sessionB.json"),
            ],
        )
        .unwrap();
        // Replace only sessionA's events.
        replace_file_events(
            &mut conn,
            "sessionA.json",
            &[sample_event("gemini:s1:m2", "sessionA.json")],
        )
        .unwrap();
        let a_key: String = conn
            .query_row(
                "SELECT dedup_key FROM events WHERE source_file='sessionA.json'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(a_key, "gemini:s1:m2");
        let b: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE source_file='sessionB.json'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(b, 1);
    }

    #[test]
    fn file_state_roundtrip_and_overwrite() {
        let (_dir, conn) = temp_db();
        assert!(get_file_state(&conn, "f1.jsonl").unwrap().is_none());
        set_file_state(
            &conn,
            "f1.jsonl",
            FileState { size: 1024, mtime: 42, byte_offset: 512 },
        )
        .unwrap();
        let st = get_file_state(&conn, "f1.jsonl").unwrap().unwrap();
        assert_eq!((st.size, st.mtime, st.byte_offset), (1024, 42, 512));
        set_file_state(
            &conn,
            "f1.jsonl",
            FileState { size: 2048, mtime: 43, byte_offset: 1000 },
        )
        .unwrap();
        let st2 = get_file_state(&conn, "f1.jsonl").unwrap().unwrap();
        assert_eq!((st2.size, st2.mtime, st2.byte_offset), (2048, 43, 1000));
    }

    #[test]
    fn prune_removes_missing_files_but_never_events() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(&dir.path().join("test.db")).unwrap();

        let real = dir.path().join("real.jsonl");
        std::fs::write(&real, b"x").unwrap();
        let real_str = real.to_str().unwrap().to_string();

        set_file_state(&conn, &real_str, FileState { size: 1, mtime: 0, byte_offset: 0 }).unwrap();
        set_file_state(
            &conn,
            "/nonexistent/gone.jsonl",
            FileState { size: 1, mtime: 0, byte_offset: 0 },
        )
        .unwrap();

        // The ledger is permanent: an event referencing the missing file must survive.
        insert_events(&mut conn, &[sample_event("claude:x:1", "/nonexistent/gone.jsonl")]).unwrap();

        let removed = prune_missing_files(&conn).unwrap();
        assert_eq!(removed, 1);

        let files: i64 = conn
            .query_row("SELECT COUNT(*) FROM scanned_files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(files, 1);
        let events: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(events, 1);
    }
}
```

- [ ] **Step 3: Declare the modules in `src-tauri/src/lib.rs`.** Add these two lines at
  the very top of the file (above the existing Tauri `run()` / command code left
  by Task 1's scaffold):

```rust
pub mod types;
pub mod db;
```

- [ ] **Step 4: Run the tests — expect a compile failure.**

```bash
cd src-tauri && cargo test db::tests -- --nocapture
```

Expected: FAIL to compile with `error[E0425]: cannot find function `open_db` in
this scope` (and the same for `insert_events`, `upsert_events`,
`replace_file_events`, `get_file_state`, `set_file_state`,
`prune_missing_files`). This confirms the tests are wired and exercising the
not-yet-written API.

- [ ] **Step 5: Implement the db layer.** Insert this block into
  `src-tauri/src/db.rs` between the `use` lines and the `#[cfg(test)]` module
  (complete, final code):

```rust
const SCHEMA: &str = "\
BEGIN;
CREATE TABLE IF NOT EXISTS events (
  dedup_key TEXT PRIMARY KEY,
  source TEXT NOT NULL,
  timestamp INTEGER NOT NULL,
  model TEXT NOT NULL,
  project TEXT,
  api_calls INTEGER NOT NULL DEFAULT 1,
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens INTEGER NOT NULL DEFAULT 0,
  cache_write_5m_tokens INTEGER NOT NULL DEFAULT 0,
  cache_write_1h_tokens INTEGER NOT NULL DEFAULT 0,
  source_file TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_events_ts ON events(timestamp);
CREATE INDEX IF NOT EXISTS idx_events_file ON events(source_file);
CREATE TABLE IF NOT EXISTS scanned_files (
  path TEXT PRIMARY KEY,
  size INTEGER,
  mtime INTEGER,
  byte_offset INTEGER
);
CREATE TABLE IF NOT EXISTS prices (
  model TEXT PRIMARY KEY,
  input_per_tok REAL,
  output_per_tok REAL,
  cache_read_per_tok REAL,
  cache_write_5m_per_tok REAL,
  cache_write_1h_per_tok REAL
);
CREATE TABLE IF NOT EXISTS price_overrides (
  model TEXT PRIMARY KEY,
  input_per_tok REAL,
  output_per_tok REAL,
  cache_read_per_tok REAL,
  cache_write_per_tok REAL
);
PRAGMA user_version = 1;
COMMIT;";

const INSERT_SQL: &str = "INSERT OR IGNORE INTO events \
(dedup_key, source, timestamp, model, project, api_calls, input_tokens, output_tokens, \
cache_read_tokens, cache_write_5m_tokens, cache_write_1h_tokens, source_file) \
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)";

const REPLACE_SQL: &str = "INSERT OR REPLACE INTO events \
(dedup_key, source, timestamp, model, project, api_calls, input_tokens, output_tokens, \
cache_read_tokens, cache_write_5m_tokens, cache_write_1h_tokens, source_file) \
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)";

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 1 {
        conn.execute_batch(SCHEMA)?;
    }
    Ok(())
}

pub fn open_db(path: &std::path::Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    // journal_mode returns the applied mode as a row, so read it via query_row.
    let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    conn.busy_timeout(std::time::Duration::from_millis(5000))?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn insert_events(conn: &mut Connection, events: &[UsageEvent]) -> rusqlite::Result<u64> {
    let tx = conn.transaction()?;
    let mut inserted = 0u64;
    {
        let mut stmt = tx.prepare(INSERT_SQL)?;
        for e in events {
            // execute() returns changes(): 1 when inserted, 0 when the key already exists.
            inserted += stmt.execute(params![
                e.dedup_key, e.source, e.timestamp, e.model, e.project, e.api_calls,
                e.input_tokens, e.output_tokens, e.cache_read_tokens,
                e.cache_write_5m_tokens, e.cache_write_1h_tokens, e.source_file
            ])? as u64;
        }
    }
    tx.commit()?;
    Ok(inserted)
}

pub fn upsert_events(conn: &mut Connection, events: &[UsageEvent]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(REPLACE_SQL)?;
        for e in events {
            stmt.execute(params![
                e.dedup_key, e.source, e.timestamp, e.model, e.project, e.api_calls,
                e.input_tokens, e.output_tokens, e.cache_read_tokens,
                e.cache_write_5m_tokens, e.cache_write_1h_tokens, e.source_file
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn replace_file_events(
    conn: &mut Connection,
    source_file: &str,
    events: &[UsageEvent],
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM events WHERE source_file = ?1", [source_file])?;
    {
        let mut stmt = tx.prepare(INSERT_SQL)?;
        for e in events {
            stmt.execute(params![
                e.dedup_key, e.source, e.timestamp, e.model, e.project, e.api_calls,
                e.input_tokens, e.output_tokens, e.cache_read_tokens,
                e.cache_write_5m_tokens, e.cache_write_1h_tokens, e.source_file
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn get_file_state(conn: &Connection, path: &str) -> rusqlite::Result<Option<FileState>> {
    conn.query_row(
        "SELECT size, mtime, byte_offset FROM scanned_files WHERE path = ?1",
        [path],
        |row| {
            Ok(FileState {
                size: row.get(0)?,
                mtime: row.get(1)?,
                byte_offset: row.get(2)?,
            })
        },
    )
    .optional()
}

pub fn set_file_state(conn: &Connection, path: &str, state: FileState) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO scanned_files (path, size, mtime, byte_offset) \
         VALUES (?1, ?2, ?3, ?4)",
        params![path, state.size, state.mtime, state.byte_offset],
    )?;
    Ok(())
}

pub fn prune_missing_files(conn: &Connection) -> rusqlite::Result<u64> {
    let paths: Vec<String> = {
        let mut stmt = conn.prepare("SELECT path FROM scanned_files")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<String>>>()?
    };
    let mut removed = 0u64;
    for p in paths {
        if !std::path::Path::new(&p).exists() {
            conn.execute("DELETE FROM scanned_files WHERE path = ?1", [&p])?;
            removed += 1;
        }
    }
    Ok(removed)
}
```

- [ ] **Step 6: Run the tests — expect all green.**

```bash
cd src-tauri && cargo test db::tests -- --nocapture
```

Expected: `test result: ok. 7 passed; 0 failed` (the seven tests
`fresh_db_has_tables_and_user_version`, `open_is_idempotent`,
`insert_ignores_duplicates_and_counts_inserted`, `upsert_replaces_existing_row`,
`replace_file_events_is_scoped_to_source_file`,
`file_state_roundtrip_and_overwrite`,
`prune_removes_missing_files_but_never_events`).

- [ ] **Step 7: Commit.**

```bash
git add src-tauri/src/types.rs src-tauri/src/db.rs src-tauri/src/lib.rs
git commit -m "feat: add normalized types and SQLite ledger layer (schema, migrate, insert/upsert/replace, file-state, prune)"
```


---

### Task 3: Claude adapter

Parses `~/.claude/projects/**/*.jsonl` into normalized `UsageEvent`s: usage
extraction, 5m/1h cache-write split (absent sub-object → whole creation total in
5m), all-zero `<synthetic>` skip, `cwd` project with worktree roll-up, dedup on
`message.id`+`requestId` (fallback to `message.id`), byte-offset resume
(complete-lines only; trailing partial left unconsumed; size shrink → full
reparse), timestamp from the line's ISO `timestamp` field.

**Files:**
- Create: `src-tauri/src/adapters/mod.rs`
- Create: `src-tauri/src/adapters/claude.rs`
- Create: `src-tauri/tests/fixtures/claude/projects/alpha/session1.jsonl`
- Create: `src-tauri/tests/fixtures/claude/projects/alpha/session2.jsonl`
- Create: `src-tauri/tests/fixtures/claude/projects/beta/session3.jsonl`
- Modify: `src-tauri/src/lib.rs` (add `pub mod adapters;`)
- Test: `src-tauri/src/adapters/claude.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes (from Task 2, copied verbatim):
  ```rust
  pub struct UsageEvent {
      pub dedup_key: String,
      pub source: String,
      pub timestamp: i64,
      pub model: String,
      pub project: Option<String>,
      pub api_calls: i64,
      pub input_tokens: i64,
      pub output_tokens: i64,
      pub cache_read_tokens: i64,
      pub cache_write_5m_tokens: i64,
      pub cache_write_1h_tokens: i64,
      pub source_file: String,
  }
  pub struct FileState { pub size: i64, pub mtime: i64, pub byte_offset: i64 }
  pub struct SourceScanResult { pub events_inserted: u64, pub lines_skipped: u64, pub error: Option<String> } // derives Default
  pub fn open_db(path: &std::path::Path) -> rusqlite::Result<rusqlite::Connection>;
  pub fn insert_events(conn: &mut Connection, events: &[UsageEvent]) -> rusqlite::Result<u64>;
  pub fn get_file_state(conn: &Connection, path: &str) -> rusqlite::Result<Option<FileState>>;
  pub fn set_file_state(conn: &Connection, path: &str, state: FileState) -> rusqlite::Result<()>;
  ```
- Produces (Task 7 relies on, copied verbatim):
  ```rust
  pub fn scan_claude(conn: &mut Connection, projects_root: &Path) -> SourceScanResult;
  ```

---

- [ ] **Step 1: Create the fixture files**

  Run this exactly (the `printf` at the end appends the trailing partial line
  with **no** newline, so `session3.jsonl` ends mid-line on purpose):

  ```bash
  cd src-tauri
  mkdir -p tests/fixtures/claude/projects/alpha tests/fixtures/claude/projects/beta

  cat > tests/fixtures/claude/projects/alpha/session1.jsonl <<'EOF'
  {"type":"assistant","requestId":"req_1","timestamp":"2026-06-01T10:00:00.000Z","cwd":"/Users/dev/projects/alpha","message":{"id":"msg_aaa","model":"claude-opus-4-8","usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":20,"cache_creation_input_tokens":10,"cache_creation":{"ephemeral_5m_input_tokens":4,"ephemeral_1h_input_tokens":6}}}}
  {"type":"assistant","requestId":"req_9","timestamp":"2026-06-01T10:05:00.000Z","cwd":"/Users/dev/projects/alpha","message":{"id":"msg_dup","model":"claude-sonnet-4-5","usage":{"input_tokens":200,"output_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}
  EOF

  cat > tests/fixtures/claude/projects/alpha/session2.jsonl <<'EOF'
  {"type":"assistant","requestId":"req_9","timestamp":"2026-06-01T10:05:00.000Z","cwd":"/Users/dev/projects/alpha","message":{"id":"msg_dup","model":"claude-sonnet-4-5","usage":{"input_tokens":200,"output_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}
  {"type":"assistant","requestId":"req_2","timestamp":"2026-06-01T11:00:00.000Z","cwd":"/Users/dev/projects/alpha","message":{"id":"msg_bbb","model":"claude-opus-4-8","usage":{"input_tokens":300,"output_tokens":100,"cache_read_input_tokens":50,"cache_creation_input_tokens":80}}}
  EOF

  cat > tests/fixtures/claude/projects/beta/session3.jsonl <<'EOF'
  {"type":"assistant","timestamp":"2026-06-02T09:00:00.000Z","cwd":"/Users/dev/projects/beta","message":{"id":"msg_syn","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}
  {"type":"assistant","timestamp":"2026-06-02T09:10:00.000Z","cwd":"/Users/dev/projects/beta","message":{"id":"msg_ccc","model":"claude-opus-4-8","usage":{"input_tokens":40,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}
  {"type":"assistant","requestId":"req_3","timestamp":"2026-06-02T09:20:00.000Z","cwd":"/Users/dev/projects/beta/.claude/worktrees/foo-123","message":{"id":"msg_ddd","model":"claude-opus-4-8","usage":{"input_tokens":70,"output_tokens":8,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}
  { this is not valid json
  EOF

  printf '%s' '{"type":"assistant","requestId":"req_4","timestamp":"2026-06-02T09:30:00.000Z","cwd":"/Users/dev/projects/beta","message":{"id":"msg_eee","model":"claude-opus-4-8","usage":{"input_tokens":99,"output_tokens":9,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}' >> tests/fixtures/claude/projects/beta/session3.jsonl
  cd ..
  ```

  Fixture map (what each line exercises):
  - `alpha/session1` line 1 `msg_aaa`: explicit `cache_creation` split (5m=4, 1h=6).
  - `alpha/session1` line 2 + `alpha/session2` line 1: the **same** `msg_dup` in two files (session-resume duplicate) → deduped to one row.
  - `alpha/session2` line 2 `msg_bbb`: `cache_creation_input_tokens=80` with **no** `cache_creation` sub-object → all 80 land in 5m.
  - `beta/session3` line 1 `msg_syn`: all-zero `<synthetic>` → skipped, never inserted, not counted as skipped.
  - `beta/session3` line 2 `msg_ccc`: **no** `requestId` → fallback dedup key `claude:msg_ccc`.
  - `beta/session3` line 3 `msg_ddd`: worktree `cwd` → rolled up to `/Users/dev/projects/beta`.
  - `beta/session3` line 4: malformed JSON → `lines_skipped += 1`.
  - `beta/session3` line 5 `msg_eee`: trailing partial line (no newline) → left unconsumed, **not** inserted, **not** counted skipped.

- [ ] **Step 2: Wire the `adapters` module**

  Create `src-tauri/src/adapters/mod.rs` with exactly:

  ```rust
  pub mod claude;
  ```

  Then in `src-tauri/src/lib.rs`, add the line `pub mod adapters;` next to the
  `pub mod types;` and `pub mod db;` declarations added in Task 2 (top of the
  file, module-declaration block).

- [ ] **Step 3: Create `adapters/claude.rs` with the failing test and a stub**

  Create `src-tauri/src/adapters/claude.rs`:

  ```rust
  use crate::db::{get_file_state, insert_events, set_file_state};
  use crate::types::{FileState, SourceScanResult, UsageEvent};
  use rusqlite::Connection;
  use std::path::{Path, PathBuf};

  pub fn scan_claude(_conn: &mut Connection, _projects_root: &Path) -> SourceScanResult {
      SourceScanResult::default()
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::db::open_db;
      use std::io::Write;

      fn fixtures() -> PathBuf {
          Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude/projects")
      }

      #[test]
      fn parses_dedups_splits_and_skips() {
          let dir = tempfile::tempdir().unwrap();
          let mut conn = open_db(&dir.path().join("t.db")).unwrap();
          let res = scan_claude(&mut conn, &fixtures());

          assert_eq!(res.error, None);
          assert_eq!(res.events_inserted, 5);
          assert_eq!(res.lines_skipped, 1);

          let total: i64 = conn
              .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
              .unwrap();
          assert_eq!(total, 5);

          // duplicate message across two files deduped to one row
          let dup: i64 = conn
              .query_row(
                  "SELECT COUNT(*) FROM events WHERE dedup_key = 'claude:msg_dup:req_9'",
                  [],
                  |r| r.get(0),
              )
              .unwrap();
          assert_eq!(dup, 1);

          // explicit 5m/1h split preserved
          let (m5, h1): (i64, i64) = conn
              .query_row(
                  "SELECT cache_write_5m_tokens, cache_write_1h_tokens FROM events WHERE dedup_key = 'claude:msg_aaa:req_1'",
                  [],
                  |r| Ok((r.get(0)?, r.get(1)?)),
              )
              .unwrap();
          assert_eq!((m5, h1), (4, 6));

          // absent cache_creation sub-object => whole creation total in 5m
          let (m5b, h1b): (i64, i64) = conn
              .query_row(
                  "SELECT cache_write_5m_tokens, cache_write_1h_tokens FROM events WHERE dedup_key = 'claude:msg_bbb:req_2'",
                  [],
                  |r| Ok((r.get(0)?, r.get(1)?)),
              )
              .unwrap();
          assert_eq!((m5b, h1b), (80, 0));

          // <synthetic> all-zero line skipped
          let syn: i64 = conn
              .query_row("SELECT COUNT(*) FROM events WHERE model = '<synthetic>'", [], |r| r.get(0))
              .unwrap();
          assert_eq!(syn, 0);

          // worktree cwd rolled up to parent repo
          let proj: String = conn
              .query_row(
                  "SELECT project FROM events WHERE dedup_key = 'claude:msg_ddd:req_3'",
                  [],
                  |r| r.get(0),
              )
              .unwrap();
          assert_eq!(proj, "/Users/dev/projects/beta");

          // missing requestId => fallback dedup key
          let fb: i64 = conn
              .query_row("SELECT COUNT(*) FROM events WHERE dedup_key = 'claude:msg_ccc'", [], |r| r.get(0))
              .unwrap();
          assert_eq!(fb, 1);

          // timestamp parsed from the ISO `timestamp` field (2026-06-01T10:00:00Z)
          let ts: i64 = conn
              .query_row(
                  "SELECT timestamp FROM events WHERE dedup_key = 'claude:msg_aaa:req_1'",
                  [],
                  |r| r.get(0),
              )
              .unwrap();
          assert_eq!(ts, 1780308000);
      }

      #[test]
      fn resumes_after_append_and_ignores_trailing_partial() {
          let dir = tempfile::tempdir().unwrap();
          let mut conn = open_db(&dir.path().join("t.db")).unwrap();
          let proj = dir.path().join("projects/x");
          std::fs::create_dir_all(&proj).unwrap();
          let logp = proj.join("s.jsonl");
          let root = dir.path().join("projects");

          let line1 = r#"{"type":"assistant","requestId":"req_a","timestamp":"2026-06-01T10:00:00.000Z","cwd":"/Users/dev/projects/x","message":{"id":"msg_r1","model":"claude-opus-4-8","usage":{"input_tokens":10,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
          let line2 = r#"{"type":"assistant","requestId":"req_b","timestamp":"2026-06-01T11:00:00.000Z","cwd":"/Users/dev/projects/x","message":{"id":"msg_r2","model":"claude-opus-4-8","usage":{"input_tokens":20,"output_tokens":2,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;

          // first write: line1 complete
          std::fs::write(&logp, format!("{line1}\n")).unwrap();
          let r1 = scan_claude(&mut conn, &root);
          assert_eq!(r1.events_inserted, 1);

          // append line2 WITHOUT a trailing newline -> partial, must be ignored
          {
              let mut f = std::fs::OpenOptions::new().append(true).open(&logp).unwrap();
              write!(f, "{line2}").unwrap();
          }
          let r2 = scan_claude(&mut conn, &root);
          assert_eq!(r2.events_inserted, 0);

          // complete line2 with the newline -> now consumed on resume
          {
              let mut f = std::fs::OpenOptions::new().append(true).open(&logp).unwrap();
              writeln!(f).unwrap();
          }
          let r3 = scan_claude(&mut conn, &root);
          assert_eq!(r3.events_inserted, 1);

          let total: i64 = conn
              .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
              .unwrap();
          assert_eq!(total, 2);
      }
  }
  ```

  Run it — expect a RED failure (stub inserts nothing):

  ```bash
  cargo test --manifest-path src-tauri/Cargo.toml claude:: -- --nocapture
  ```

  Expected: compiles, both tests FAIL, first assertion diff
  `assertion `left == right` failed  left: 0  right: 5` (and the resume test
  fails at `left: 0  right: 1`).

- [ ] **Step 4: Implement `scan_claude`**

  In `src-tauri/src/adapters/claude.rs`, replace the stub function

  ```rust
  pub fn scan_claude(_conn: &mut Connection, _projects_root: &Path) -> SourceScanResult {
      SourceScanResult::default()
  }
  ```

  with the full implementation and its helpers:

  ```rust
  pub fn scan_claude(conn: &mut Connection, projects_root: &Path) -> SourceScanResult {
      let mut result = SourceScanResult::default();
      let mut files = Vec::new();
      find_jsonl(projects_root, &mut files);
      files.sort();
      for path in files {
          if let Err(e) = scan_file(conn, &path, &mut result) {
              result.error = Some(e.to_string());
              return result;
          }
      }
      result
  }

  fn find_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
      let entries = match std::fs::read_dir(dir) {
          Ok(e) => e,
          Err(_) => return, // missing directory => zero events, not an error
      };
      for entry in entries.flatten() {
          let p = entry.path();
          if p.is_dir() {
              find_jsonl(&p, out);
          } else if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
              out.push(p);
          }
      }
  }

  fn scan_file(
      conn: &mut Connection,
      path: &Path,
      result: &mut SourceScanResult,
  ) -> rusqlite::Result<()> {
      use std::io::{Read, Seek, SeekFrom};

      let meta = match std::fs::metadata(path) {
          Ok(m) => m,
          Err(_) => return Ok(()),
      };
      let size = meta.len() as i64;
      let mtime = meta
          .modified()
          .ok()
          .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
          .map(|d| d.as_secs() as i64)
          .unwrap_or(0);
      let path_str = path.to_string_lossy().to_string();

      // Resume from stored offset only when the file has not shrunk; otherwise
      // reparse from the start (idempotent via dedup keys).
      let start = match get_file_state(conn, &path_str)? {
          Some(fs) if size >= fs.size => fs.byte_offset,
          _ => 0,
      };

      let mut file = match std::fs::File::open(path) {
          Ok(f) => f,
          Err(_) => return Ok(()),
      };
      if file.seek(SeekFrom::Start(start as u64)).is_err() {
          return Ok(());
      }
      let mut buf = Vec::new();
      if file.read_to_end(&mut buf).is_err() {
          return Ok(());
      }

      // Consume only complete newline-terminated lines; a trailing partial line
      // is left for the next scan.
      let consumed = buf
          .iter()
          .rposition(|&b| b == b'\n')
          .map(|i| i + 1)
          .unwrap_or(0);

      let mut events = Vec::new();
      for line in buf[..consumed].split(|&b| b == b'\n') {
          if line.is_empty() {
              continue;
          }
          match parse_line(line, &path_str) {
              Ok(Some(ev)) => events.push(ev),
              Ok(None) => {}                          // non-assistant or synthetic: ignore
              Err(()) => result.lines_skipped += 1,   // malformed line
          }
      }

      let inserted = insert_events(conn, &events)?;
      result.events_inserted += inserted;

      let new_offset = start + consumed as i64;
      set_file_state(conn, &path_str, FileState { size, mtime, byte_offset: new_offset })?;
      Ok(())
  }

  fn parse_line(line: &[u8], source_file: &str) -> Result<Option<UsageEvent>, ()> {
      let v: serde_json::Value = serde_json::from_slice(line).map_err(|_| ())?;
      if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
          return Ok(None);
      }
      let msg = &v["message"];
      let usage = &msg["usage"];

      let input = usage["input_tokens"].as_i64().unwrap_or(0);
      let output = usage["output_tokens"].as_i64().unwrap_or(0);
      let cache_read = usage["cache_read_input_tokens"].as_i64().unwrap_or(0);
      let cc_total = usage["cache_creation_input_tokens"].as_i64().unwrap_or(0);
      let cc = &usage["cache_creation"];
      let (cw5m, cw1h) = if cc.is_object() {
          (
              cc["ephemeral_5m_input_tokens"].as_i64().unwrap_or(0),
              cc["ephemeral_1h_input_tokens"].as_i64().unwrap_or(0),
          )
      } else {
          // sub-object absent: whole creation total is 5m-TTL
          (cc_total, 0)
      };

      // <synthetic> error placeholders have all-zero usage: skip, don't count.
      if input == 0 && output == 0 && cache_read == 0 && cw5m == 0 && cw1h == 0 {
          return Ok(None);
      }

      let id = match msg["id"].as_str() {
          Some(s) if !s.is_empty() => s,
          _ => return Ok(None),
      };
      let dedup_key = match v.get("requestId").and_then(|r| r.as_str()) {
          Some(r) => format!("claude:{id}:{r}"),
          None => format!("claude:{id}"),
      };
      let model = msg["model"].as_str().unwrap_or("unknown").to_string();
      let project = v.get("cwd").and_then(|c| c.as_str()).map(rollup_worktree);
      let timestamp = match v.get("timestamp").and_then(|t| t.as_str()).and_then(iso_to_epoch) {
          Some(ts) => ts,
          None => return Ok(None),
      };

      Ok(Some(UsageEvent {
          dedup_key,
          source: "claude".to_string(),
          timestamp,
          model,
          project,
          api_calls: 1,
          input_tokens: input,
          output_tokens: output,
          cache_read_tokens: cache_read,
          cache_write_5m_tokens: cw5m,
          cache_write_1h_tokens: cw1h,
          source_file: source_file.to_string(),
      }))
  }

  fn rollup_worktree(cwd: &str) -> String {
      match cwd.find("/.claude/worktrees/") {
          Some(i) => cwd[..i].to_string(),
          None => cwd.to_string(),
      }
  }

  // Parse "YYYY-MM-DDTHH:MM:SS(.fff)?Z" (always UTC) to epoch seconds.
  // Howard Hinnant's days-from-civil algorithm; avoids a chrono dependency.
  fn iso_to_epoch(s: &str) -> Option<i64> {
      if s.len() < 19 {
          return None;
      }
      let year: i64 = s.get(0..4)?.parse().ok()?;
      let month: i64 = s.get(5..7)?.parse().ok()?;
      let day: i64 = s.get(8..10)?.parse().ok()?;
      let hour: i64 = s.get(11..13)?.parse().ok()?;
      let min: i64 = s.get(14..16)?.parse().ok()?;
      let sec: i64 = s.get(17..19)?.parse().ok()?;

      let y = if month <= 2 { year - 1 } else { year };
      let era = if y >= 0 { y } else { y - 399 } / 400;
      let yoe = y - era * 400; // [0, 399]
      let mp = if month > 2 { month - 3 } else { month + 9 };
      let doy = (153 * mp + 2) / 5 + day - 1; // [0, 365]
      let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
      let days = era * 146097 + doe - 719468; // days since 1970-01-01
      Some(days * 86400 + hour * 3600 + min * 60 + sec)
  }
  ```

  Run — expect GREEN:

  ```bash
  cargo test --manifest-path src-tauri/Cargo.toml claude:: -- --nocapture
  ```

  Expected: `test result: ok. 2 passed; 0 failed`.

- [ ] **Step 5: Commit**

  ```bash
  git add src-tauri/src/adapters/mod.rs src-tauri/src/adapters/claude.rs src-tauri/src/lib.rs src-tauri/tests/fixtures/claude
  git commit -m "feat: add Claude Code source adapter with byte-offset resume"
  ```


---

### Task 4: Codex adapter

Parses `~/.codex/sessions/**/rollout-*.jsonl`. Codex `token_count` events carry
**cumulative** `total_token_usage` snapshots, and duplicate snapshot lines are
common. Per-event tokens = `max(0, delta)` of each field between consecutive
`token_count` events in a file; the first event contributes its full totals, so
duplicate snapshots self-correct to a zero delta and drop out. Cached input is a
subset of input, so `input = Δinput − Δcached` and `cache_read = Δcached`. Model
comes from the last-seen `turn_context.payload.model` (fallback `"unknown"`);
project (`cwd`) from the last-seen `session_meta.payload.cwd`. Changed/new files
are re-parsed in full; dedup keys are byte-offset based so re-parse is idempotent.

**Files:**
- Create: `src-tauri/src/adapters/codex.rs`
- Create: `src-tauri/tests/fixtures/codex/rollout-fixture.jsonl`
- Modify: `src-tauri/src/adapters/mod.rs` (add `pub mod codex;`)
- Modify: `src-tauri/src/lib.rs` (ensure `mod adapters;` is declared — only if absent)
- Test: inline `#[cfg(test)]` module in `src-tauri/src/adapters/codex.rs`

**Interfaces:**
- Consumes (from Task 2):
  ```rust
  // types.rs
  pub struct UsageEvent {
      pub dedup_key: String, pub source: String, pub timestamp: i64,
      pub model: String, pub project: Option<String>, pub api_calls: i64,
      pub input_tokens: i64, pub output_tokens: i64, pub cache_read_tokens: i64,
      pub cache_write_5m_tokens: i64, pub cache_write_1h_tokens: i64,
      pub source_file: String,
  }
  pub struct FileState { pub size: i64, pub mtime: i64, pub byte_offset: i64 }
  #[derive(Debug, Default, Serialize, Clone)]
  #[serde(rename_all = "camelCase")]
  pub struct SourceScanResult { pub events_inserted: u64, pub lines_skipped: u64, pub error: Option<String> }
  // db.rs
  pub fn insert_events(conn: &mut Connection, events: &[UsageEvent]) -> rusqlite::Result<u64>; // INSERT OR IGNORE, count actually inserted
  pub fn get_file_state(conn: &Connection, path: &str) -> rusqlite::Result<Option<FileState>>;
  pub fn set_file_state(conn: &Connection, path: &str, state: FileState) -> rusqlite::Result<()>;
  pub fn open_db(path: &std::path::Path) -> rusqlite::Result<rusqlite::Connection>;
  ```
- Produces (relied on by Task 7):
  ```rust
  pub fn scan_codex(conn: &mut Connection, sessions_root: &Path) -> SourceScanResult;
  ```

---

- [ ] **Step 1: Register the module**

Ensure `src-tauri/src/adapters/mod.rs` declares the codex module. If the file
already exists (Task 3 created it), append the line `pub mod codex;`. If it does
NOT exist yet, create `src-tauri/src/adapters/mod.rs` with exactly:

```rust
pub mod codex;
```

and ensure `src-tauri/src/lib.rs` contains the line `mod adapters;` (add it next
to the existing `mod types;` / `mod db;` declarations only if it is not already
present — do not duplicate it).

- [ ] **Step 2: Write the fixture file**

Create `src-tauri/tests/fixtures/codex/rollout-fixture.jsonl` with exactly these
6 lines (each line is one complete JSON object; no trailing blank line matters).
It contains: a `session_meta` (cwd), a `turn_context` (model `gpt-5.4`), a
`token_count` with `info: null` (must be skipped), a first snapshot, a duplicate
of that snapshot (delta 0 → dropped), and a growing snapshot.

```jsonl
{"timestamp":"2026-04-25T13:03:20.000Z","type":"session_meta","payload":{"session_id":"sess-abc","cwd":"/Users/dev/projects/alpha","cli_version":"0.1.0"}}
{"timestamp":"2026-04-25T13:03:21.000Z","type":"turn_context","payload":{"turn_id":"t1","cwd":"/Users/dev/projects/alpha","model":"gpt-5.4","effort":"medium"}}
{"timestamp":"2026-04-25T13:03:22.000Z","type":"event_msg","payload":{"type":"token_count","info":null}}
{"timestamp":"2026-04-25T13:03:28.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":10,"reasoning_output_tokens":0,"total_tokens":110},"last_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":10,"reasoning_output_tokens":0,"total_tokens":110},"model_context_window":258400}}}
{"timestamp":"2026-04-25T13:03:30.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":10,"reasoning_output_tokens":0,"total_tokens":110},"last_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":10,"reasoning_output_tokens":0,"total_tokens":110},"model_context_window":258400}}}
{"timestamp":"2026-04-25T13:03:35.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":250,"cached_input_tokens":100,"output_tokens":30,"reasoning_output_tokens":0,"total_tokens":280},"last_token_usage":{"input_tokens":150,"cached_input_tokens":60,"output_tokens":20,"reasoning_output_tokens":0,"total_tokens":170},"model_context_window":258400}}}
```

Expected adapter output for this file (used by the test in Step 4):
- 2 events (info:null skipped, duplicate snapshot skipped as zero-delta).
- Event 1 (from snapshot 100/40/10): `input=60` (100−40), `cache_read=40`, `output=10`, `ts=1777122208`.
- Event 2 (Δ = 150/60/20): `input=90` (150−60), `cache_read=60`, `output=20`, `ts=1777122215`.
- Totals: `SUM(input)=150`, `SUM(cache_read)=100`, `SUM(output)=30`.
- Invariant: `SUM(input)+SUM(cache_read)=250` = the file's FINAL `total_token_usage.input_tokens` (NOT the naive sum 100+100+250=450).
- `model="gpt-5.4"`, `project="/Users/dev/projects/alpha"` on both events.

- [ ] **Step 3: Write the failing test**

Add this test module at the end of `src-tauri/src/adapters/codex.rs` (the file
does not exist yet — create it containing ONLY this test module for now; Step 5
adds the implementation above it in the same file):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_db;

    fn fixture_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex")
    }

    #[test]
    fn codex_cumulative_delta_and_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let mut conn = open_db(&tmp.path().join("t.db")).unwrap();

        let r = scan_codex(&mut conn, &fixture_root());
        assert_eq!(r.error, None, "no error expected");
        assert_eq!(r.events_inserted, 2, "info:null + duplicate snapshot dropped");
        assert_eq!(r.lines_skipped, 0, "no malformed lines");

        let (n, si, sc, so): (i64, i64, i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), SUM(input_tokens), SUM(cache_read_tokens), SUM(output_tokens) \
                 FROM events WHERE source='codex'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(n, 2);
        // Adapter total equals the file's FINAL snapshot, not the naive sum.
        assert_eq!(si, 150, "input excludes cached, summed via deltas");
        assert_eq!(sc, 100, "cache_read = cached deltas");
        assert_eq!(so, 30, "output deltas");
        assert_eq!(si + sc, 250, "input+cache_read == final cumulative input_tokens");

        // Model tracked from turn_context; project from session_meta.
        let model: String = conn
            .query_row("SELECT DISTINCT model FROM events WHERE source='codex'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(model, "gpt-5.4");
        let project: String = conn
            .query_row("SELECT DISTINCT project FROM events WHERE source='codex'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(project, "/Users/dev/projects/alpha");

        // Timestamps parsed from the token_count line's ISO field.
        let (min_ts, max_ts): (i64, i64) = conn
            .query_row("SELECT MIN(timestamp), MAX(timestamp) FROM events WHERE source='codex'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(min_ts, 1777122208);
        assert_eq!(max_ts, 1777122215);

        // Re-scan is idempotent: unchanged file inserts nothing, totals stable.
        let r2 = scan_codex(&mut conn, &fixture_root());
        assert_eq!(r2.events_inserted, 0, "unchanged file skipped");
        let n2: i64 = conn
            .query_row("SELECT COUNT(*) FROM events WHERE source='codex'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n2, 2);
    }
}
```

Run it — expect a COMPILE failure because `scan_codex` does not exist yet:

```bash
cd src-tauri && cargo test codex:: 2>&1 | tail -20
```

Expected: `error[E0425]: cannot find function \`scan_codex\` in this scope` (and
`cannot find function \`iso_to_epoch\`-related`/unresolved items). Test does not run.

- [ ] **Step 4: Implement the adapter**

Prepend the implementation to `src-tauri/src/adapters/codex.rs`, ABOVE the
`#[cfg(test)] mod tests` block from Step 3. Final file =
implementation + test module.

```rust
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::Value;

use crate::db;
use crate::types::{FileState, SourceScanResult, UsageEvent};

/// Scan all `*.jsonl` rollout files under `sessions_root` (recursively).
/// Missing directory → zero events, no error.
pub fn scan_codex(conn: &mut Connection, sessions_root: &Path) -> SourceScanResult {
    let mut result = SourceScanResult::default();
    let mut files = Vec::new();
    collect_jsonl(sessions_root, &mut files);
    for path in files {
        match scan_file(conn, &path) {
            Ok((inserted, skipped)) => {
                result.events_inserted += inserted;
                result.lines_skipped += skipped;
            }
            Err(e) => result.error = Some(e),
        }
    }
    result
}

fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return, // missing dir is not an error
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

/// Returns (events_inserted, lines_skipped) for one file.
fn scan_file(conn: &mut Connection, path: &Path) -> Result<(u64, u64), String> {
    let path_str = path.to_string_lossy().to_string();
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let size = meta.len() as i64;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // Unchanged file → skip (full re-parse only on change).
    if let Ok(Some(state)) = db::get_file_state(conn, &path_str) {
        if state.size == size && state.mtime == mtime {
            return Ok((0, 0));
        }
    }

    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let file_stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut events: Vec<UsageEvent> = Vec::new();
    let mut skipped: u64 = 0;
    let mut model = String::from("unknown");
    let mut cwd: Option<String> = None;
    // Previous cumulative snapshot (raw, unclamped).
    let mut prev_input: i64 = 0;
    let mut prev_cached: i64 = 0;
    let mut prev_output: i64 = 0;

    let mut offset: usize = 0;
    for line in content.split_inclusive('\n') {
        let line_offset = offset;
        offset += line.len();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let typ = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match typ {
            "session_meta" => {
                if let Some(c) = v.pointer("/payload/cwd").and_then(|c| c.as_str()) {
                    cwd = Some(c.to_string());
                }
            }
            "turn_context" => {
                if let Some(m) = v.pointer("/payload/model").and_then(|m| m.as_str()) {
                    model = m.to_string();
                }
            }
            "event_msg" => {
                let payload = match v.get("payload") {
                    Some(p) => p,
                    None => continue,
                };
                if payload.get("type").and_then(|t| t.as_str()) != Some("token_count") {
                    continue;
                }
                // Skip info:null control lines.
                let info = match payload.get("info") {
                    Some(i) if !i.is_null() => i,
                    _ => continue,
                };
                let usage = match info.get("total_token_usage") {
                    Some(u) => u,
                    None => continue,
                };
                let cur_input = usage.get("input_tokens").and_then(|x| x.as_i64()).unwrap_or(0);
                let cur_cached = usage
                    .get("cached_input_tokens")
                    .and_then(|x| x.as_i64())
                    .unwrap_or(0);
                let cur_output = usage.get("output_tokens").and_then(|x| x.as_i64()).unwrap_or(0);

                let d_input = (cur_input - prev_input).max(0);
                let d_cached = (cur_cached - prev_cached).max(0);
                let d_output = (cur_output - prev_output).max(0);
                prev_input = cur_input;
                prev_cached = cur_cached;
                prev_output = cur_output;

                // cached is a subset of input; keep them mutually exclusive.
                let input = (d_input - d_cached).max(0);
                let cache_read = d_cached;
                let output = d_output;
                // Duplicate snapshots and degenerate rows produce an all-zero delta.
                if input == 0 && cache_read == 0 && output == 0 {
                    continue;
                }

                let ts = v
                    .get("timestamp")
                    .and_then(|t| t.as_str())
                    .and_then(iso_to_epoch)
                    .unwrap_or(0);

                events.push(UsageEvent {
                    dedup_key: format!("codex:{}:{}", file_stem, line_offset),
                    source: "codex".to_string(),
                    timestamp: ts,
                    model: model.clone(),
                    project: cwd.clone(),
                    api_calls: 1,
                    input_tokens: input,
                    output_tokens: output,
                    cache_read_tokens: cache_read,
                    cache_write_5m_tokens: 0,
                    cache_write_1h_tokens: 0,
                    source_file: path_str.clone(),
                });
            }
            _ => {}
        }
    }

    let inserted = db::insert_events(conn, &events).map_err(|e| e.to_string())?;
    db::set_file_state(
        conn,
        &path_str,
        FileState {
            size,
            mtime,
            byte_offset: size,
        },
    )
    .map_err(|e| e.to_string())?;
    Ok((inserted, skipped))
}

/// Parse an ISO-8601 UTC timestamp (`YYYY-MM-DDThh:mm:ss[.fff]Z`) to epoch
/// seconds. Uses Howard Hinnant's days-from-civil algorithm; fractional
/// seconds and the trailing `Z` are ignored.
fn iso_to_epoch(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |a: &[u8]| -> Option<i64> { std::str::from_utf8(a).ok()?.parse().ok() };
    let year = num(&b[0..4])?;
    let month = num(&b[5..7])?;
    let day = num(&b[8..10])?;
    let hour = num(&b[11..13])?;
    let min = num(&b[14..16])?;
    let sec = num(&b[17..19])?;

    let y = if month <= 2 { year - 1 } else { year };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hour * 3600 + min * 60 + sec)
}
```

- [ ] **Step 5: Run the test — expect PASS**

```bash
cd src-tauri && cargo test codex:: 2>&1 | tail -20
```

Expected: `test adapters::codex::tests::codex_cumulative_delta_and_idempotent ... ok`
and `test result: ok. 1 passed`.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/adapters/codex.rs src-tauri/src/adapters/mod.rs src-tauri/src/lib.rs src-tauri/tests/fixtures/codex/rollout-fixture.jsonl
git commit -m "feat: add Codex adapter with cumulative-snapshot delta parsing"
```


---

### Task 5: Gemini adapter

**Files:**
- Create: `src-tauri/src/adapters/gemini.rs`
- Modify: `src-tauri/src/adapters/mod.rs`, `src-tauri/src/lib.rs`
- Test: `src-tauri/src/adapters/gemini.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes (from Task 2, copied verbatim from skeleton):
  - `pub fn open_db(path: &std::path::Path) -> rusqlite::Result<rusqlite::Connection>;`
  - `pub fn get_file_state(conn: &Connection, path: &str) -> rusqlite::Result<Option<FileState>>;`
  - `pub fn set_file_state(conn: &Connection, path: &str, state: FileState) -> rusqlite::Result<()>;`
  - `pub fn replace_file_events(conn: &mut Connection, source_file: &str, events: &[UsageEvent]) -> rusqlite::Result<()>;`
  - `pub struct FileState { pub size: i64, pub mtime: i64, pub byte_offset: i64 }`
  - `pub struct UsageEvent { pub dedup_key: String, pub source: String, pub timestamp: i64, pub model: String, pub project: Option<String>, pub api_calls: i64, pub input_tokens: i64, pub output_tokens: i64, pub cache_read_tokens: i64, pub cache_write_5m_tokens: i64, pub cache_write_1h_tokens: i64, pub source_file: String }`
  - `pub struct SourceScanResult { pub events_inserted: u64, pub lines_skipped: u64, pub error: Option<String> }` (derives `Default`)
- Produces (relied on by Task 7, copied verbatim from skeleton):
  - `pub fn scan_gemini(conn: &mut Connection, tmp_root: &Path, projects_json: &Path) -> SourceScanResult;`

Rules implemented (from spec "Source adapters → Gemini CLI"):
- Files: `tmp_root/*/chats/session-*.json` (whole-file JSON).
- Per message with a `tokens` object: `input = max(0, tokens.input − tokens.cached)`,
  `output = tokens.output + tokens.thoughts`, `cache_read = tokens.cached`, cache writes `0`.
  Messages without a `tokens` object contribute no event.
- Model = per-message `model` (fallback `"unknown"`). Timestamp = message ISO `timestamp` → epoch seconds UTC.
- Project = the `tmp/` subdirectory name reverse-mapped via `projects.json`
  (`{"projects": {realPath: friendlyName}}` → friendly→real); a subdir name not in the
  map is a hash dir, shown as its first 8 chars.
- Dedup key = `gemini:{sessionId}:{message.id}`.
- Incremental: replace-per-file. Skip a file whose size AND mtime match the stored
  `scanned_files` row; otherwise `DELETE WHERE source_file = ?` + re-insert (via
  `replace_file_events`) and update file state. A missing `tmp_root` → zero events, no error.
  A file that is not valid JSON → `lines_skipped += 1`, file left un-stated (retried next scan).

---

- [ ] **Step 1: Wire the gemini module into the crate**

Run from the repo root (idempotent — safe if a sibling adapter task already added the lines):

```bash
mkdir -p src-tauri/src/adapters
grep -q 'pub mod gemini;' src-tauri/src/adapters/mod.rs 2>/dev/null || printf 'pub mod gemini;\n' >> src-tauri/src/adapters/mod.rs
grep -q 'pub mod adapters;' src-tauri/src/lib.rs || printf 'pub mod adapters;\n' >> src-tauri/src/lib.rs
```

- [ ] **Step 2: Write the failing test file**

Create `src-tauri/src/adapters/gemini.rs` with ONLY the test module below (the
implementation is added in Step 4, above this block). Fixtures are small synthetic
JSON literals built into a `tempdir`; they mirror the real
`~/.gemini/tmp/*/chats/session-*.json` shape (top-level `sessionId` + `messages[]`,
each message `{id, timestamp, type, model, tokens:{input,output,cached,thoughts,tool,total}}`)
and honor the verified invariant `total == input + output + thoughts`.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // session with an info message (no tokens → skipped) and two gemini messages
    const SESSION_ALPHA: &str = r#"{
      "sessionId": "sess-alpha",
      "projectHash": "alpha",
      "startTime": "2026-03-01T10:00:00.000Z",
      "lastUpdated": "2026-03-01T11:30:00.500Z",
      "messages": [
        { "id": "m0", "timestamp": "2026-03-01T10:00:00.000Z", "type": "info",
          "content": "Gemini CLI update available!" },
        { "id": "m1", "timestamp": "2026-03-01T10:05:00.000Z", "type": "gemini",
          "model": "gemini-2.5-flash",
          "tokens": { "input": 1000, "output": 200, "cached": 300, "thoughts": 50, "tool": 0, "total": 1250 } },
        { "id": "m2", "timestamp": "2026-03-01T11:30:00.500Z", "type": "gemini",
          "model": "gemini-2.5-flash",
          "tokens": { "input": 500, "output": 100, "cached": 0, "thoughts": 0, "tool": 0, "total": 600 } }
      ]
    }"#;

    // session under a hash-named dir (not in projects.json → shortened to 8 chars)
    const SESSION_HASH: &str = r#"{
      "sessionId": "sess-beta",
      "projectHash": "abcdef1234567890",
      "startTime": "2026-03-02T09:00:00.000Z",
      "lastUpdated": "2026-03-02T09:00:00.000Z",
      "messages": [
        { "id": "m3", "timestamp": "2026-03-02T09:00:00.000Z", "type": "gemini",
          "model": "gemini-3-pro-preview",
          "tokens": { "input": 800, "output": 400, "cached": 200, "thoughts": 100, "tool": 0, "total": 1300 } }
      ]
    }"#;

    fn write(path: &std::path::Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn test_scan_gemini_extracts_and_maps() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let tmp_root = base.join("tmp");
        let projects_json = base.join("projects.json");

        std::fs::write(
            &projects_json,
            r#"{"projects":{"/Users/dev/projects/alpha":"alpha"}}"#,
        )
        .unwrap();
        write(&tmp_root.join("alpha/chats/session-1.json"), SESSION_ALPHA);
        write(&tmp_root.join("abcdef1234567890/chats/session-2.json"), SESSION_HASH);
        write(&tmp_root.join("alpha/chats/session-bad.json"), "{ not json");

        let mut conn = crate::db::open_db(&base.join("t.db")).unwrap();
        let r = scan_gemini(&mut conn, &tmp_root, &projects_json);
        assert_eq!(r.events_inserted, 3);
        assert_eq!(r.lines_skipped, 1); // the malformed file only
        assert!(r.error.is_none());

        // m1: input excludes cached (1000-300); output includes thoughts (200+50)
        let (input, output, cread, model, project): (i64, i64, i64, String, String) = conn
            .query_row(
                "SELECT input_tokens, output_tokens, cache_read_tokens, model, project \
                 FROM events WHERE dedup_key = 'gemini:sess-alpha:m1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap();
        assert_eq!(input, 700);
        assert_eq!(output, 250);
        assert_eq!(cread, 300);
        assert_eq!(model, "gemini-2.5-flash");
        assert_eq!(project, "/Users/dev/projects/alpha"); // friendly-name reverse map

        // m1 timestamp = epoch of 2026-03-01T10:05:00Z
        let ts: i64 = conn
            .query_row(
                "SELECT timestamp FROM events WHERE dedup_key = 'gemini:sess-alpha:m1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ts, 1772359500);

        // m3: hash dir shortened to first 8 chars; math 800-200 / 400+100
        let (i3, o3, project3): (i64, i64, String) = conn
            .query_row(
                "SELECT input_tokens, output_tokens, project \
                 FROM events WHERE dedup_key = 'gemini:sess-beta:m3'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(i3, 600);
        assert_eq!(o3, 500);
        assert_eq!(project3, "abcdef12");

        // the info message (no tokens) produced no event
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 3);

        // idempotent: unchanged files skipped → 0 new, still 3 total
        let r2 = scan_gemini(&mut conn, &tmp_root, &projects_json);
        assert_eq!(r2.events_inserted, 0);
        let count2: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count2, 3);
    }

    #[test]
    fn test_scan_gemini_replaces_changed_file() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let tmp_root = base.join("tmp");
        let projects_json = base.join("projects.json");
        std::fs::write(&projects_json, r#"{"projects":{}}"#).unwrap();
        let session = tmp_root.join("proj/chats/session-x.json");

        write(
            &session,
            r#"{"sessionId":"sx","messages":[
              {"id":"a","timestamp":"2026-03-01T10:00:00.000Z","type":"gemini","model":"gemini-2.5-flash",
               "tokens":{"input":100,"output":10,"cached":0,"thoughts":0,"tool":0,"total":110}}
            ]}"#,
        );

        let mut conn = crate::db::open_db(&base.join("t.db")).unwrap();
        let r1 = scan_gemini(&mut conn, &tmp_root, &projects_json);
        assert_eq!(r1.events_inserted, 1);

        // rewrite with two different messages (larger size → change detected)
        write(
            &session,
            r#"{"sessionId":"sx","messages":[
              {"id":"b","timestamp":"2026-03-01T10:01:00.000Z","type":"gemini","model":"gemini-2.5-flash",
               "tokens":{"input":200,"output":20,"cached":0,"thoughts":0,"tool":0,"total":220}},
              {"id":"c","timestamp":"2026-03-01T10:02:00.000Z","type":"gemini","model":"gemini-2.5-flash",
               "tokens":{"input":300,"output":30,"cached":0,"thoughts":0,"tool":0,"total":330}}
            ]}"#,
        );

        let r2 = scan_gemini(&mut conn, &tmp_root, &projects_json);
        assert_eq!(r2.events_inserted, 2);

        // old event 'a' was deleted by replace-per-file; only b, c remain
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
        let has_a: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE dedup_key = 'gemini:sx:a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has_a, 0);
    }
}
```

- [ ] **Step 3: Run the test — expect RED**

```bash
cargo test --manifest-path src-tauri/Cargo.toml gemini::
```

Expected: compile error `cannot find function 'scan_gemini' in this scope` (E0425) —
the test references `scan_gemini`, which does not exist yet.

- [ ] **Step 4: Add the implementation**

Insert the following ABOVE the `#[cfg(test)]` line in
`src-tauri/src/adapters/gemini.rs` (so the file is: implementation, then the test module).

```rust
use crate::db::{get_file_state, replace_file_events, set_file_state};
use crate::types::{FileState, SourceScanResult, UsageEvent};
use rusqlite::Connection;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Deserialize)]
struct SessionFile {
    #[serde(rename = "sessionId")]
    session_id: String,
    messages: Vec<Message>,
}

#[derive(Deserialize)]
struct Message {
    id: String,
    timestamp: String,
    model: Option<String>,
    tokens: Option<Tokens>,
}

#[derive(Deserialize)]
struct Tokens {
    input: i64,
    output: i64,
    cached: i64,
    thoughts: i64,
}

pub fn scan_gemini(conn: &mut Connection, tmp_root: &Path, projects_json: &Path) -> SourceScanResult {
    let mut result = SourceScanResult::default();
    let reverse = load_reverse_map(projects_json);

    let subdirs = match fs::read_dir(tmp_root) {
        Ok(rd) => rd,
        Err(_) => return result, // missing dir → zero events, no error
    };
    for sub in subdirs.flatten() {
        let sub_path = sub.path();
        if !sub_path.is_dir() {
            continue;
        }
        let dir_name = match sub_path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let project = resolve_project(&dir_name, &reverse);
        let chats = sub_path.join("chats");
        let entries = match fs::read_dir(&chats) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            if name.starts_with("session-") && name.ends_with(".json") {
                process_file(conn, &path, &project, &mut result);
            }
        }
    }
    result
}

fn process_file(conn: &mut Connection, path: &Path, project: &str, result: &mut SourceScanResult) {
    let path_str = path.to_string_lossy().to_string();
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return,
    };
    let size = meta.len() as i64;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // unchanged (same size AND mtime) → skip whole file
    if let Ok(Some(state)) = get_file_state(conn, &path_str) {
        if state.size == size && state.mtime == mtime {
            return;
        }
    }

    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => {
            result.lines_skipped += 1;
            return;
        }
    };
    let session: SessionFile = match serde_json::from_str(&content) {
        Ok(s) => s,
        Err(_) => {
            result.lines_skipped += 1;
            return;
        }
    };

    let mut events = Vec::new();
    for m in &session.messages {
        let tokens = match &m.tokens {
            Some(t) => t,
            None => continue, // non-token messages contribute nothing
        };
        let ts = match iso_to_epoch(&m.timestamp) {
            Some(t) => t,
            None => {
                result.lines_skipped += 1;
                continue;
            }
        };
        events.push(UsageEvent {
            dedup_key: format!("gemini:{}:{}", session.session_id, m.id),
            source: "gemini".to_string(),
            timestamp: ts,
            model: m.model.clone().unwrap_or_else(|| "unknown".to_string()),
            project: Some(project.to_string()),
            api_calls: 1,
            input_tokens: (tokens.input - tokens.cached).max(0),
            output_tokens: tokens.output + tokens.thoughts,
            cache_read_tokens: tokens.cached,
            cache_write_5m_tokens: 0,
            cache_write_1h_tokens: 0,
            source_file: path_str.clone(),
        });
    }

    let n = events.len() as u64;
    if replace_file_events(conn, &path_str, &events).is_err() {
        result.error = Some(format!("failed to write events for {}", path_str));
        return;
    }
    result.events_inserted += n;
    let _ = set_file_state(conn, &path_str, FileState { size, mtime, byte_offset: 0 });
}

/// projects.json is `{"projects": {realPath: friendlyName}}`; build friendly → real.
fn load_reverse_map(projects_json: &Path) -> HashMap<String, String> {
    #[derive(Deserialize)]
    struct Projects {
        projects: HashMap<String, String>,
    }
    let mut map = HashMap::new();
    if let Ok(content) = fs::read_to_string(projects_json) {
        if let Ok(p) = serde_json::from_str::<Projects>(&content) {
            for (real, friendly) in p.projects {
                map.insert(friendly, real);
            }
        }
    }
    map
}

fn resolve_project(dir_name: &str, reverse: &HashMap<String, String>) -> String {
    match reverse.get(dir_name) {
        Some(real) => real.clone(),
        None => dir_name.chars().take(8).collect(), // hash dir → shortened hash
    }
}

/// Parse "YYYY-MM-DDTHH:MM:SS[.fff][Z]" → epoch seconds UTC (Howard Hinnant
/// days-from-civil; no chrono dependency). Verified against Python for
/// 1970-01-01, 2000-02-29, and the fixture dates.
fn iso_to_epoch(s: &str) -> Option<i64> {
    if s.len() < 19 {
        return None;
    }
    let num = |a: usize, z: usize| -> Option<i64> { s.get(a..z)?.parse::<i64>().ok() };
    let year = num(0, 4)?;
    let month = num(5, 7)?;
    let day = num(8, 10)?;
    let hour = num(11, 13)?;
    let min = num(14, 16)?;
    let sec = num(17, 19)?;

    let y = if month <= 2 { year - 1 } else { year };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hour * 3600 + min * 60 + sec)
}
```

- [ ] **Step 5: Run the test — expect GREEN**

```bash
cargo test --manifest-path src-tauri/Cargo.toml gemini::
```

Expected: `test result: ok. 2 passed; 0 failed` (`test_scan_gemini_extracts_and_maps`,
`test_scan_gemini_replaces_changed_file`).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/adapters/gemini.rs src-tauri/src/adapters/mod.rs src-tauri/src/lib.rs
git commit -m "feat: add Gemini CLI adapter"
```


---

### Task 6: Hermes adapter

**Files:**
- Create: `src-tauri/src/adapters/hermes.rs`
- Modify: `src-tauri/src/adapters/mod.rs`
- Test: `src-tauri/src/adapters/hermes.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes (from Task 2):
  ```rust
  // types.rs
  pub struct UsageEvent {
      pub dedup_key: String, pub source: String, pub timestamp: i64,
      pub model: String, pub project: Option<String>, pub api_calls: i64,
      pub input_tokens: i64, pub output_tokens: i64, pub cache_read_tokens: i64,
      pub cache_write_5m_tokens: i64, pub cache_write_1h_tokens: i64,
      pub source_file: String,
  }
  #[derive(Debug, Default, Serialize, Clone)]
  #[serde(rename_all = "camelCase")]
  pub struct SourceScanResult { pub events_inserted: u64, pub lines_skipped: u64, pub error: Option<String> }
  // db.rs
  pub fn open_db(path: &std::path::Path) -> rusqlite::Result<rusqlite::Connection>;
  pub fn upsert_events(conn: &mut Connection, events: &[UsageEvent]) -> rusqlite::Result<()>; // INSERT OR REPLACE
  ```
- Produces (relied on by Task 7):
  ```rust
  pub fn scan_hermes(conn: &mut Connection, hermes_db: &Path) -> SourceScanResult;
  ```

Adapter rules (from spec §"Hermes"): open `~/.hermes/state.db` **read-only**
(`?mode=ro`) with a busy timeout; one event per `sessions` row that has any
tokens > 0 OR `api_call_count` > 0; `reasoning_tokens` folds into
`output_tokens`; `cache_write_tokens` → `cache_write_5m` (1h = 0);
`api_calls = api_call_count`, forced to a minimum of 1 when tokens > 0;
`project = cwd` when non-empty else NULL; `timestamp = started_at` (REAL epoch
seconds) truncated to whole seconds; dedup key `hermes:{id}`, **upserted** so
live/growing session rows update in place; on open/lock failure return the
error in `SourceScanResult` and keep already-ingested events untouched.

> Real-schema check (read-only, done during planning): `sessions` columns read
> by this adapter are `id, model, started_at (REAL NOT NULL), input_tokens,
> output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens,
> api_call_count, cwd`. Sample row: `('20260601_141437_f2c17b','qwen3.6-35b-mtp',
> 1780287300.21103, 64728, 5088, 1394761, 0, 0, 30, '')`.

---

- [ ] **Step 1: Register the module**

In `src-tauri/src/adapters/mod.rs`, ensure this line is present (add it if it
is not already there):

```rust
pub mod hermes;
```

- [ ] **Step 2: Write the failing test file (implementation stubbed out)**

Create `src-tauri/src/adapters/hermes.rs` with the imports, an
implementation placeholder, and the complete test module:

```rust
// TokenLedger — Hermes adapter.
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use crate::db::upsert_events;
use crate::types::{SourceScanResult, UsageEvent};

// __HERMES_IMPL__ (replaced in Step 4)

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_db;
    use rusqlite::Connection;
    use std::path::Path;
    use tempfile::tempdir;

    /// Build a minimal Hermes-schema sqlite DB (subset of columns the adapter reads).
    fn build_hermes_db(path: &Path) {
        let src = Connection::open(path).unwrap();
        src.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                model TEXT,
                started_at REAL NOT NULL,
                input_tokens INTEGER,
                output_tokens INTEGER,
                cache_read_tokens INTEGER,
                cache_write_tokens INTEGER,
                reasoning_tokens INTEGER,
                api_call_count INTEGER,
                cwd TEXT
            );",
        )
        .unwrap();
        // s1: full row — reasoning + cache_write + cwd populated, fractional started_at.
        src.execute(
            "INSERT INTO sessions VALUES
             ('s1','qwen3.6-35b',1780287300.21103,64728,5088,1394761,100,50,30,'/Users/dev/projects/alpha')",
            [],
        )
        .unwrap();
        // s2: all-zero tokens and zero api calls -> skipped.
        src.execute(
            "INSERT INTO sessions VALUES
             ('s2','qwen3.6-35b',1780289247.5,0,0,0,0,0,0,'')",
            [],
        )
        .unwrap();
        // s3: tokens>0 but api_call_count 0 -> api_calls forced to 1; empty cwd -> project NULL.
        src.execute(
            "INSERT INTO sessions VALUES
             ('s3','qwen-35b',1780310783.7583,905075,8094,0,0,0,0,'')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn extracts_and_normalizes_sessions() {
        let hermes_dir = tempdir().unwrap();
        let hermes_db = hermes_dir.path().join("state.db");
        build_hermes_db(&hermes_db);

        let app_dir = tempdir().unwrap();
        let mut conn = open_db(&app_dir.path().join("tokenledger.db")).unwrap();

        let res = scan_hermes(&mut conn, &hermes_db);
        assert!(res.error.is_none(), "unexpected error: {:?}", res.error);
        assert_eq!(res.events_inserted, 2); // s1 + s3; s2 skipped
        assert_eq!(res.lines_skipped, 1);   // s2

        // s1: reasoning folded into output, cache_write -> 5m bucket, ts truncated, cwd kept.
        let (model, ts, input, output, cr, cw5, cw1, calls, project): (
            String, i64, i64, i64, i64, i64, i64, i64, Option<String>,
        ) = conn
            .query_row(
                "SELECT model, timestamp, input_tokens, output_tokens, cache_read_tokens,
                        cache_write_5m_tokens, cache_write_1h_tokens, api_calls, project
                 FROM events WHERE dedup_key = 'hermes:s1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?,
                        r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?)),
            )
            .unwrap();
        assert_eq!(model, "qwen3.6-35b");
        assert_eq!(ts, 1780287300);        // 1780287300.21103 truncated to whole seconds
        assert_eq!(input, 64728);
        assert_eq!(output, 5088 + 50);     // reasoning_tokens folded into output
        assert_eq!(cr, 1394761);
        assert_eq!(cw5, 100);              // cache_write_tokens -> 5m bucket
        assert_eq!(cw1, 0);
        assert_eq!(calls, 30);             // api_call_count verbatim
        assert_eq!(project, Some("/Users/dev/projects/alpha".to_string()));

        // s3: api_calls forced to 1; empty cwd -> NULL project.
        let (calls3, project3): (i64, Option<String>) = conn
            .query_row(
                "SELECT api_calls, project FROM events WHERE dedup_key = 'hermes:s3'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(calls3, 1);
        assert_eq!(project3, None);
    }

    #[test]
    fn missing_db_reports_error_keeps_events() {
        let app_dir = tempdir().unwrap();
        let mut conn = open_db(&app_dir.path().join("tokenledger.db")).unwrap();

        let res = scan_hermes(&mut conn, Path::new("/nonexistent/hermes/state.db"));
        assert!(res.error.is_some());
        assert_eq!(res.events_inserted, 0);
    }

    #[test]
    fn upsert_grows_live_rows() {
        let hermes_dir = tempdir().unwrap();
        let hermes_db = hermes_dir.path().join("state.db");
        build_hermes_db(&hermes_db);

        let app_dir = tempdir().unwrap();
        let mut conn = open_db(&app_dir.path().join("tokenledger.db")).unwrap();

        scan_hermes(&mut conn, &hermes_db);

        // Simulate a live session growing: s1 gains output tokens.
        {
            let src = Connection::open(&hermes_db).unwrap();
            src.execute("UPDATE sessions SET output_tokens = 9000 WHERE id = 's1'", [])
                .unwrap();
        }

        let res = scan_hermes(&mut conn, &hermes_db);
        assert!(res.error.is_none());

        let output: i64 = conn
            .query_row(
                "SELECT output_tokens FROM events WHERE dedup_key = 'hermes:s1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(output, 9000 + 50); // upsert replaced the row; reasoning still folded in
    }
}
```

- [ ] **Step 3: Run the test — expect a COMPILE failure**

```bash
cargo test hermes -- --nocapture
```

Expected: build fails with `cannot find function `scan_hermes` in this scope`
(the placeholder comment has no `scan_hermes` yet). This confirms the tests
actually exercise the function.

- [ ] **Step 4: Implement `scan_hermes`**

In `src-tauri/src/adapters/hermes.rs`, replace the placeholder line
`// __HERMES_IMPL__ (replaced in Step 4)` with the complete implementation:

```rust
pub fn scan_hermes(conn: &mut Connection, hermes_db: &Path) -> SourceScanResult {
    // Open the Hermes ledger read-only so we never lock out its live writer.
    let uri = format!("file:{}?mode=ro", hermes_db.display());
    let ro = match Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    ) {
        Ok(c) => c,
        Err(e) => {
            // Lock/open failure: keep prior events, report staleness.
            return SourceScanResult {
                error: Some(format!("hermes: open failed: {e}")),
                ..Default::default()
            };
        }
    };
    let _ = ro.busy_timeout(Duration::from_millis(5000));

    let mut stmt = match ro.prepare(
        "SELECT id, model, started_at, \
                input_tokens, output_tokens, cache_read_tokens, \
                cache_write_tokens, reasoning_tokens, api_call_count, cwd \
         FROM sessions",
    ) {
        Ok(s) => s,
        Err(e) => {
            return SourceScanResult {
                error: Some(format!("hermes: query failed: {e}")),
                ..Default::default()
            };
        }
    };

    let rows = match stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,                   // id
            r.get::<_, Option<String>>(1)?,           // model
            r.get::<_, f64>(2)?,                       // started_at (REAL epoch secs)
            r.get::<_, Option<i64>>(3)?.unwrap_or(0),  // input_tokens
            r.get::<_, Option<i64>>(4)?.unwrap_or(0),  // output_tokens
            r.get::<_, Option<i64>>(5)?.unwrap_or(0),  // cache_read_tokens
            r.get::<_, Option<i64>>(6)?.unwrap_or(0),  // cache_write_tokens
            r.get::<_, Option<i64>>(7)?.unwrap_or(0),  // reasoning_tokens
            r.get::<_, Option<i64>>(8)?.unwrap_or(0),  // api_call_count
            r.get::<_, Option<String>>(9)?,           // cwd
        ))
    }) {
        Ok(r) => r,
        Err(e) => {
            return SourceScanResult {
                error: Some(format!("hermes: read failed: {e}")),
                ..Default::default()
            };
        }
    };

    let mut events: Vec<UsageEvent> = Vec::new();
    let mut skipped: u64 = 0;
    for row in rows {
        let (id, model, started_at, input, output, cache_read, cache_write, reasoning, api_call_count, cwd) =
            match row {
                Ok(v) => v,
                Err(_) => {
                    skipped += 1;
                    continue;
                }
            };

        let total = input + output + cache_read + cache_write + reasoning;
        if total <= 0 && api_call_count <= 0 {
            skipped += 1; // no tokens and no API calls — nothing to record
            continue;
        }

        // api_call_count is authoritative; force at least 1 when tokens exist.
        let api_calls = if api_call_count > 0 { api_call_count } else { 1 };

        let project = match cwd {
            Some(p) if !p.is_empty() => Some(p),
            _ => None,
        };

        events.push(UsageEvent {
            dedup_key: format!("hermes:{id}"),
            source: "hermes".to_string(),
            timestamp: started_at as i64,          // truncate fractional seconds
            model: model.unwrap_or_else(|| "unknown".to_string()),
            project,
            api_calls,
            input_tokens: input,
            output_tokens: output + reasoning,      // reasoning folds into output
            cache_read_tokens: cache_read,
            cache_write_5m_tokens: cache_write,     // single Hermes bucket -> 5m
            cache_write_1h_tokens: 0,
            source_file: hermes_db.display().to_string(),
        });
    }

    let inserted = events.len() as u64;
    if let Err(e) = upsert_events(conn, &events) {
        return SourceScanResult {
            error: Some(format!("hermes: upsert failed: {e}")),
            ..Default::default()
        };
    }

    SourceScanResult { events_inserted: inserted, lines_skipped: skipped, error: None }
}
```

- [ ] **Step 5: Run the test — expect PASS**

```bash
cargo test hermes -- --nocapture
```

Expected: `test result: ok. 3 passed; 0 failed` for
`extracts_and_normalizes_sessions`, `missing_db_reports_error_keeps_events`,
and `upsert_grows_live_rows`.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/adapters/hermes.rs src-tauri/src/adapters/mod.rs
git commit -m "feat: add Hermes adapter (read-only sqlite session ingest)"
```


---

### Task 7: scan.rs orchestrator

**Files:**
- Create: `src-tauri/src/scan.rs`
- Modify: `src-tauri/src/lib.rs` (register `pub mod scan;`)
- Test: `src-tauri/src/scan.rs` (inline `#[cfg(test)]` module)

**Interfaces:**
- Consumes (from Tasks 2–6, exact signatures — do not redefine):
  ```rust
  // types.rs
  pub struct SourceScanResult { pub events_inserted: u64, pub lines_skipped: u64, pub error: Option<String> }
  pub struct SourceStatus { pub source: String, pub events_inserted: u64, pub lines_skipped: u64, pub error: Option<String> }
  pub struct ScanStatus { pub sources: Vec<SourceStatus>, pub scanned_at: i64 }
  // db.rs
  pub fn open_db(path: &std::path::Path) -> rusqlite::Result<rusqlite::Connection>;
  pub fn prune_missing_files(conn: &Connection) -> rusqlite::Result<u64>;
  // adapters/*.rs
  pub fn scan_claude(conn: &mut Connection, projects_root: &Path) -> SourceScanResult;
  pub fn scan_codex(conn: &mut Connection, sessions_root: &Path) -> SourceScanResult;
  pub fn scan_gemini(conn: &mut Connection, tmp_root: &Path, projects_json: &Path) -> SourceScanResult;
  pub fn scan_hermes(conn: &mut Connection, hermes_db: &Path) -> SourceScanResult;
  ```
- Produces (later Tasks 10 relies on these — exact names):
  ```rust
  pub struct SourceRoots { pub claude: PathBuf, pub codex: PathBuf, pub gemini_tmp: PathBuf, pub gemini_projects_json: PathBuf, pub hermes_db: PathBuf }
  impl SourceRoots { pub fn default_roots() -> Self; }
  pub fn run_scan(conn: &mut Connection, roots: &SourceRoots) -> ScanStatus;
  ```

---

- [ ] **Step 1: Register the module and write the failing test**

  Add `pub mod scan;` to `src-tauri/src/lib.rs`, placing it beside the existing
  module declarations (`pub mod types;`, `pub mod db;`, `pub mod adapters;`)
  added by prior tasks:

  ```rust
  pub mod scan;
  ```

  Create `src-tauri/src/scan.rs` with ONLY the test module (the implementation
  lands in Step 2, so the crate will fail to compile — that is the expected
  failure):

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::db::open_db;
      use std::fs;
      use std::path::PathBuf;

      // Minimal Claude assistant line (non-zero usage → one event ingested).
      // Shape matches real ~/.claude/projects/**/*.jsonl assistant records.
      const CLAUDE_LINE: &str = r#"{"type":"assistant","requestId":"req_test1","timestamp":"2026-07-01T10:00:00.000Z","cwd":"/Users/dev/projects/alpha","message":{"id":"msg_test1","model":"claude-opus-4-8","usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":10,"cache_creation_input_tokens":20,"cache_creation":{"ephemeral_5m_input_tokens":20,"ephemeral_1h_input_tokens":0}}}}"#;

      fn find<'a>(status: &'a ScanStatus, source: &str) -> &'a SourceStatus {
          status
              .sources
              .iter()
              .find(|s| s.source == source)
              .unwrap_or_else(|| panic!("missing source {source}"))
      }

      #[test]
      fn default_roots_live_under_home() {
          let r = SourceRoots::default_roots();
          assert!(r.claude.ends_with(".claude/projects"));
          assert!(r.codex.ends_with(".codex/sessions"));
          assert!(r.gemini_tmp.ends_with(".gemini/tmp"));
          assert!(r.gemini_projects_json.ends_with(".gemini/projects.json"));
          assert!(r.hermes_db.ends_with(".hermes/state.db"));
      }

      #[test]
      fn run_scan_isolates_sources() {
          let tmp = tempfile::tempdir().unwrap();
          let base: PathBuf = tmp.path().to_path_buf();

          // Real Claude fixture: one project dir with one usage-bearing line.
          let claude_root = base.join("claude");
          fs::create_dir_all(claude_root.join("proj1")).unwrap();
          fs::write(claude_root.join("proj1").join("session.jsonl"), CLAUDE_LINE).unwrap();

          // Everything else points at paths that do not exist.
          let roots = SourceRoots {
              claude: claude_root,
              codex: base.join("no-codex"),
              gemini_tmp: base.join("no-gemini"),
              gemini_projects_json: base.join("no-projects.json"),
              hermes_db: base.join("no-hermes.db"),
          };

          let mut conn = open_db(&base.join("ledger.db")).unwrap();
          let status = run_scan(&mut conn, &roots);

          assert_eq!(status.sources.len(), 4);
          assert!(status.scanned_at > 0);

          // Claude still ingests its event even though hermes errors.
          let claude = find(&status, "claude");
          assert_eq!(claude.events_inserted, 1);
          assert!(claude.error.is_none());

          // Missing directories → zero events, no error.
          let codex = find(&status, "codex");
          assert_eq!(codex.events_inserted, 0);
          assert!(codex.error.is_none());
          let gemini = find(&status, "gemini");
          assert_eq!(gemini.events_inserted, 0);
          assert!(gemini.error.is_none());

          // Nonexistent hermes DB → error string set; other sources unaffected.
          let hermes = find(&status, "hermes");
          assert!(hermes.error.is_some());
      }
  }
  ```

- [ ] **Step 2: Run the test — expect a compile failure**

  ```bash
  cd src-tauri && cargo test scan:: -- --nocapture
  ```

  Expected: FAILS to compile with `cannot find function` / `cannot find type`
  errors — e.g. `cannot find function 'run_scan' in this scope` and
  `cannot find type 'SourceRoots' in this scope`.

- [ ] **Step 3: Implement `scan.rs`**

  Prepend the implementation to `src-tauri/src/scan.rs`, above the existing
  `#[cfg(test)] mod tests` block:

  ```rust
  use std::panic::{catch_unwind, AssertUnwindSafe};
  use std::path::PathBuf;
  use std::time::{SystemTime, UNIX_EPOCH};

  use rusqlite::Connection;

  use crate::adapters::claude::scan_claude;
  use crate::adapters::codex::scan_codex;
  use crate::adapters::gemini::scan_gemini;
  use crate::adapters::hermes::scan_hermes;
  use crate::db::prune_missing_files;
  use crate::types::{ScanStatus, SourceScanResult, SourceStatus};

  pub struct SourceRoots {
      pub claude: PathBuf,
      pub codex: PathBuf,
      pub gemini_tmp: PathBuf,
      pub gemini_projects_json: PathBuf,
      pub hermes_db: PathBuf,
  }

  impl SourceRoots {
      pub fn default_roots() -> Self {
          let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
          SourceRoots {
              claude: home.join(".claude/projects"),
              codex: home.join(".codex/sessions"),
              gemini_tmp: home.join(".gemini/tmp"),
              gemini_projects_json: home.join(".gemini/projects.json"),
              hermes_db: home.join(".hermes/state.db"),
          }
      }
  }

  // Runs one adapter, converting a panic into a SourceStatus error so the
  // remaining sources still run. Non-panic errors already arrive as
  // SourceScanResult.error and pass straight through.
  fn run_one(source: &str, f: impl FnOnce() -> SourceScanResult) -> SourceStatus {
      match catch_unwind(AssertUnwindSafe(f)) {
          Ok(r) => SourceStatus {
              source: source.to_string(),
              events_inserted: r.events_inserted,
              lines_skipped: r.lines_skipped,
              error: r.error,
          },
          Err(_) => SourceStatus {
              source: source.to_string(),
              events_inserted: 0,
              lines_skipped: 0,
              error: Some("adapter panicked".to_string()),
          },
      }
  }

  pub fn run_scan(conn: &mut Connection, roots: &SourceRoots) -> ScanStatus {
      let mut sources = Vec::with_capacity(4);
      sources.push(run_one("claude", || scan_claude(conn, &roots.claude)));
      sources.push(run_one("codex", || scan_codex(conn, &roots.codex)));
      sources.push(run_one("gemini", || {
          scan_gemini(conn, &roots.gemini_tmp, &roots.gemini_projects_json)
      }));
      sources.push(run_one("hermes", || scan_hermes(conn, &roots.hermes_db)));

      // Ledger hygiene only: drops scanned_files rows for vanished paths.
      // Never deletes events (see prune_missing_files contract). Best-effort.
      let _ = prune_missing_files(conn);

      let scanned_at = SystemTime::now()
          .duration_since(UNIX_EPOCH)
          .map(|d| d.as_secs() as i64)
          .unwrap_or(0);

      ScanStatus { sources, scanned_at }
  }
  ```

- [ ] **Step 4: Run the test — expect green**

  ```bash
  cd src-tauri && cargo test scan:: -- --nocapture
  ```

  Expected: PASSES — `test result: ok. 2 passed; 0 failed`
  (`default_roots_live_under_home` and `run_scan_isolates_sources`).

- [ ] **Step 5: Commit**

  ```bash
  git add src-tauri/src/scan.rs src-tauri/src/lib.rs
  git commit -m "feat: add scan orchestrator (SourceRoots, run_scan)"
  ```


---

### Task 8: pricing.rs

**Files:**
- Create: `src-tauri/src/pricing.rs`
- Create: `src-tauri/resources/model_prices.json` (LiteLLM snapshot, via curl)
- Modify: `src-tauri/src/lib.rs` (declare `pub mod pricing;`)
- Test: `src-tauri/src/pricing.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes (from Task 2, db.rs):
  ```rust
  pub fn open_db(path: &std::path::Path) -> rusqlite::Result<rusqlite::Connection>; // WAL, busy_timeout, migrate
  ```
  and the `prices` / `price_overrides` tables created by `db::migrate`:
  ```sql
  prices(model TEXT PRIMARY KEY, input_per_tok REAL, output_per_tok REAL,
    cache_read_per_tok REAL, cache_write_5m_per_tok REAL, cache_write_1h_per_tok REAL);
  price_overrides(model TEXT PRIMARY KEY, input_per_tok REAL, output_per_tok REAL,
    cache_read_per_tok REAL, cache_write_per_tok REAL);
  ```
- Produces (relied on by Task 9 queries.rs and Task 10 lib.rs):
  ```rust
  pub fn normalize_model(raw: &str) -> String; // lowercase; strip through last '/'; strip trailing -\d{8}
  #[derive(Debug, Clone, Copy, Default, PartialEq)]
  pub struct Rates { pub input: f64, pub output: f64, pub cache_read: f64, pub cache_write_5m: f64, pub cache_write_1h: f64 } // per token
  pub fn rebuild_prices(conn: &mut Connection, litellm_json: &str) -> Result<u64, String>;
  pub fn refresh_prices(conn: &mut Connection, cache_dir: &Path) -> Result<u64, String>;
  #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
  #[serde(rename_all = "camelCase")]
  pub struct OverrideRates { pub input: Option<f64>, pub output: Option<f64>, pub cache_read: Option<f64>, pub cache_write: Option<f64> }
  pub fn set_override(conn: &Connection, model: &str, rates: OverrideRates) -> rusqlite::Result<()>;
  pub fn delete_override(conn: &Connection, model: &str) -> rusqlite::Result<()>;
  pub struct RateMap { /* private */ }
  impl RateMap {
      pub fn load(conn: &Connection) -> rusqlite::Result<RateMap>;
      pub fn resolve(&self, raw_model: &str) -> Option<Rates>; // override -> exact -> normalized; None = unpriced
  }
  ```

All commands below run from `src-tauri/` unless noted.

- [ ] **Step 1: Register the module in lib.rs.**
  Add the module declaration to `src-tauri/src/lib.rs` alongside the other `pub mod` lines (add it if this is the first module Task added; it is idempotent to have it once):
  ```rust
  pub mod pricing;
  ```

- [ ] **Step 2: Write the failing test file.**
  Create `src-tauri/src/pricing.rs` with ONLY the test module (implementation added in Step 5). This references functions that do not exist yet, so it must fail to compile:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      // ~8-entry LiteLLM slice. Field names verified against the real
      // model_prices_and_context_window.json. `sample_spec` has string
      // costs (skipped via as_f64), `chatgpt/gpt-5.4` is all-null (skipped),
      // `replicate/.../gemini-2.5-flash` is a non-canonical reseller collision.
      const FIXTURE: &str = r#"{
        "sample_spec": {
          "input_cost_per_token": "float",
          "output_cost_per_token": "float",
          "litellm_provider": "example"
        },
        "gpt-5.4": {
          "input_cost_per_token": 2.5e-06,
          "output_cost_per_token": 1e-05,
          "cache_read_input_token_cost": 2.5e-07,
          "litellm_provider": "openai"
        },
        "chatgpt/gpt-5.4": {
          "input_cost_per_token": null,
          "output_cost_per_token": null,
          "litellm_provider": "openai"
        },
        "claude-sonnet-4-5": {
          "input_cost_per_token": 3e-06,
          "output_cost_per_token": 1.5e-05,
          "cache_read_input_token_cost": 3e-07,
          "cache_creation_input_token_cost": 3.75e-06,
          "cache_creation_input_token_cost_above_1hr": 6e-06,
          "litellm_provider": "anthropic"
        },
        "gemini-2.5-flash": {
          "input_cost_per_token": 3e-07,
          "output_cost_per_token": 2.5e-06,
          "cache_read_input_token_cost": 3e-08,
          "litellm_provider": "vertex_ai-language-models"
        },
        "replicate/meta/gemini-2.5-flash": {
          "input_cost_per_token": 2.5e-06,
          "output_cost_per_token": 2.5e-06,
          "litellm_provider": "replicate"
        },
        "claude-3-5-sonnet-20241022": {
          "input_cost_per_token": 3e-06,
          "output_cost_per_token": 1.5e-05,
          "cache_creation_input_token_cost": 3.75e-06,
          "litellm_provider": "anthropic"
        }
      }"#;

      fn test_conn() -> (tempfile::TempDir, Connection) {
          let dir = tempfile::tempdir().unwrap();
          let conn = crate::db::open_db(&dir.path().join("test.db")).unwrap();
          (dir, conn)
      }

      #[test]
      fn normalize_strips_slash_and_date_suffix() {
          assert_eq!(normalize_model("GPT-5.4"), "gpt-5.4");
          assert_eq!(normalize_model("anthropic/claude-3-5-sonnet-20241022"), "claude-3-5-sonnet");
          assert_eq!(normalize_model("claude-sonnet-4-5"), "claude-sonnet-4-5"); // -4-5 is not -\d{8}
          assert_eq!(normalize_model("replicate/meta/gemini-2.5-flash"), "gemini-2.5-flash");
      }

      #[test]
      fn rebuild_counts_distinct_rows() {
          let (_d, mut conn) = test_conn();
          // 5 exact rows + 4 normalized keys, unioned = 6 distinct model rows.
          let n = rebuild_prices(&mut conn, FIXTURE).unwrap();
          assert_eq!(n, 6);
      }

      #[test]
      fn exact_wins_and_null_reseller_does_not_pollute() {
          let (_d, mut conn) = test_conn();
          rebuild_prices(&mut conn, FIXTURE).unwrap();
          let rm = RateMap::load(&conn).unwrap();
          // gpt-5.4 exact hit.
          assert_eq!(rm.resolve("gpt-5.4").unwrap().input, 2.5e-06);
          // The all-null chatgpt/gpt-5.4 was skipped, so it created no null
          // normalized row; it resolves to the canonical gpt-5.4 price.
          assert_eq!(rm.resolve("chatgpt/gpt-5.4").unwrap().input, 2.5e-06);
      }

      #[test]
      fn canonical_wins_normalized_collision() {
          let (_d, mut conn) = test_conn();
          rebuild_prices(&mut conn, FIXTURE).unwrap();
          let rm = RateMap::load(&conn).unwrap();
          // Not an exact key -> normalized to gemini-2.5-flash; canonical 3e-07
          // must win over the 2.5e-06 reseller.
          assert_eq!(rm.resolve("gemini-2.5-flash-20250101").unwrap().input, 3e-07);
      }

      #[test]
      fn claude_cache_rates_and_1h_fallback() {
          let (_d, mut conn) = test_conn();
          rebuild_prices(&mut conn, FIXTURE).unwrap();
          let rm = RateMap::load(&conn).unwrap();
          let r = rm.resolve("claude-sonnet-4-5").unwrap();
          assert_eq!(r.cache_read, 3e-07);
          assert_eq!(r.cache_write_5m, 3.75e-06);
          assert_eq!(r.cache_write_1h, 6e-06);
          // claude-3-5-sonnet-20241022 has 5m cost but no above_1hr -> 1h falls back to 5m.
          let f = rm.resolve("claude-3-5-sonnet-20241022").unwrap();
          assert_eq!(f.cache_write_5m, 3.75e-06);
          assert_eq!(f.cache_write_1h, 3.75e-06);
      }

      #[test]
      fn unknown_model_is_none() {
          let (_d, mut conn) = test_conn();
          rebuild_prices(&mut conn, FIXTURE).unwrap();
          let rm = RateMap::load(&conn).unwrap();
          assert_eq!(rm.resolve("totally-unknown-model"), None);
      }

      #[test]
      fn override_wins_fills_none_and_applies_cache_write_both_ttls() {
          let (_d, mut conn) = test_conn();
          rebuild_prices(&mut conn, FIXTURE).unwrap();
          set_override(
              &conn,
              "gemini-2.5-flash",
              OverrideRates { input: Some(9e-06), output: None, cache_read: None, cache_write: Some(1e-06) },
          )
          .unwrap();
          let rm = RateMap::load(&conn).unwrap();
          let r = rm.resolve("gemini-2.5-flash").unwrap();
          assert_eq!(r.input, 9e-06);          // override beats LiteLLM 3e-07
          assert_eq!(r.output, 0.0);           // None -> 0
          assert_eq!(r.cache_read, 0.0);       // None -> 0
          assert_eq!(r.cache_write_5m, 1e-06); // cache_write on both TTLs
          assert_eq!(r.cache_write_1h, 1e-06);
          // Delete restores the LiteLLM price.
          delete_override(&conn, "gemini-2.5-flash").unwrap();
          let rm2 = RateMap::load(&conn).unwrap();
          assert_eq!(rm2.resolve("gemini-2.5-flash").unwrap().input, 3e-07);
      }
  }
  ```

- [ ] **Step 3: Run the test — expect a compile failure.**
  ```bash
  cargo test pricing:: -- --nocapture
  ```
  Expected: FAIL to compile with `cannot find function 'normalize_model' in this scope`, `cannot find type 'Rates'`, `failed to resolve: use of undeclared crate or module 'RateMap'`, etc. (the implementation does not exist yet).

- [ ] **Step 4: Fetch the bundled LiteLLM snapshot.**
  The implementation's `include_str!("../resources/model_prices.json")` fallback requires this file to exist at compile time.
  ```bash
  mkdir -p resources
  curl -sSL --max-time 30 \
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json" \
    -o resources/model_prices.json
  test -s resources/model_prices.json && head -c 40 resources/model_prices.json && echo " ... snapshot saved"
  ```
  Expected: prints the opening of the JSON (e.g. `{ "sample_spec": {`) then `... snapshot saved`. If the machine is offline, drop in any valid JSON object literal (`echo '{}' > resources/model_prices.json`) so the crate compiles; the real file is refreshed at runtime by `refresh_prices`.

- [ ] **Step 5: Implement pricing.rs (prepend above the test module).**
  Insert this complete implementation at the TOP of `src-tauri/src/pricing.rs`, before the existing `#[cfg(test)] mod tests` block:
  ```rust
  use rusqlite::Connection;
  use serde::{Deserialize, Serialize};
  use std::collections::HashMap;
  use std::path::Path;

  /// Providers whose normalized entry wins a collision over prefixed resellers.
  const CANONICAL: &[&str] = &["anthropic", "openai", "gemini", "vertex_ai-language-models"];

  const LITELLM_URL: &str =
      "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

  /// lowercase -> strip through last '/' -> strip a trailing `-YYYYMMDD` suffix.
  pub fn normalize_model(raw: &str) -> String {
      let lower = raw.to_lowercase();
      let after_slash = match lower.rfind('/') {
          Some(i) => lower[i + 1..].to_string(),
          None => lower,
      };
      if after_slash.len() >= 9 {
          let (head, tail) = after_slash.split_at(after_slash.len() - 9);
          if tail.starts_with('-') && tail[1..].chars().all(|c| c.is_ascii_digit()) {
              return head.to_string();
          }
      }
      after_slash
  }

  #[derive(Debug, Clone, Copy, Default, PartialEq)]
  pub struct Rates {
      pub input: f64,
      pub output: f64,
      pub cache_read: f64,
      pub cache_write_5m: f64,
      pub cache_write_1h: f64,
  }

  /// A candidate price row with Option fields so merges can honor
  /// "never overwrite a non-null value with a null one".
  #[derive(Clone)]
  struct Row {
      input: Option<f64>,
      output: Option<f64>,
      cache_read: Option<f64>,
      cw5m: Option<f64>,
      cw1h: Option<f64>,
  }

  fn cost(entry: &serde_json::Value, key: &str) -> Option<f64> {
      // as_f64 returns None for null AND for string placeholders (e.g. sample_spec).
      entry.get(key).and_then(|v| v.as_f64())
  }

  fn write_price_row(conn: &Connection, model: &str, row: &Row) -> rusqlite::Result<()> {
      // 1h TTL falls back to the 5m rate when absent; null -> 0 at write time.
      let cw5m = row.cw5m.unwrap_or(0.0);
      let cw1h = row.cw1h.or(row.cw5m).unwrap_or(0.0);
      conn.execute(
          "INSERT OR REPLACE INTO prices \
           (model, input_per_tok, output_per_tok, cache_read_per_tok, cache_write_5m_per_tok, cache_write_1h_per_tok) \
           VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
          rusqlite::params![
              model,
              row.input.unwrap_or(0.0),
              row.output.unwrap_or(0.0),
              row.cache_read.unwrap_or(0.0),
              cw5m,
              cw1h,
          ],
      )?;
      Ok(())
  }

  /// Rebuild the `prices` table from a LiteLLM JSON snapshot. Writes an exact row
  /// (model = the raw LiteLLM key) for every entry with a non-null input OR output
  /// cost, plus guarded normalized fallback rows. Returns the row count.
  pub fn rebuild_prices(conn: &mut Connection, litellm_json: &str) -> Result<u64, String> {
      let root: serde_json::Value =
          serde_json::from_str(litellm_json).map_err(|e| format!("parse litellm json: {e}"))?;
      let obj = root
          .as_object()
          .ok_or_else(|| "litellm json is not an object".to_string())?;

      let mut exact: Vec<(String, Row)> = Vec::new();
      let mut norm: HashMap<String, (Row, bool)> = HashMap::new(); // key -> (row, canonical)

      for (key, entry) in obj {
          let input = cost(entry, "input_cost_per_token");
          let output = cost(entry, "output_cost_per_token");
          // Skip entries whose input AND output are both null/non-numeric.
          if input.is_none() && output.is_none() {
              continue;
          }
          let row = Row {
              input,
              output,
              cache_read: cost(entry, "cache_read_input_token_cost"),
              cw5m: cost(entry, "cache_creation_input_token_cost"),
              cw1h: cost(entry, "cache_creation_input_token_cost_above_1hr"),
          };
          exact.push((key.clone(), row.clone()));

          let canonical = entry
              .get("litellm_provider")
              .and_then(|v| v.as_str())
              .map(|p| CANONICAL.contains(&p))
              .unwrap_or(false);
          let nkey = normalize_model(key);
          match norm.get_mut(&nkey) {
              None => {
                  norm.insert(nkey, (row, canonical));
              }
              Some((existing, existing_canon)) => {
                  let new_wins = canonical && !*existing_canon;
                  if new_wins {
                      // New (canonical) row wins; keep an old non-null field only where new is null.
                      existing.input = row.input.or(existing.input);
                      existing.output = row.output.or(existing.output);
                      existing.cache_read = row.cache_read.or(existing.cache_read);
                      existing.cw5m = row.cw5m.or(existing.cw5m);
                      existing.cw1h = row.cw1h.or(existing.cw1h);
                      *existing_canon = true;
                  } else {
                      // New row does not win; only fill fields the existing row lacks.
                      existing.input = existing.input.or(row.input);
                      existing.output = existing.output.or(row.output);
                      existing.cache_read = existing.cache_read.or(row.cache_read);
                      existing.cw5m = existing.cw5m.or(row.cw5m);
                      existing.cw1h = existing.cw1h.or(row.cw1h);
                  }
              }
          }
      }

      let tx = conn.transaction().map_err(|e| e.to_string())?;
      tx.execute("DELETE FROM prices", []).map_err(|e| e.to_string())?;
      // Normalized rows first; exact rows overwrite on key collision (exact is authoritative).
      for (model, (row, _)) in &norm {
          write_price_row(&tx, model, row).map_err(|e| e.to_string())?;
      }
      for (model, row) in &exact {
          write_price_row(&tx, model, row).map_err(|e| e.to_string())?;
      }
      let count: u64 = tx
          .query_row("SELECT COUNT(*) FROM prices", [], |r| r.get(0))
          .map_err(|e| e.to_string())?;
      tx.commit().map_err(|e| e.to_string())?;
      Ok(count)
  }

  /// Fetch the latest LiteLLM snapshot (10s timeout), cache it, and rebuild.
  /// On any fetch/parse failure, fall back to the cached file, then the bundled snapshot.
  pub fn refresh_prices(conn: &mut Connection, cache_dir: &Path) -> Result<u64, String> {
      let cache_file = cache_dir.join("model_prices.json");
      let fetched = ureq::get(LITELLM_URL)
          .timeout(std::time::Duration::from_secs(10))
          .call()
          .ok()
          .and_then(|resp| resp.into_string().ok());
      if let Some(body) = fetched {
          if let Ok(n) = rebuild_prices(conn, &body) {
              let _ = std::fs::create_dir_all(cache_dir);
              let _ = std::fs::write(&cache_file, &body);
              return Ok(n);
          }
      }
      if let Ok(body) = std::fs::read_to_string(&cache_file) {
          return rebuild_prices(conn, &body);
      }
      let bundled = include_str!("../resources/model_prices.json");
      rebuild_prices(conn, bundled)
  }

  #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
  #[serde(rename_all = "camelCase")]
  pub struct OverrideRates {
      pub input: Option<f64>,
      pub output: Option<f64>,
      pub cache_read: Option<f64>,
      pub cache_write: Option<f64>,
  }

  pub fn set_override(conn: &Connection, model: &str, rates: OverrideRates) -> rusqlite::Result<()> {
      conn.execute(
          "INSERT OR REPLACE INTO price_overrides \
           (model, input_per_tok, output_per_tok, cache_read_per_tok, cache_write_per_tok) \
           VALUES (?1, ?2, ?3, ?4, ?5)",
          rusqlite::params![model, rates.input, rates.output, rates.cache_read, rates.cache_write],
      )?;
      Ok(())
  }

  pub fn delete_override(conn: &Connection, model: &str) -> rusqlite::Result<()> {
      conn.execute(
          "DELETE FROM price_overrides WHERE model = ?1",
          rusqlite::params![model],
      )?;
      Ok(())
  }

  pub struct RateMap {
      prices: HashMap<String, Rates>,
      overrides: HashMap<String, Rates>,
  }

  impl RateMap {
      pub fn load(conn: &Connection) -> rusqlite::Result<RateMap> {
          let mut prices = HashMap::new();
          let mut stmt = conn.prepare(
              "SELECT model, input_per_tok, output_per_tok, cache_read_per_tok, \
               cache_write_5m_per_tok, cache_write_1h_per_tok FROM prices",
          )?;
          let rows = stmt.query_map([], |r| {
              Ok((
                  r.get::<_, String>(0)?,
                  Rates {
                      input: r.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                      output: r.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                      cache_read: r.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                      cache_write_5m: r.get::<_, Option<f64>>(4)?.unwrap_or(0.0),
                      cache_write_1h: r.get::<_, Option<f64>>(5)?.unwrap_or(0.0),
                  },
              ))
          })?;
          for row in rows {
              let (m, rt) = row?;
              prices.insert(m, rt);
          }

          let mut overrides = HashMap::new();
          let mut stmt2 = conn.prepare(
              "SELECT model, input_per_tok, output_per_tok, cache_read_per_tok, \
               cache_write_per_tok FROM price_overrides",
          )?;
          let orows = stmt2.query_map([], |r| {
              // Override's single cache_write applies to BOTH TTLs; None -> 0.
              let cw = r.get::<_, Option<f64>>(4)?.unwrap_or(0.0);
              Ok((
                  r.get::<_, String>(0)?,
                  Rates {
                      input: r.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                      output: r.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                      cache_read: r.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                      cache_write_5m: cw,
                      cache_write_1h: cw,
                  },
              ))
          })?;
          for row in orows {
              let (m, rt) = row?;
              overrides.insert(m, rt);
          }

          Ok(RateMap { prices, overrides })
      }

      /// override (raw name) -> exact price (raw name) -> normalized price. None = unpriced.
      pub fn resolve(&self, raw_model: &str) -> Option<Rates> {
          if let Some(r) = self.overrides.get(raw_model) {
              return Some(*r);
          }
          if let Some(r) = self.prices.get(raw_model) {
              return Some(*r);
          }
          self.prices.get(&normalize_model(raw_model)).copied()
      }
  }
  ```

- [ ] **Step 6: Run the test — expect all green.**
  ```bash
  cargo test pricing:: -- --nocapture
  ```
  Expected: PASS — `normalize_strips_slash_and_date_suffix`, `rebuild_counts_distinct_rows`, `exact_wins_and_null_reseller_does_not_pollute`, `canonical_wins_normalized_collision`, `claude_cache_rates_and_1h_fallback`, `unknown_model_is_none`, `override_wins_fills_none_and_applies_cache_write_both_ttls` all pass (`test result: ok. 7 passed`).

- [ ] **Step 7: Commit.**
  ```bash
  git add src-tauri/src/pricing.rs src-tauri/src/lib.rs src-tauri/resources/model_prices.json
  git commit -m "feat: add pricing.rs with LiteLLM rebuild, model normalization, overrides, and RateMap"
  ```


---

### Task 9: queries.rs

**Files:**
- Create: `src-tauri/src/queries.rs`
- Modify: `src-tauri/src/lib.rs` (declare the module)
- Test: inline `#[cfg(test)] mod tests` inside `src-tauri/src/queries.rs`

**Interfaces:**

- Consumes (from Task 2):
  ```rust
  pub fn open_db(path: &std::path::Path) -> rusqlite::Result<rusqlite::Connection>;
  pub fn insert_events(conn: &mut Connection, events: &[UsageEvent]) -> rusqlite::Result<u64>;
  // types.rs
  pub struct UsageEvent {
      pub dedup_key: String, pub source: String, pub timestamp: i64, pub model: String,
      pub project: Option<String>, pub api_calls: i64, pub input_tokens: i64,
      pub output_tokens: i64, pub cache_read_tokens: i64,
      pub cache_write_5m_tokens: i64, pub cache_write_1h_tokens: i64, pub source_file: String,
  }
  ```
- Consumes (from Task 8):
  ```rust
  pub struct Rates { pub input: f64, pub output: f64, pub cache_read: f64, pub cache_write_5m: f64, pub cache_write_1h: f64 } // per token
  pub struct RateMap { /* private */ }
  impl RateMap {
      pub fn load(conn: &Connection) -> rusqlite::Result<RateMap>;
      pub fn resolve(&self, raw_model: &str) -> Option<Rates>; // None = unpriced
  }
  pub fn set_override(conn: &Connection, model: &str, rates: OverrideRates) -> rusqlite::Result<()>;
  pub struct OverrideRates { pub input: Option<f64>, pub output: Option<f64>, pub cache_read: Option<f64>, pub cache_write: Option<f64> }
  ```
- Produces (relied on by Task 10 commands):
  ```rust
  pub struct Filters { pub tools: Vec<String>, pub models: Vec<String>, pub project: Option<String>, pub start_ts: Option<i64>, pub end_ts: Option<i64> } // empty vec = all; end exclusive
  pub struct Summary { pub input_tokens: i64, pub output_tokens: i64, pub cache_read_tokens: i64, pub cache_write_tokens: i64, pub total_tokens: i64, pub requests: i64, pub cost: Option<f64>, pub has_unpriced: bool, pub unpriced_models: Vec<String>, pub cache_hit_rate: f64 }
  pub struct TrendPoint { pub bucket: String, pub input_tokens: i64, pub output_tokens: i64, pub cache_read_tokens: i64, pub cache_write_tokens: i64, pub total_tokens: i64, pub cost: f64 }
  pub struct BreakdownRow { pub key: String, pub input_tokens: i64, pub output_tokens: i64, pub cache_read_tokens: i64, pub cache_write_tokens: i64, pub total_tokens: i64, pub requests: i64, pub cost: Option<f64> }
  pub fn summary(conn: &Connection, f: &Filters) -> rusqlite::Result<Summary>;
  pub fn trend(conn: &Connection, f: &Filters, bucket: &str) -> rusqlite::Result<Vec<TrendPoint>>;
  pub fn breakdown(conn: &Connection, by: &str, f: &Filters) -> rusqlite::Result<Vec<BreakdownRow>>;
  ```

---

- [ ] **Step 1: Declare the module in `lib.rs`**

  Add `pub mod queries;` alongside the other `pub mod` declarations near the top of `src-tauri/src/lib.rs` (it sits next to `pub mod types;`, `pub mod db;`, `pub mod pricing;` from earlier tasks). Nothing else in `lib.rs` changes in this task.

- [ ] **Step 2: Write the failing test + the query types (no functions yet)**

  Create `src-tauri/src/queries.rs` with the serde structs and the complete test module. The three query functions are intentionally absent so the crate fails to compile — that is the red state.

  ```rust
  use serde::{Deserialize, Serialize};

  #[derive(Debug, Clone, Default, Deserialize)]
  #[serde(rename_all = "camelCase")]
  pub struct Filters {
      pub tools: Vec<String>,
      pub models: Vec<String>,
      pub project: Option<String>,
      pub start_ts: Option<i64>,
      pub end_ts: Option<i64>,
  }

  #[derive(Debug, Serialize)]
  #[serde(rename_all = "camelCase")]
  pub struct Summary {
      pub input_tokens: i64,
      pub output_tokens: i64,
      pub cache_read_tokens: i64,
      pub cache_write_tokens: i64,
      pub total_tokens: i64,
      pub requests: i64,
      pub cost: Option<f64>,
      pub has_unpriced: bool,
      pub unpriced_models: Vec<String>,
      pub cache_hit_rate: f64,
  }

  #[derive(Debug, Serialize)]
  #[serde(rename_all = "camelCase")]
  pub struct TrendPoint {
      pub bucket: String,
      pub input_tokens: i64,
      pub output_tokens: i64,
      pub cache_read_tokens: i64,
      pub cache_write_tokens: i64,
      pub total_tokens: i64,
      pub cost: f64,
  }

  #[derive(Debug, Serialize)]
  #[serde(rename_all = "camelCase")]
  pub struct BreakdownRow {
      pub key: String,
      pub input_tokens: i64,
      pub output_tokens: i64,
      pub cache_read_tokens: i64,
      pub cache_write_tokens: i64,
      pub total_tokens: i64,
      pub requests: i64,
      pub cost: Option<f64>,
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::db;
      use crate::pricing::{self, OverrideRates};
      use crate::types::UsageEvent;
      use tempfile::tempdir;

      // 2026-07-01T12:00:00Z and 2026-07-02T12:00:00Z (event times)
      const DAY1_TS: i64 = 1_782_907_200;
      const DAY2_TS: i64 = 1_782_993_600;
      // 2026-07-01T00:00:00Z and 2026-07-02T00:00:00Z (local-midnight bounds under TZ=UTC)
      const DAY1_START: i64 = 1_782_864_000;
      const DAY2_START: i64 = 1_782_950_400;

      fn approx(a: f64, b: f64) {
          assert!((a - b).abs() < 1e-9, "{a} != {b}");
      }

      #[allow(clippy::too_many_arguments)]
      fn ev(
          key: &str, source: &str, ts: i64, model: &str, project: Option<&str>,
          calls: i64, input: i64, output: i64, cr: i64, w5: i64, w1: i64,
      ) -> UsageEvent {
          UsageEvent {
              dedup_key: key.to_string(),
              source: source.to_string(),
              timestamp: ts,
              model: model.to_string(),
              project: project.map(|p| p.to_string()),
              api_calls: calls,
              input_tokens: input,
              output_tokens: output,
              cache_read_tokens: cr,
              cache_write_5m_tokens: w5,
              cache_write_1h_tokens: w1,
              source_file: "fixture.jsonl".to_string(),
          }
      }

      // Seed: two priced gpt-5.4 events (day1 + day2, project alpha, source codex)
      // and one unpriced hermes-local event (day1, no project, source hermes,
      // api_call_count = 3). Prices for gpt-5.4 inserted directly.
      fn seed() -> (tempfile::TempDir, rusqlite::Connection) {
          let dir = tempdir().unwrap();
          let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
          let events = vec![
              ev("a", "codex", DAY1_TS, "gpt-5.4", Some("/Users/dev/projects/alpha"), 1, 1000, 500, 200, 100, 50),
              ev("b", "codex", DAY2_TS, "gpt-5.4", Some("/Users/dev/projects/alpha"), 1, 2000, 1000, 0, 0, 0),
              ev("c", "hermes", DAY1_TS, "hermes-local", None, 3, 300, 100, 0, 0, 0),
          ];
          db::insert_events(&mut conn, &events).unwrap();
          conn.execute(
              "INSERT INTO prices (model, input_per_tok, output_per_tok, cache_read_per_tok, cache_write_5m_per_tok, cache_write_1h_per_tok) \
               VALUES ('gpt-5.4', 0.000002, 0.000010, 0.0000005, 0.0000025, 0.000004)",
              [],
          ).unwrap();
          (dir, conn)
      }

      #[test]
      fn summary_totals_cost_and_unpriced() {
          let (_dir, conn) = seed();
          let s = summary(&conn, &Filters::default()).unwrap();
          assert_eq!(s.input_tokens, 3300);
          assert_eq!(s.output_tokens, 1600);
          assert_eq!(s.cache_read_tokens, 200);
          assert_eq!(s.cache_write_tokens, 150);
          assert_eq!(s.total_tokens, 5250);
          assert_eq!(s.requests, 5);
          // gpt-5.4 agg: in3000 out1500 cr200 w5=100 w1=50
          // = 0.006 + 0.015 + 0.0001 + 0.00025 + 0.0002
          approx(s.cost.unwrap(), 0.02155);
          assert!(s.has_unpriced);
          assert_eq!(s.unpriced_models, vec!["hermes-local".to_string()]);
          approx(s.cache_hit_rate, 200.0 / 3650.0); // cr / (input + cr + cache_write)
      }

      #[test]
      fn summary_tool_filter_excludes_unpriced() {
          let (_dir, conn) = seed();
          let f = Filters { tools: vec!["codex".to_string()], ..Filters::default() };
          let s = summary(&conn, &f).unwrap();
          assert_eq!(s.total_tokens, 4850);
          assert_eq!(s.requests, 2);
          approx(s.cost.unwrap(), 0.02155);
          assert!(!s.has_unpriced);
          assert!(s.unpriced_models.is_empty());
      }

      #[test]
      fn summary_end_ts_is_exclusive() {
          let (_dir, conn) = seed();
          let f = Filters { start_ts: Some(DAY1_START), end_ts: Some(DAY2_START), ..Filters::default() };
          let s = summary(&conn, &f).unwrap();
          assert_eq!(s.total_tokens, 2250); // only day-1 events A + C; day-2 B excluded
          assert_eq!(s.requests, 4);
          approx(s.cost.unwrap(), 0.00755); // event A only
          assert!(s.has_unpriced);
      }

      #[test]
      fn override_prices_previously_unpriced_model() {
          let (_dir, conn) = seed();
          pricing::set_override(&conn, "hermes-local", OverrideRates {
              input: Some(0.000001), output: None, cache_read: None, cache_write: None,
          }).unwrap();
          let s = summary(&conn, &Filters::default()).unwrap();
          assert!(!s.has_unpriced);
          assert!(s.unpriced_models.is_empty());
          approx(s.cost.unwrap(), 0.02185); // 0.02155 + 300 * 0.000001
      }

      #[test]
      fn breakdown_by_model_sorted_desc_with_none_cost() {
          let (_dir, conn) = seed();
          let rows = breakdown(&conn, "model", &Filters::default()).unwrap();
          assert_eq!(rows.len(), 2);
          assert_eq!(rows[0].key, "gpt-5.4");
          assert_eq!(rows[0].total_tokens, 4850);
          assert_eq!(rows[0].requests, 2);
          approx(rows[0].cost.unwrap(), 0.02155);
          assert_eq!(rows[1].key, "hermes-local");
          assert_eq!(rows[1].total_tokens, 400);
          assert_eq!(rows[1].requests, 3);
          assert!(rows[1].cost.is_none());
      }

      #[test]
      fn breakdown_by_project_maps_null_to_unknown() {
          let (_dir, conn) = seed();
          let rows = breakdown(&conn, "project", &Filters::default()).unwrap();
          assert_eq!(rows[0].key, "/Users/dev/projects/alpha");
          assert_eq!(rows[0].total_tokens, 4850);
          assert_eq!(rows[1].key, "unknown");
          assert_eq!(rows[1].total_tokens, 400);
      }

      #[test]
      fn trend_daily_buckets_local_time() {
          std::env::set_var("TZ", "UTC"); // pin bucketing timezone for a deterministic date string
          let (_dir, conn) = seed();
          let pts = trend(&conn, &Filters::default(), "day").unwrap();
          assert_eq!(pts.len(), 2);
          assert_eq!(pts[0].bucket, "2026-07-01");
          assert_eq!(pts[0].total_tokens, 2250); // A + C
          approx(pts[0].cost, 0.00755);
          assert_eq!(pts[1].bucket, "2026-07-02");
          assert_eq!(pts[1].total_tokens, 3000); // B
          approx(pts[1].cost, 0.014);
      }
  }
  ```

- [ ] **Step 3: Run — expect RED (functions don't exist)**

  ```bash
  cargo test --manifest-path src-tauri/Cargo.toml queries:: -- --nocapture
  ```
  Expected: compile error, `error[E0425]: cannot find function `summary` in this scope` (and the same for `trend`, `breakdown`).

- [ ] **Step 4: Implement `build_where` + the three query functions**

  Add the following to `src-tauri/src/queries.rs` immediately **above** the `#[cfg(test)] mod tests` block. The `use` lines go at the top of this inserted block (Rust allows `use` mid-module).

  ```rust
  use std::collections::HashMap;
  use rusqlite::{params_from_iter, types::Value, Connection};
  use crate::pricing::RateMap;

  // Builds the dynamic WHERE fragment (empty vec = no constraint; end_ts exclusive).
  fn build_where(f: &Filters) -> (String, Vec<Value>) {
      let mut clauses: Vec<String> = Vec::new();
      let mut params: Vec<Value> = Vec::new();
      if !f.tools.is_empty() {
          let ph = f.tools.iter().map(|_| "?").collect::<Vec<_>>().join(",");
          clauses.push(format!("source IN ({ph})"));
          for t in &f.tools {
              params.push(Value::Text(t.clone()));
          }
      }
      if !f.models.is_empty() {
          let ph = f.models.iter().map(|_| "?").collect::<Vec<_>>().join(",");
          clauses.push(format!("model IN ({ph})"));
          for m in &f.models {
              params.push(Value::Text(m.clone()));
          }
      }
      if let Some(p) = &f.project {
          clauses.push("project = ?".to_string());
          params.push(Value::Text(p.clone()));
      }
      if let Some(s) = f.start_ts {
          clauses.push("timestamp >= ?".to_string());
          params.push(Value::Integer(s));
      }
      if let Some(e) = f.end_ts {
          clauses.push("timestamp < ?".to_string());
          params.push(Value::Integer(e));
      }
      let where_sql = if clauses.is_empty() {
          String::new()
      } else {
          format!("WHERE {}", clauses.join(" AND "))
      };
      (where_sql, params)
  }

  pub fn summary(conn: &Connection, f: &Filters) -> rusqlite::Result<Summary> {
      let rates = RateMap::load(conn)?;
      let (where_sql, params) = build_where(f);
      let sql = format!(
          "SELECT model, \
           SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens), \
           SUM(cache_write_5m_tokens), SUM(cache_write_1h_tokens), SUM(api_calls) \
           FROM events {where_sql} GROUP BY model"
      );
      let mut stmt = conn.prepare(&sql)?;
      let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
          Ok((
              r.get::<_, String>(0)?,
              r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?,
              r.get::<_, i64>(4)?, r.get::<_, i64>(5)?, r.get::<_, i64>(6)?,
          ))
      })?;

      let (mut input, mut output, mut cache_read, mut cw5m, mut cw1h, mut requests) =
          (0i64, 0i64, 0i64, 0i64, 0i64, 0i64);
      let mut cost = 0.0f64;
      let mut priced_tokens = 0i64;
      let mut unpriced_models: Vec<String> = Vec::new();

      for row in rows {
          let (model, in_, out, cr, w5, w1, calls) = row?;
          input += in_;
          output += out;
          cache_read += cr;
          cw5m += w5;
          cw1h += w1;
          requests += calls;
          let tokens = in_ + out + cr + w5 + w1;
          match rates.resolve(&model) {
              Some(rt) => {
                  cost += in_ as f64 * rt.input
                      + out as f64 * rt.output
                      + cr as f64 * rt.cache_read
                      + w5 as f64 * rt.cache_write_5m
                      + w1 as f64 * rt.cache_write_1h;
                  priced_tokens += tokens;
              }
              None => {
                  if tokens > 0 {
                      unpriced_models.push(model);
                  }
              }
          }
      }

      let cache_write = cw5m + cw1h;
      let total = input + output + cache_read + cache_write;
      let denom = input + cache_read + cache_write;
      let cache_hit_rate = if denom > 0 {
          cache_read as f64 / denom as f64
      } else {
          0.0
      };

      Ok(Summary {
          input_tokens: input,
          output_tokens: output,
          cache_read_tokens: cache_read,
          cache_write_tokens: cache_write,
          total_tokens: total,
          requests,
          cost: if priced_tokens > 0 { Some(cost) } else { None },
          has_unpriced: !unpriced_models.is_empty(),
          unpriced_models,
          cache_hit_rate,
      })
  }

  pub fn trend(conn: &Connection, f: &Filters, bucket: &str) -> rusqlite::Result<Vec<TrendPoint>> {
      let fmt = if bucket == "hour" { "%Y-%m-%d %H:00" } else { "%Y-%m-%d" };
      let rates = RateMap::load(conn)?;
      let (where_sql, params) = build_where(f);
      let sql = format!(
          "SELECT strftime('{fmt}', timestamp, 'unixepoch', 'localtime') AS bucket, model, \
           SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens), \
           SUM(cache_write_5m_tokens), SUM(cache_write_1h_tokens) \
           FROM events {where_sql} GROUP BY bucket, model ORDER BY bucket"
      );
      let mut stmt = conn.prepare(&sql)?;
      let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
          Ok((
              r.get::<_, String>(0)?, r.get::<_, String>(1)?,
              r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?,
              r.get::<_, i64>(5)?, r.get::<_, i64>(6)?,
          ))
      })?;

      let mut idx: HashMap<String, usize> = HashMap::new();
      let mut points: Vec<TrendPoint> = Vec::new();
      for row in rows {
          let (bucket, model, in_, out, cr, w5, w1) = row?;
          let c = match rates.resolve(&model) {
              Some(rt) => {
                  in_ as f64 * rt.input
                      + out as f64 * rt.output
                      + cr as f64 * rt.cache_read
                      + w5 as f64 * rt.cache_write_5m
                      + w1 as f64 * rt.cache_write_1h
              }
              None => 0.0,
          };
          let i = *idx.entry(bucket.clone()).or_insert_with(|| {
              points.push(TrendPoint {
                  bucket: bucket.clone(),
                  input_tokens: 0,
                  output_tokens: 0,
                  cache_read_tokens: 0,
                  cache_write_tokens: 0,
                  total_tokens: 0,
                  cost: 0.0,
              });
              points.len() - 1
          });
          let p = &mut points[i];
          p.input_tokens += in_;
          p.output_tokens += out;
          p.cache_read_tokens += cr;
          p.cache_write_tokens += w5 + w1;
          p.total_tokens += in_ + out + cr + w5 + w1;
          p.cost += c;
      }
      Ok(points)
  }

  #[derive(Default)]
  struct Agg {
      input: i64,
      output: i64,
      cache_read: i64,
      cache_write: i64,
      total: i64,
      requests: i64,
      cost: f64,
      priced: i64,
  }

  pub fn breakdown(conn: &Connection, by: &str, f: &Filters) -> rusqlite::Result<Vec<BreakdownRow>> {
      let group_col = match by {
          "tool" => "source",
          "project" => "project",
          _ => "model",
      };
      let rates = RateMap::load(conn)?;
      let (where_sql, params) = build_where(f);
      let sql = format!(
          "SELECT {group_col} AS grp, model, \
           SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens), \
           SUM(cache_write_5m_tokens), SUM(cache_write_1h_tokens), SUM(api_calls) \
           FROM events {where_sql} GROUP BY grp, model"
      );
      let mut stmt = conn.prepare(&sql)?;
      let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
          Ok((
              r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?,
              r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?,
              r.get::<_, i64>(5)?, r.get::<_, i64>(6)?, r.get::<_, i64>(7)?,
          ))
      })?;

      let mut map: HashMap<String, Agg> = HashMap::new();
      for row in rows {
          let (grp, model, in_, out, cr, w5, w1, calls) = row?;
          let key = grp.unwrap_or_else(|| "unknown".to_string());
          let a = map.entry(key).or_default();
          a.input += in_;
          a.output += out;
          a.cache_read += cr;
          a.cache_write += w5 + w1;
          a.total += in_ + out + cr + w5 + w1;
          a.requests += calls;
          if let Some(rt) = rates.resolve(&model) {
              a.cost += in_ as f64 * rt.input
                  + out as f64 * rt.output
                  + cr as f64 * rt.cache_read
                  + w5 as f64 * rt.cache_write_5m
                  + w1 as f64 * rt.cache_write_1h;
              a.priced += in_ + out + cr + w5 + w1;
          }
      }

      let mut out: Vec<BreakdownRow> = map
          .into_iter()
          .map(|(key, a)| BreakdownRow {
              key,
              input_tokens: a.input,
              output_tokens: a.output,
              cache_read_tokens: a.cache_read,
              cache_write_tokens: a.cache_write,
              total_tokens: a.total,
              requests: a.requests,
              cost: if a.priced > 0 { Some(a.cost) } else { None },
          })
          .collect();
      out.sort_by(|x, y| y.total_tokens.cmp(&x.total_tokens));
      Ok(out)
  }
  ```

- [ ] **Step 5: Run — expect GREEN**

  ```bash
  cargo test --manifest-path src-tauri/Cargo.toml queries:: -- --nocapture
  ```
  Expected: `test result: ok. 7 passed; 0 failed`.

- [ ] **Step 6: Commit**

  ```bash
  git add src-tauri/src/queries.rs src-tauri/src/lib.rs
  git commit -m "feat: add summary/trend/breakdown query aggregation with retroactive pricing"
  ```


---

### Task 10: Wiring (lib.rs, commands, tauri.conf)

Depends on Tasks 7 (`scan.rs`), 8 (`pricing.rs`), 9 (`queries.rs`) — and transitively 2–6. This task assembles the Rust core into a runnable Tauri app: `AppState`, the six IPC commands, launch-time setup (app data dir, `open_db`, background price refresh), a thin `main.rs`, and the final `tauri.conf.json`. Commands are thin pass-throughs, so the only new test is a smoke test proving `AppState` and the command call-shapes wire against the real functions.

**Files:**
- Modify: `src-tauri/src/lib.rs` (currently the create-tauri-app scaffold with a `greet` command)
- Modify: `src-tauri/src/main.rs` (scaffold; confirm thin)
- Modify: `src-tauri/tauri.conf.json` (scaffold defaults → final)
- Test: `src-tauri/src/lib.rs` (`#[cfg(test)] mod tests` — smoke test)

**Interfaces:**
- Consumes (exact signatures from earlier tasks):
  - `types.rs`: `pub struct ScanStatus { pub sources: Vec<SourceStatus>, pub scanned_at: i64 }`
  - `db.rs`: `pub fn open_db(path: &std::path::Path) -> rusqlite::Result<rusqlite::Connection>;`
  - `scan.rs`: `pub struct SourceRoots { pub claude: PathBuf, pub codex: PathBuf, pub gemini_tmp: PathBuf, pub gemini_projects_json: PathBuf, pub hermes_db: PathBuf }`, `impl SourceRoots { pub fn default_roots() -> Self; }`, `pub fn run_scan(conn: &mut Connection, roots: &SourceRoots) -> ScanStatus;`
  - `pricing.rs`: `pub fn refresh_prices(conn: &mut Connection, cache_dir: &Path) -> Result<u64, String>;`, `pub fn set_override(conn: &Connection, model: &str, rates: OverrideRates) -> rusqlite::Result<()>;`, `pub fn delete_override(conn: &Connection, model: &str) -> rusqlite::Result<()>;`, `pub struct OverrideRates { pub input: Option<f64>, pub output: Option<f64>, pub cache_read: Option<f64>, pub cache_write: Option<f64> }`
  - `queries.rs`: `pub struct Filters { ... }` (derives `Default`), `pub fn summary(conn: &Connection, f: &Filters) -> rusqlite::Result<Summary>;`, `pub fn trend(conn: &Connection, f: &Filters, bucket: &str) -> rusqlite::Result<Vec<TrendPoint>>;`, `pub fn breakdown(conn: &Connection, by: &str, f: &Filters) -> rusqlite::Result<Vec<BreakdownRow>>;`, plus `Summary`, `TrendPoint`, `BreakdownRow`.
- Produces (relied on by frontend Tasks 11–15 via IPC command names):
  - `pub struct AppState { pub db: Mutex<Connection>, pub roots: SourceRoots, pub scan_lock: Mutex<()> }`
  - `#[tauri::command] async fn scan(state) -> Result<ScanStatus, String>`
  - `#[tauri::command] fn summary(state, filters: Filters) -> Result<Summary, String>`
  - `#[tauri::command] fn trend(state, filters: Filters, bucket: String) -> Result<Vec<TrendPoint>, String>`
  - `#[tauri::command] fn breakdown(state, by: String, filters: Filters) -> Result<Vec<BreakdownRow>, String>`
  - `#[tauri::command] fn set_price_override(state, model: String, rates: OverrideRates) -> Result<(), String>`
  - `#[tauri::command] fn delete_price_override(state, model: String) -> Result<(), String>`
  - `pub fn run()` (called by `main.rs`)

---

- [ ] **Step 1: Write the failing smoke test.** Append this test module to the **bottom** of the scaffolded `src-tauri/src/lib.rs` (leave the rest of the scaffold untouched for now). It references `AppState` and the crate modules `db`/`scan`/`queries`, none of which the scaffold declares yet, so it must fail to compile.

```rust
#[cfg(test)]
mod tests {
    use super::AppState;
    use crate::queries::Filters;
    use crate::scan::SourceRoots;
    use crate::{db, queries, scan};
    use std::sync::Mutex;

    // Proves AppState constructs and the exact call-shapes used by the IPC
    // commands (run_scan + queries::summary) type-check against the real
    // functions. Empty fixture roots => 4 source statuses, zero events.
    #[test]
    fn appstate_wires_scan_and_query() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_db(&dir.path().join("tokenledger.db")).unwrap();
        let roots = SourceRoots {
            claude: dir.path().join("claude"),
            codex: dir.path().join("codex"),
            gemini_tmp: dir.path().join("gemini"),
            gemini_projects_json: dir.path().join("projects.json"),
            hermes_db: dir.path().join("state.db"),
        };
        let state = AppState {
            db: Mutex::new(conn),
            roots,
            scan_lock: Mutex::new(()),
        };

        let mut db = state.db.lock().unwrap();
        let status = scan::run_scan(&mut db, &state.roots);
        assert_eq!(status.sources.len(), 4);

        let sum = queries::summary(&db, &Filters::default()).unwrap();
        assert_eq!(sum.total_tokens, 0);
    }
}
```

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml appstate_wires_scan_and_query
```

Expected: **FAIL to compile** — `error[E0432]: unresolved import crate::db` (and `crate::queries`, `crate::scan`) and `error[E0412]: cannot find type AppState in this scope`.

- [ ] **Step 2: Implement `lib.rs` — modules, AppState, commands, setup.** Replace the **entire contents** of `src-tauri/src/lib.rs` with the following (this removes the scaffold `greet` command and keeps the smoke test from Step 1 at the bottom):

```rust
mod adapters;
mod db;
mod pricing;
mod queries;
mod scan;
mod types;

use std::sync::Mutex;

use rusqlite::Connection;
use tauri::{Manager, State};

use pricing::OverrideRates;
use queries::{BreakdownRow, Filters, Summary, TrendPoint};
use scan::{run_scan, SourceRoots};
use types::ScanStatus;

pub struct AppState {
    pub db: Mutex<Connection>,
    pub roots: SourceRoots,
    pub scan_lock: Mutex<()>,
}

#[tauri::command]
async fn scan(state: State<'_, AppState>) -> Result<ScanStatus, String> {
    // Serialize scans: a second caller blocks on scan_lock, then runs its own
    // incremental scan. That is the coalescing policy.
    let _guard = state.scan_lock.lock().map_err(|e| e.to_string())?;
    // ponytail: single Mutex<Connection> per the AppState contract. A scan
    // briefly blocks reads; incremental scans are cheap, so no separate read
    // connection. Add one only if UI jank during scans is ever measured.
    let mut db = state.db.lock().map_err(|e| e.to_string())?;
    Ok(run_scan(&mut db, &state.roots))
}

#[tauri::command]
fn summary(state: State<'_, AppState>, filters: Filters) -> Result<Summary, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    queries::summary(&db, &filters).map_err(|e| e.to_string())
}

#[tauri::command]
fn trend(
    state: State<'_, AppState>,
    filters: Filters,
    bucket: String,
) -> Result<Vec<TrendPoint>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    queries::trend(&db, &filters, &bucket).map_err(|e| e.to_string())
}

#[tauri::command]
fn breakdown(
    state: State<'_, AppState>,
    by: String,
    filters: Filters,
) -> Result<Vec<BreakdownRow>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    queries::breakdown(&db, &by, &filters).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_price_override(
    state: State<'_, AppState>,
    model: String,
    rates: OverrideRates,
) -> Result<(), String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    pricing::set_override(&db, &model, rates).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_price_override(state: State<'_, AppState>, model: String) -> Result<(), String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    pricing::delete_override(&db, &model).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let conn = db::open_db(&data_dir.join("tokenledger.db"))?;
            app.manage(AppState {
                db: Mutex::new(conn),
                roots: SourceRoots::default_roots(),
                scan_lock: Mutex::new(()),
            });
            // Refresh LiteLLM prices off the main thread; any fetch failure falls
            // back to the cached/bundled snapshot inside refresh_prices.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let state = handle.state::<AppState>();
                if let Ok(mut db) = state.db.lock() {
                    let _ = pricing::refresh_prices(&mut db, &data_dir);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            scan,
            summary,
            trend,
            breakdown,
            set_price_override,
            delete_price_override
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::AppState;
    use crate::queries::Filters;
    use crate::scan::SourceRoots;
    use crate::{db, queries, scan};
    use std::sync::Mutex;

    // Proves AppState constructs and the exact call-shapes used by the IPC
    // commands (run_scan + queries::summary) type-check against the real
    // functions. Empty fixture roots => 4 source statuses, zero events.
    #[test]
    fn appstate_wires_scan_and_query() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_db(&dir.path().join("tokenledger.db")).unwrap();
        let roots = SourceRoots {
            claude: dir.path().join("claude"),
            codex: dir.path().join("codex"),
            gemini_tmp: dir.path().join("gemini"),
            gemini_projects_json: dir.path().join("projects.json"),
            hermes_db: dir.path().join("state.db"),
        };
        let state = AppState {
            db: Mutex::new(conn),
            roots,
            scan_lock: Mutex::new(()),
        };

        let mut db = state.db.lock().unwrap();
        let status = scan::run_scan(&mut db, &state.roots);
        assert_eq!(status.sources.len(), 4);

        let sum = queries::summary(&db, &Filters::default()).unwrap();
        assert_eq!(sum.total_tokens, 0);
    }
}
```

Notes for the implementer (no action, just why it compiles):
- `async fn scan` holds the two `MutexGuard`s but has **no `.await`**, so the future is `Send` and Tauri accepts it. `scan` must be async so the (potentially slow) scan runs off the IPC/main thread; the query commands stay sync (fast, indexed reads).
- The `fn scan` command and `mod scan` coexist (different namespaces); `queries::summary` is module-qualified to avoid the `fn summary` command name.

- [ ] **Step 3: Confirm `main.rs` is thin.** The create-tauri-app scaffold already generates this; set the contents of `src-tauri/src/main.rs` to exactly:

```rust
// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tokenledger_lib::run();
}
```

(If the scaffold used a different lib crate name than `tokenledger_lib`, it is `<package_name>_lib` from `src-tauri/Cargo.toml`; Task 1 named the package `tokenledger`, so `tokenledger_lib` is correct.)

- [ ] **Step 4: Finalize `tauri.conf.json`.** Replace the **entire contents** of `src-tauri/tauri.conf.json` with:

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "TokenLedger",
  "version": "0.1.0",
  "identifier": "com.brianwong.tokenledger",
  "build": {
    "beforeDevCommand": "npm run dev",
    "devUrl": "http://localhost:1420",
    "beforeBuildCommand": "npm run build",
    "frontendDist": "../dist"
  },
  "app": {
    "windows": [
      {
        "title": "TokenLedger",
        "width": 1100,
        "height": 780,
        "minWidth": 900,
        "minHeight": 640
      }
    ],
    "security": {
      "csp": null
    }
  },
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": [
      "icons/32x32.png",
      "icons/128x128.png",
      "icons/128x128@2x.png",
      "icons/icon.icns",
      "icons/icon.ico"
    ]
  }
}
```

- [ ] **Step 5: Verify green — smoke test, full test suite, and build.** Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml appstate_wires_scan_and_query
cargo test --manifest-path src-tauri/Cargo.toml
cargo build --manifest-path src-tauri/Cargo.toml
```

Expected:
- First: `test tests::appstate_wires_scan_and_query ... ok`.
- Second: all suites from Tasks 2–9 plus this one pass — `test result: ok. ... 0 failed`.
- Third: `Finished \`dev\` profile [unoptimized + debuginfo] target(s)` — the whole crate, `generate_handler!`, and `generate_context!` (which parses `tauri.conf.json`) compile.

- [ ] **Step 6: Commit.**

```bash
git add src-tauri/src/lib.rs src-tauri/src/main.rs src-tauri/tauri.conf.json
git commit -m "feat: wire AppState, IPC commands, and Tauri app config"
```


---

### Task 11: Frontend foundation

**Files:**
- Create: `src/types.ts`
- Create: `src/api.ts`
- Create: `src/lib/dateRange.ts`
- Create: `src/lib/format.ts`
- Modify: `src/App.tsx` (replace the create-tauri-app template body)
- Test: `src/lib/dateRange.test.ts`, `src/lib/format.test.ts` (vitest)

**Interfaces:**
- Consumes (Tauri commands wired in Task 10; JSON is serde `camelCase`):
  - `scan(state) -> Result<ScanStatus, String>`
  - `summary(state, filters: Filters) -> Result<Summary, String>`
  - `trend(state, filters: Filters, bucket: String) -> Result<Vec<TrendPoint>, String>`
  - `breakdown(state, by: String, filters: Filters) -> Result<Vec<BreakdownRow>, String>`
  - `set_price_override(state, model: String, rates: OverrideRates) -> Result<(), String>`
  - `delete_price_override(state, model: String) -> Result<(), String>`
  - Rust struct field shapes (camelCase in JSON): `Filters { tools, models, project, startTs?, endTs? }`, `Summary { inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens, totalTokens, requests, cost, hasUnpriced, unpricedModels, cacheHitRate }`, `TrendPoint { bucket, inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens, totalTokens, cost }`, `BreakdownRow { key, inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens, totalTokens, requests, cost }`, `ScanStatus { sources, scannedAt }`, `SourceStatus { source, eventsInserted, linesSkipped, error }`, `OverrideRates { input, output, cacheRead, cacheWrite }` (each `Option<f64>`).
- Produces (Tasks 12–15 rely on these exact names):
  - `src/types.ts`: `Tool`, `RangePreset`, `CustomRange`, `DateRange`, `Filters`, `Summary`, `TrendPoint`, `BreakdownRow`, `ScanStatus`, `SourceStatus`, `OverrideRates`.
  - `src/api.ts`: `scan()`, `fetchSummary(filters)`, `fetchTrend(filters, bucket)`, `fetchBreakdown(by, filters)`, `setPriceOverride(model, rates)`, `deletePriceOverride(model)`.
  - `src/lib/dateRange.ts`: `rangeToBounds(range, now?) -> { startTs?: number; endTs?: number }` (epoch **seconds**, end exclusive).
  - `src/lib/format.ts`: `formatTokens(n)`, `formatCost(c, hasUnpriced)`.
  - `src/App.tsx`: default-exported `App` holding state `{ tool: Tool | 'all', model: string | 'all', range: DateRange, refreshSec: 0 | 30 | 60 }`, scanning on mount, refetching on filter change, auto-refresh timer.

---

- [ ] **Step 1: Create `src/types.ts`** — mirror the serde `camelCase` JSON of every IPC struct exactly.

```ts
// src/types.ts
// Mirrors the Rust IPC structs (serde rename_all = "camelCase"). Do not rename.

export type Tool = 'claude' | 'codex' | 'gemini' | 'hermes';

export type RangePreset = 'today' | '7d' | '30d' | 'all';

export interface CustomRange {
  start: string; // 'YYYY-MM-DD', inclusive
  end: string;   // 'YYYY-MM-DD', inclusive
}

export type DateRange = RangePreset | CustomRange;

export interface Filters {
  tools: string[];          // empty = all
  models: string[];         // empty = all
  project: string | null;
  startTs?: number;         // epoch seconds, inclusive
  endTs?: number;           // epoch seconds, exclusive
}

export interface Summary {
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number; // 5m + 1h combined
  totalTokens: number;      // hero number
  requests: number;
  cost: number | null;      // null when zero priced tokens in range
  hasUnpriced: boolean;
  unpricedModels: string[];
  cacheHitRate: number;     // 0..1
}

export interface TrendPoint {
  bucket: string;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  totalTokens: number;
  cost: number;
}

export interface BreakdownRow {
  key: string;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  totalTokens: number;
  requests: number;
  cost: number | null;
}

export interface SourceStatus {
  source: string;
  eventsInserted: number;
  linesSkipped: number;
  error: string | null;
}

export interface ScanStatus {
  sources: SourceStatus[];
  scannedAt: number;
}

export interface OverrideRates {
  input: number | null;      // per token; null = 0
  output: number | null;
  cacheRead: number | null;
  cacheWrite: number | null;
}
```

- [ ] **Step 2: Create `src/api.ts`** — thin Tauri v2 `invoke` wrappers (command names match Task 10 exactly).

```ts
// src/api.ts
import { invoke } from '@tauri-apps/api/core';
import type {
  ScanStatus,
  Summary,
  TrendPoint,
  BreakdownRow,
  Filters,
  OverrideRates,
} from './types';

export function scan(): Promise<ScanStatus> {
  return invoke('scan');
}

export function fetchSummary(filters: Filters): Promise<Summary> {
  return invoke('summary', { filters });
}

export function fetchTrend(
  filters: Filters,
  bucket: 'day' | 'hour',
): Promise<TrendPoint[]> {
  return invoke('trend', { filters, bucket });
}

export function fetchBreakdown(
  by: 'tool' | 'model' | 'project',
  filters: Filters,
): Promise<BreakdownRow[]> {
  return invoke('breakdown', { by, filters });
}

export function setPriceOverride(
  model: string,
  rates: OverrideRates,
): Promise<void> {
  return invoke('set_price_override', { model, rates });
}

export function deletePriceOverride(model: string): Promise<void> {
  return invoke('delete_price_override', { model });
}
```

- [ ] **Step 3: Write the failing `rangeToBounds` test** at `src/lib/dateRange.test.ts`. Expected local-time epoch seconds are constructed with the `Date` constructor (source of truth for the Mac's local zone), so the test is independent of the implementation.

```ts
// src/lib/dateRange.test.ts
import { describe, it, expect } from 'vitest';
import { rangeToBounds } from './dateRange';

// Fixed "now": 2026-07-07 14:30 local time.
const NOW = new Date(2026, 6, 7, 14, 30, 0);
const midnightSecs = (y: number, m: number, d: number) =>
  Math.floor(new Date(y, m, d).getTime() / 1000);

describe('rangeToBounds', () => {
  it('today: local midnight start, no end', () => {
    const r = rangeToBounds('today', NOW);
    expect(r.startTs).toBe(midnightSecs(2026, 6, 7));
    expect(r.endTs).toBeUndefined();
  });

  it('7d: midnight six days back, no end', () => {
    const r = rangeToBounds('7d', NOW);
    expect(r.startTs).toBe(midnightSecs(2026, 6, 1));
    expect(r.endTs).toBeUndefined();
  });

  it('30d: midnight 29 days back, no end', () => {
    const r = rangeToBounds('30d', NOW);
    expect(r.startTs).toBe(midnightSecs(2026, 5, 8)); // 2026-06-08
    expect(r.endTs).toBeUndefined();
  });

  it('all: both bounds undefined', () => {
    expect(rangeToBounds('all', NOW)).toEqual({});
  });

  it('custom: inclusive end becomes exclusive next-day midnight', () => {
    const r = rangeToBounds({ start: '2026-07-01', end: '2026-07-03' }, NOW);
    expect(r.startTs).toBe(midnightSecs(2026, 6, 1));
    expect(r.endTs).toBe(midnightSecs(2026, 6, 4)); // 07-03 + 1 day, exclusive
  });
});
```

Run:

```bash
npx vitest run src/lib/dateRange.test.ts
```

Expected: FAIL — `Failed to load .../src/lib/dateRange.ts` / "Cannot find module './dateRange'" (the module does not exist yet).

- [ ] **Step 4: Implement `src/lib/dateRange.ts`** (minimal code to pass). Parses custom dates as **local** midnight (never `new Date('YYYY-MM-DD')`, which is UTC).

```ts
// src/lib/dateRange.ts
import type { DateRange, CustomRange } from '../types';

function midnight(d: Date): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate());
}

function secs(d: Date): number {
  return Math.floor(d.getTime() / 1000);
}

function parseLocalDate(s: string): Date {
  const [y, m, d] = s.split('-').map(Number);
  return new Date(y, m - 1, d);
}

// 'today' = local midnight..now (end open); '7d'/'30d' = midnight (today-6/-29)..open;
// 'all' = both open; custom = midnight(start)..midnight(end + 1 day), end exclusive.
export function rangeToBounds(
  range: DateRange,
  now: Date = new Date(),
): { startTs?: number; endTs?: number } {
  if (typeof range === 'string') {
    switch (range) {
      case 'today':
        return { startTs: secs(midnight(now)) };
      case '7d': {
        const start = midnight(now);
        start.setDate(start.getDate() - 6);
        return { startTs: secs(start) };
      }
      case '30d': {
        const start = midnight(now);
        start.setDate(start.getDate() - 29);
        return { startTs: secs(start) };
      }
      case 'all':
        return {};
    }
  }
  const r = range as CustomRange;
  const start = parseLocalDate(r.start);
  const end = parseLocalDate(r.end);
  end.setDate(end.getDate() + 1); // inclusive end date -> exclusive next-day midnight
  return { startTs: secs(start), endTs: secs(end) };
}
```

Run:

```bash
npx vitest run src/lib/dateRange.test.ts
```

Expected: PASS — 5 passed.

- [ ] **Step 5: Write the failing `format` test** at `src/lib/format.test.ts`.

```ts
// src/lib/format.test.ts
import { describe, it, expect } from 'vitest';
import { formatTokens, formatCost } from './format';

describe('formatTokens', () => {
  it('zero', () => expect(formatTokens(0)).toBe('0'));
  it('groups thousands', () => expect(formatTokens(1234)).toBe('1,234'));
  it('millions', () => expect(formatTokens(1_234_567)).toBe('1.23M'));
  it('billions', () => expect(formatTokens(1_234_567_890)).toBe('1.23B'));
  it('exact million boundary', () => expect(formatTokens(1_000_000)).toBe('1.00M'));
});

describe('formatCost', () => {
  it('null is unpriced', () => expect(formatCost(null, false)).toBe('unpriced'));
  it('priced', () => expect(formatCost(12.5, false)).toBe('$12.50'));
  it('unpriced marker', () => expect(formatCost(12.5, true)).toBe('≥ $12.50'));
});
```

Run:

```bash
npx vitest run src/lib/format.test.ts
```

Expected: FAIL — "Cannot find module './format'" (the module does not exist yet).

- [ ] **Step 6: Implement `src/lib/format.ts`** (minimal code to pass).

```ts
// src/lib/format.ts

// 1234 -> "1,234"; 1_234_567 -> "1.23M"; 1_234_567_890 -> "1.23B".
export function formatTokens(n: number): string {
  if (n >= 1_000_000_000) return (n / 1_000_000_000).toFixed(2) + 'B';
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(2) + 'M';
  return n.toLocaleString('en-US');
}

// null -> "unpriced"; hasUnpriced -> "≥ $X.XX"; else "$X.XX".
export function formatCost(c: number | null, hasUnpriced: boolean): string {
  if (c === null) return 'unpriced';
  const amount = `$${c.toFixed(2)}`;
  return hasUnpriced ? `≥ ${amount}` : amount;
}
```

Run:

```bash
npx vitest run src/lib/format.test.ts
```

Expected: PASS — 8 passed.

- [ ] **Step 7: Replace `src/App.tsx`** with the foundation shell: state, scan-on-mount, filter-change refetch, auto-refresh timer, `Promise.all` data fetch, loading/error/scanning states, plain layout divs. Tasks 12–15 swap the plain divs for real components; the state shape and wiring here are the contract.

```tsx
// src/App.tsx
import { useCallback, useEffect, useMemo, useState } from 'react';
import type {
  Tool,
  DateRange,
  Filters,
  Summary,
  TrendPoint,
  BreakdownRow,
} from './types';
import { scan, fetchSummary, fetchTrend, fetchBreakdown } from './api';
import { rangeToBounds } from './lib/dateRange';
import { formatTokens, formatCost } from './lib/format';

export default function App() {
  const [tool, setTool] = useState<Tool | 'all'>('all');
  const [model, setModel] = useState<string | 'all'>('all');
  const [range, setRange] = useState<DateRange>('30d');
  const [refreshSec, setRefreshSec] = useState<0 | 30 | 60>(30);

  const [summary, setSummary] = useState<Summary | null>(null);
  const [trend, setTrend] = useState<TrendPoint[]>([]);
  const [modelRows, setModelRows] = useState<BreakdownRow[]>([]);
  const [projectRows, setProjectRows] = useState<BreakdownRow[]>([]);

  const [loading, setLoading] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const filters: Filters = useMemo(() => {
    const { startTs, endTs } = rangeToBounds(range);
    return {
      tools: tool === 'all' ? [] : [tool],
      models: model === 'all' ? [] : [model],
      project: null,
      startTs,
      endTs,
    };
  }, [tool, model, range]);

  const loadData = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const bucket = range === 'today' ? 'hour' : 'day';
      const [s, t, m, p] = await Promise.all([
        fetchSummary(filters),
        fetchTrend(filters, bucket),
        fetchBreakdown('model', filters),
        fetchBreakdown('project', filters),
      ]);
      setSummary(s);
      setTrend(t);
      setModelRows(m);
      setProjectRows(p);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [filters, range]);

  const runScan = useCallback(async () => {
    setScanning(true);
    try {
      await scan();
      await loadData();
    } catch (e) {
      setError(String(e));
    } finally {
      setScanning(false);
    }
  }, [loadData]);

  // Scan once on mount.
  useEffect(() => {
    runScan();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Refetch whenever the active filters change.
  useEffect(() => {
    loadData();
  }, [loadData]);

  // Auto-refresh timer; 0 = off. Cleared and recreated on interval change.
  useEffect(() => {
    if (refreshSec === 0) return;
    const id = setInterval(() => {
      runScan();
    }, refreshSec * 1000);
    return () => clearInterval(id);
  }, [refreshSec, runScan]);

  const modelOptions = modelRows.map((r) => r.key);

  return (
    <div style={{ padding: 16, fontFamily: 'system-ui, sans-serif' }}>
      <h1>TokenLedger</h1>

      <div style={{ display: 'flex', gap: 12, marginBottom: 16 }}>
        <select
          value={tool}
          onChange={(e) => setTool(e.target.value as Tool | 'all')}
        >
          <option value="all">All tools</option>
          <option value="claude">Claude</option>
          <option value="codex">Codex</option>
          <option value="gemini">Gemini</option>
          <option value="hermes">Hermes</option>
        </select>

        <select value={model} onChange={(e) => setModel(e.target.value)}>
          <option value="all">All models</option>
          {modelOptions.map((m) => (
            <option key={m} value={m}>
              {m}
            </option>
          ))}
        </select>

        <select
          value={typeof range === 'string' ? range : 'custom'}
          onChange={(e) => setRange(e.target.value as DateRange)}
        >
          <option value="today">Today</option>
          <option value="7d">7d</option>
          <option value="30d">30d</option>
          <option value="all">All</option>
        </select>

        <select
          value={refreshSec}
          onChange={(e) =>
            setRefreshSec(Number(e.target.value) as 0 | 30 | 60)
          }
        >
          <option value={0}>Off</option>
          <option value={30}>30s</option>
          <option value={60}>60s</option>
        </select>

        <button onClick={runScan}>Rescan</button>
      </div>

      {error && <div style={{ color: '#ff5c7a' }}>Error: {error}</div>}
      {loading && <div>Loading…</div>}

      {summary && (
        <div style={{ marginBottom: 16 }}>
          <div style={{ fontSize: 40, fontWeight: 700 }}>
            {formatTokens(summary.totalTokens)}
          </div>
          <div>total tokens</div>
          <div>{summary.requests.toLocaleString('en-US')} requests</div>
          <div>{formatCost(summary.cost, summary.hasUnpriced)}</div>
          <div>at API list prices — not billed</div>
          <div>input {formatTokens(summary.inputTokens)}</div>
          <div>output {formatTokens(summary.outputTokens)}</div>
          <div>cache write {formatTokens(summary.cacheWriteTokens)}</div>
          <div>cache read {formatTokens(summary.cacheReadTokens)}</div>
          <div>
            cache hit rate {(summary.cacheHitRate * 100).toFixed(1)}%
          </div>
        </div>
      )}

      <div>trend points: {trend.length}</div>

      <h2>By model</h2>
      <table>
        <tbody>
          {modelRows.map((r) => (
            <tr key={r.key}>
              <td>{r.key}</td>
              <td>{formatTokens(r.totalTokens)}</td>
              <td>{r.requests}</td>
              <td>{formatCost(r.cost, false)}</td>
            </tr>
          ))}
        </tbody>
      </table>

      <h2>By project</h2>
      <table>
        <tbody>
          {projectRows.map((r) => (
            <tr key={r.key}>
              <td>{r.key}</td>
              <td>{formatTokens(r.totalTokens)}</td>
              <td>{r.requests}</td>
              <td>{formatCost(r.cost, false)}</td>
            </tr>
          ))}
        </tbody>
      </table>

      <div style={{ marginTop: 16, color: '#8b8b96' }}>
        {scanning ? 'scanning…' : 'idle'}
      </div>
    </div>
  );
}
```

- [ ] **Step 8: Verify the whole suite and the production build are green.**

```bash
npx vitest run
npm run build
```

Expected: `npx vitest run` → all tests pass (2 files, 13 tests). `npm run build` → `tsc` reports no errors and `vite build` writes `dist/` with exit code 0.

- [ ] **Step 9: Visual check — `npm run tauri dev`.**

```bash
npm run tauri dev
```

Expected: the TokenLedger window opens; the footer briefly reads `scanning…` on launch, then `idle`. After the first scan the hero shows a formatted total-tokens number, a request count, and a cost line ("$X.XX", "≥ $X.XX", or "unpriced") with the sub-label "at API list prices — not billed". The tool / model / range / refresh selects change the numbers on selection; "Rescan" re-runs the scan; the "By model" and "By project" tables list rows from the real logs. No red "Error:" line. Close the window to end.

- [ ] **Step 10: Commit.**

```bash
git add src/types.ts src/api.ts src/lib/dateRange.ts src/lib/dateRange.test.ts src/lib/format.ts src/lib/format.test.ts src/App.tsx
git commit -m "feat: frontend foundation — types, api wrappers, date/format utils, App shell"
```


---

### Task 12: FilterBar + HeroCard + StatCards

**Files:**
- Create: `src/components/FilterBar.tsx`
- Create: `src/components/HeroCard.tsx`
- Create: `src/components/StatCards.tsx`
- Modify: `src/App.tsx`

**Interfaces:**
- Consumes (from Task 11 `src/types.ts`, mirroring the serde camelCase IPC structs):
  ```ts
  export type Tool = 'claude' | 'codex' | 'gemini' | 'hermes';
  export type RangePreset = 'today' | '7d' | '30d' | 'all';
  export interface CustomRange { start: string; end: string } // 'YYYY-MM-DD', inclusive
  export type DateRange = RangePreset | CustomRange;
  export interface Summary {
    inputTokens: number; outputTokens: number; cacheReadTokens: number;
    cacheWriteTokens: number; totalTokens: number; requests: number;
    cost: number | null; hasUnpriced: boolean; unpricedModels: string[];
    cacheHitRate: number;
  }
  export interface BreakdownRow {
    key: string; inputTokens: number; outputTokens: number; cacheReadTokens: number;
    cacheWriteTokens: number; totalTokens: number; requests: number; cost: number | null;
  }
  ```
- Consumes (from Task 11 `src/lib/format.ts`):
  ```ts
  export function formatTokens(n: number): string;                       // 1234 -> "1,234"; 1_234_567 -> "1.23M"
  export function formatCost(c: number | null, hasUnpriced: boolean): string; // null -> "unpriced"; hasUnpriced -> "≥ $X.XX"; else "$X.XX"
  ```
- Consumes (from Task 11 `src/App.tsx`): App holds the state shape
  `{ tool: Tool | 'all', model: string | 'all', range: DateRange, refreshSec: 0 | 30 | 60 }`
  with setters `setTool`/`setModel`/`setRange`/`setRefreshSec`, plus fetched data
  `summary: Summary | null` and `modelBreakdown: BreakdownRow[]` (the
  `breakdown('model', …)` result).
- Produces (later tasks + App consume these default exports and prop types):
  ```ts
  // FilterBar.tsx
  export interface FilterBarProps {
    tool: Tool | 'all'; model: string | 'all'; range: DateRange; refreshSec: 0 | 30 | 60;
    modelOptions: string[];
    onToolChange: (tool: Tool | 'all') => void;
    onModelChange: (model: string | 'all') => void;
    onRangeChange: (range: DateRange) => void;
    onRefreshChange: (sec: 0 | 30 | 60) => void;
  }
  export default function FilterBar(props: FilterBarProps): JSX.Element;
  // HeroCard.tsx
  export interface HeroCardProps { summary: Summary | null }
  export default function HeroCard(props: HeroCardProps): JSX.Element;
  // StatCards.tsx
  export interface StatCardsProps { summary: Summary | null }
  export default function StatCards(props: StatCardsProps): JSX.Element;
  ```

> Presentational task: components are pure functions of props (no data fetching,
> no IPC). Class names are functional placeholders; the dark theme + grid land in
> Task 15 (`index.css`). No unit tests — verification is typecheck/build + a
> visual check in `npm run tauri dev`.

- [ ] **Step 1: Create `src/components/FilterBar.tsx`**

Single-select segmented tool control (All + 4), model dropdown fed from
breakdown-by-model keys, date-range presets with native `<input type="date">`
custom inputs, and a refresh-interval select. `isCustom` narrows `DateRange` by
`typeof === 'object'`.

```tsx
import type { Tool, DateRange, CustomRange, RangePreset } from '../types';

const TOOLS: Array<{ value: Tool | 'all'; label: string }> = [
  { value: 'all', label: 'All' },
  { value: 'claude', label: 'Claude' },
  { value: 'codex', label: 'Codex' },
  { value: 'gemini', label: 'Gemini' },
  { value: 'hermes', label: 'Hermes' },
];

const PRESETS: Array<{ value: RangePreset; label: string }> = [
  { value: 'today', label: 'Today' },
  { value: '7d', label: '7d' },
  { value: '30d', label: '30d' },
  { value: 'all', label: 'All' },
];

export interface FilterBarProps {
  tool: Tool | 'all';
  model: string | 'all';
  range: DateRange;
  refreshSec: 0 | 30 | 60;
  modelOptions: string[];
  onToolChange: (tool: Tool | 'all') => void;
  onModelChange: (model: string | 'all') => void;
  onRangeChange: (range: DateRange) => void;
  onRefreshChange: (sec: 0 | 30 | 60) => void;
}

function isCustom(range: DateRange): range is CustomRange {
  return typeof range === 'object';
}

function todayLocal(): string {
  const d = new Date();
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return `${d.getFullYear()}-${m}-${day}`;
}

export default function FilterBar({
  tool,
  model,
  range,
  refreshSec,
  modelOptions,
  onToolChange,
  onModelChange,
  onRangeChange,
  onRefreshChange,
}: FilterBarProps) {
  const custom = isCustom(range);
  const activePreset = custom ? 'custom' : range;

  return (
    <div className="filter-bar">
      <div className="segmented" role="group" aria-label="Tool">
        {TOOLS.map((t) => (
          <button
            key={t.value}
            type="button"
            className={tool === t.value ? 'seg active' : 'seg'}
            onClick={() => onToolChange(t.value)}
          >
            {t.label}
          </button>
        ))}
      </div>

      <select
        className="filter-select"
        aria-label="Model"
        value={model}
        onChange={(e) => onModelChange(e.target.value)}
      >
        <option value="all">All models</option>
        {modelOptions.map((m) => (
          <option key={m} value={m}>
            {m}
          </option>
        ))}
      </select>

      <div className="segmented" role="group" aria-label="Date range">
        {PRESETS.map((p) => (
          <button
            key={p.value}
            type="button"
            className={activePreset === p.value ? 'seg active' : 'seg'}
            onClick={() => onRangeChange(p.value)}
          >
            {p.label}
          </button>
        ))}
        <button
          type="button"
          className={activePreset === 'custom' ? 'seg active' : 'seg'}
          onClick={() =>
            onRangeChange(custom ? range : { start: todayLocal(), end: todayLocal() })
          }
        >
          Custom
        </button>
      </div>

      {custom && (
        <div className="custom-range">
          <input
            type="date"
            aria-label="Start date"
            value={range.start}
            max={range.end}
            onChange={(e) => onRangeChange({ ...range, start: e.target.value })}
          />
          <span className="range-sep">→</span>
          <input
            type="date"
            aria-label="End date"
            value={range.end}
            min={range.start}
            onChange={(e) => onRangeChange({ ...range, end: e.target.value })}
          />
        </div>
      )}

      <select
        className="filter-select"
        aria-label="Refresh interval"
        value={refreshSec}
        onChange={(e) => onRefreshChange(Number(e.target.value) as 0 | 30 | 60)}
      >
        <option value={0}>Refresh off</option>
        <option value={30}>Refresh 30s</option>
        <option value={60}>Refresh 60s</option>
      </select>
    </div>
  );
}
```

- [ ] **Step 2: Create `src/components/HeroCard.tsx`**

Big total-tokens number, request count, and est. cost. Cost text follows the
exact UI copy: `unpriced` when `cost === null`; `≥ $X.XX · N unpriced models`
when priced tokens exist alongside unpriced ones; plain `$X.XX` otherwise. The
sub-label `at API list prices — not billed` is permanent.

```tsx
import type { Summary } from '../types';
import { formatTokens, formatCost } from '../lib/format';

export interface HeroCardProps {
  summary: Summary | null;
}

export default function HeroCard({ summary }: HeroCardProps) {
  if (!summary) {
    return (
      <div className="hero-card">
        <div className="hero-label">Total tokens</div>
        <div className="hero-number">—</div>
      </div>
    );
  }

  const costText =
    summary.cost !== null && summary.hasUnpriced
      ? `${formatCost(summary.cost, true)} · ${summary.unpricedModels.length} unpriced models`
      : formatCost(summary.cost, summary.hasUnpriced);

  return (
    <div className="hero-card">
      <div className="hero-label">Total tokens</div>
      <div className="hero-number">{formatTokens(summary.totalTokens)}</div>
      <div className="hero-meta">
        <span className="hero-requests">{formatTokens(summary.requests)} requests</span>
        <span className="hero-cost">
          <span className="hero-cost-label">Est. cost</span>
          <span className="hero-cost-value">{costText}</span>
          <span className="hero-cost-sub">at API list prices — not billed</span>
        </span>
      </div>
    </div>
  );
}
```

- [ ] **Step 3: Create `src/components/StatCards.tsx`**

Four token-category cards plus a cache-hit-rate card with a progress bar. Rate is
`cacheHitRate` (0..1) rendered as a rounded percentage; the bar width is inline
(functional pre-theme). `—` shown while `summary` is null.

```tsx
import type { Summary } from '../types';
import { formatTokens } from '../lib/format';

export interface StatCardsProps {
  summary: Summary | null;
}

function StatCard({ label, value }: { label: string; value: string }) {
  return (
    <div className="stat-card">
      <div className="stat-label">{label}</div>
      <div className="stat-value">{value}</div>
    </div>
  );
}

export default function StatCards({ summary }: StatCardsProps) {
  const s = summary;
  const pct = s ? Math.round(s.cacheHitRate * 100) : 0;

  return (
    <div className="stat-cards">
      <StatCard label="Input" value={s ? formatTokens(s.inputTokens) : '—'} />
      <StatCard label="Output" value={s ? formatTokens(s.outputTokens) : '—'} />
      <StatCard label="Cache write" value={s ? formatTokens(s.cacheWriteTokens) : '—'} />
      <StatCard label="Cache read" value={s ? formatTokens(s.cacheReadTokens) : '—'} />
      <div className="stat-card">
        <div className="stat-label">Cache hit rate</div>
        <div className="stat-value">{s ? `${pct}%` : '—'}</div>
        <div className="progress">
          <div className="progress-fill" style={{ width: `${pct}%` }} />
        </div>
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Wire the components into `src/App.tsx`**

In `src/App.tsx` (created in Task 11), add these imports near the top:

```tsx
import FilterBar from './components/FilterBar';
import HeroCard from './components/HeroCard';
import StatCards from './components/StatCards';
```

Then, inside the App component body, derive the model dropdown options from the
`breakdown('model', …)` result (Task 11 stores it as `modelBreakdown`):

```tsx
const modelOptions = modelBreakdown.map((r) => r.key);
```

Replace the Task-11 placeholder filter/hero/stat `<div>`s in the returned JSX
with the wired components (keep any surrounding loading/error markup from Task
11 unchanged):

```tsx
<FilterBar
  tool={tool}
  model={model}
  range={range}
  refreshSec={refreshSec}
  modelOptions={modelOptions}
  onToolChange={setTool}
  onModelChange={setModel}
  onRangeChange={setRange}
  onRefreshChange={setRefreshSec}
/>
<HeroCard summary={summary} />
<StatCards summary={summary} />
```

> If Task 11 named a setter/state variable differently, keep its name and pass
> it in the matching prop slot above — the prop names on the components are
> fixed, the App-side identifiers are whatever Task 11 declared for the state
> shape `{ tool, model, range, refreshSec }` + `summary` + `modelBreakdown`.

- [ ] **Step 5: Typecheck + build**

```bash
npm run build
```

Expected: `tsc` reports no type errors and `vite build` completes (`✓ built in …`).
If `tsc` errors with e.g. `Cannot find name 'modelBreakdown'`, the Task-11 App
state name differs — reconcile per the note in Step 4, then re-run.

- [ ] **Step 6: Visual check**

```bash
npm run tauri dev
```

Expected in the running window (unstyled — full dark theme/grid arrives in
Task 15; here we verify structure + interactivity):
- A filter bar with a 5-button tool group (All | Claude | Codex | Gemini |
  Hermes); clicking a tool marks it active and the dashboard re-queries.
- A model dropdown whose options are the model keys from the current data
  (plus `All models`).
- A date-range group (Today | 7d | 30d | All | Custom); clicking `Custom`
  reveals two native date inputs separated by `→`, and picking dates updates
  the data.
- A refresh dropdown (Refresh off / 30s / 60s).
- A hero block showing the total-tokens number, `<N> requests`, `Est. cost`
  with a value line and the sub-label `at API list prices — not billed`. With
  unpriced models in range, the value reads `≥ $X.XX · N unpriced models`; with
  zero priced tokens it reads `unpriced`.
- Five stat cards: Input, Output, Cache write, Cache read, and Cache hit rate
  (percentage + a progress bar whose fill width tracks the rate).

- [ ] **Step 7: Commit**

```bash
git add src/components/FilterBar.tsx src/components/HeroCard.tsx src/components/StatCards.tsx src/App.tsx
git commit -m "feat: add FilterBar, HeroCard, and StatCards components"
```


---

### Task 13: TrendChart + BreakdownTables

**Files:**
- Create: `src/components/TrendChart.tsx`
- Create: `src/components/BreakdownTables.tsx`
- Modify: `src/App.tsx`

**Interfaces:**
- Consumes (from Task 11 `src/types.ts`, mirroring the serde camelCase IPC structs):
  ```ts
  export interface TrendPoint {
    bucket: string;            // day: "2026-07-07"; hour: "2026-07-07 14:00"
    inputTokens: number; outputTokens: number; cacheReadTokens: number;
    cacheWriteTokens: number; totalTokens: number; cost: number;   // cost is always a number (priced-token sum)
  }
  export interface BreakdownRow {
    key: string; inputTokens: number; outputTokens: number; cacheReadTokens: number;
    cacheWriteTokens: number; totalTokens: number; requests: number; cost: number | null;
  }
  ```
- Consumes (from Task 11 `src/lib/format.ts`):
  ```ts
  export function formatTokens(n: number): string;                       // 1234 -> "1,234"; 1_234_567 -> "1.23M"
  export function formatCost(c: number | null, hasUnpriced: boolean): string; // null -> "unpriced"; else "$X.XX"
  ```
- Consumes (from Task 11 `src/App.tsx`): App fetches data via `Promise.all` and
  holds `trend: TrendPoint[]` (the `trend(filters, bucket)` result),
  `modelBreakdown: BreakdownRow[]` (`breakdown('model', …)`), and
  `projectBreakdown: BreakdownRow[]` (`breakdown('project', …)`), plus the filter
  state `range: DateRange` (bucketing is `'hour'` when `range === 'today'`, else
  `'day'`).
- Produces (App consumes these default exports and prop types):
  ```ts
  // TrendChart.tsx
  export interface TrendChartProps { points: TrendPoint[]; bucket: 'day' | 'hour' }
  export default function TrendChart(props: TrendChartProps): JSX.Element;
  // BreakdownTables.tsx
  export interface BreakdownTablesProps { modelRows: BreakdownRow[]; projectRows: BreakdownRow[] }
  export default function BreakdownTables(props: BreakdownTablesProps): JSX.Element;
  ```

> Presentational task (like Task 12): components are pure functions of props — no
> data fetching, no IPC. Class names are functional placeholders; the dark theme,
> grid, and final chart colors land in Task 15 (`index.css`). Chart colors here
> are a local `COLORS` const with the spec palette values so the visual check
> reads correctly; Task 15 may lift them to CSS variables. No unit tests —
> verification is typecheck/build + a visual check in `npm run tauri dev`.

- [ ] **Step 1: Create `src/components/TrendChart.tsx`**

Recharts `ComposedChart`: four stacked `Area`s (input / output / cache read /
cache write) share `stackId="tok"` on the left `tokens` axis; the est.-cost
`Line` uses the right `cost` axis. The X-axis tick formatter shortens the bucket
label — `"14:00"` for hour buckets (the part after the space) and `"MM-DD"` for
day buckets (drop the year). An empty `points` array renders a `No data`
placeholder instead of an axis-only chart.

```tsx
import {
  ComposedChart,
  Area,
  Line,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  Legend,
  ResponsiveContainer,
} from 'recharts';
import type { TrendPoint } from '../types';
import { formatTokens } from '../lib/format';

export interface TrendChartProps {
  points: TrendPoint[];
  bucket: 'day' | 'hour';
}

// Spec palette (docs/superpowers/specs, Task 15); Task 15 may move to CSS vars.
const COLORS = {
  input: '#7c5cff',
  output: '#2fbf71',
  cacheRead: '#3a86ff',
  cacheWrite: '#8b8b96',
  cost: '#ff5c7a',
  grid: '#26262e',
};

export default function TrendChart({ points, bucket }: TrendChartProps) {
  const tickFormat = (v: string) =>
    bucket === 'hour' ? v.split(' ')[1] ?? v : v.slice(5);

  if (points.length === 0) {
    return (
      <div className="trend-chart">
        <h3 className="trend-heading">Usage over time</h3>
        <div className="trend-empty">No data</div>
      </div>
    );
  }

  return (
    <div className="trend-chart">
      <h3 className="trend-heading">Usage over time</h3>
      <ResponsiveContainer width="100%" height={320}>
        <ComposedChart data={points} margin={{ top: 8, right: 16, bottom: 8, left: 8 }}>
          <CartesianGrid strokeDasharray="3 3" stroke={COLORS.grid} />
          <XAxis dataKey="bucket" tickFormatter={tickFormat} tick={{ fontSize: 12 }} />
          <YAxis
            yAxisId="tokens"
            tickFormatter={(v) => formatTokens(v as number)}
            tick={{ fontSize: 12 }}
          />
          <YAxis
            yAxisId="cost"
            orientation="right"
            tickFormatter={(v) => `$${(v as number).toFixed(0)}`}
            tick={{ fontSize: 12 }}
          />
          <Tooltip
            formatter={(value, name) =>
              name === 'Est. cost'
                ? `$${(value as number).toFixed(2)}`
                : formatTokens(value as number)
            }
          />
          <Legend />
          <Area
            yAxisId="tokens"
            type="monotone"
            dataKey="inputTokens"
            name="Input"
            stackId="tok"
            stroke={COLORS.input}
            fill={COLORS.input}
            fillOpacity={0.7}
          />
          <Area
            yAxisId="tokens"
            type="monotone"
            dataKey="outputTokens"
            name="Output"
            stackId="tok"
            stroke={COLORS.output}
            fill={COLORS.output}
            fillOpacity={0.7}
          />
          <Area
            yAxisId="tokens"
            type="monotone"
            dataKey="cacheReadTokens"
            name="Cache read"
            stackId="tok"
            stroke={COLORS.cacheRead}
            fill={COLORS.cacheRead}
            fillOpacity={0.7}
          />
          <Area
            yAxisId="tokens"
            type="monotone"
            dataKey="cacheWriteTokens"
            name="Cache write"
            stackId="tok"
            stroke={COLORS.cacheWrite}
            fill={COLORS.cacheWrite}
            fillOpacity={0.7}
          />
          <Line
            yAxisId="cost"
            type="monotone"
            dataKey="cost"
            name="Est. cost"
            stroke={COLORS.cost}
            strokeWidth={2}
            dot={false}
          />
        </ComposedChart>
      </ResponsiveContainer>
    </div>
  );
}
```

- [ ] **Step 2: Create `src/components/BreakdownTables.tsx`**

Two tables sharing one row/table layout: **By model** (raw model key shown
verbatim) and **By project** (absolute-path key shown as its basename, with the
full path in the cell's `title` attribute). The `Cache` column combines
`cacheReadTokens + cacheWriteTokens`. Cost cells reuse `formatCost` —
`formatCost(cost, false)` yields `"unpriced"` when `cost === null`, else
`"$X.XX"`. An empty table shows a single `No data` row. Rows arrive already
sorted by `total_tokens` desc from the backend.

```tsx
import type { BreakdownRow } from '../types';
import { formatTokens, formatCost } from '../lib/format';

export interface BreakdownTablesProps {
  modelRows: BreakdownRow[];
  projectRows: BreakdownRow[];
}

function basename(path: string): string {
  if (path === 'unknown') return 'unknown';
  const parts = path.split('/').filter(Boolean);
  return parts.length ? parts[parts.length - 1] : path;
}

function Row({ row, label, title }: { row: BreakdownRow; label: string; title?: string }) {
  return (
    <tr>
      <td className="bt-key" title={title}>
        {label}
      </td>
      <td className="bt-num">{formatTokens(row.inputTokens)}</td>
      <td className="bt-num">{formatTokens(row.outputTokens)}</td>
      <td className="bt-num">{formatTokens(row.cacheReadTokens + row.cacheWriteTokens)}</td>
      <td className="bt-num">{formatTokens(row.requests)}</td>
      <td className="bt-num">{formatCost(row.cost, false)}</td>
    </tr>
  );
}

function Table({
  heading,
  keyLabel,
  rows,
  isProject,
}: {
  heading: string;
  keyLabel: string;
  rows: BreakdownRow[];
  isProject: boolean;
}) {
  return (
    <div className="breakdown-table">
      <h3 className="bt-heading">{heading}</h3>
      <table>
        <thead>
          <tr>
            <th>{keyLabel}</th>
            <th>Input</th>
            <th>Output</th>
            <th>Cache</th>
            <th>Requests</th>
            <th>Est. cost</th>
          </tr>
        </thead>
        <tbody>
          {rows.length === 0 ? (
            <tr>
              <td className="bt-empty" colSpan={6}>
                No data
              </td>
            </tr>
          ) : (
            rows.map((r) =>
              isProject ? (
                <Row key={r.key} row={r} label={basename(r.key)} title={r.key} />
              ) : (
                <Row key={r.key} row={r} label={r.key} />
              ),
            )
          )}
        </tbody>
      </table>
    </div>
  );
}

export default function BreakdownTables({ modelRows, projectRows }: BreakdownTablesProps) {
  return (
    <div className="breakdown-tables">
      <Table heading="By model" keyLabel="Model" rows={modelRows} isProject={false} />
      <Table heading="By project" keyLabel="Project" rows={projectRows} isProject={true} />
    </div>
  );
}
```

- [ ] **Step 3: Wire the components into `src/App.tsx`**

In `src/App.tsx` (created in Task 11), add these imports near the top:

```tsx
import TrendChart from './components/TrendChart';
import BreakdownTables from './components/BreakdownTables';
```

Inside the App component body, derive the trend bucket from the current range
(the same rule Task 11 uses when calling `trend(filters, bucket)` — `'hour'`
only for the `today` preset):

```tsx
const trendBucket: 'day' | 'hour' = range === 'today' ? 'hour' : 'day';
```

Replace the Task-11 placeholder trend/breakdown `<div>`s in the returned JSX
(below the `<StatCards />` wired in Task 12) with the wired components (keep any
surrounding loading/error markup from Task 11 unchanged):

```tsx
<TrendChart points={trend} bucket={trendBucket} />
<BreakdownTables modelRows={modelBreakdown} projectRows={projectBreakdown} />
```

> If Task 11 named the fetched-data state differently, keep its names and pass
> them into the matching prop slots above — the component prop names are fixed,
> the App-side identifiers are whatever Task 11 declared for `trend`,
> `modelBreakdown`, `projectBreakdown`, and the `range` filter state.

- [ ] **Step 4: Typecheck + build**

```bash
npm run build
```

Expected: `tsc` reports no type errors and `vite build` completes
(`✓ built in …`). `recharts` resolves (installed in Task 1). If `tsc` errors
with e.g. `Cannot find name 'projectBreakdown'`, the Task-11 App state name
differs — reconcile per the note in Step 3, then re-run.

- [ ] **Step 5: Visual check**

```bash
npm run tauri dev
```

Expected in the running window (unstyled — full dark theme/grid arrives in
Task 15; here we verify structure + data binding):
- A `Usage over time` chart: four stacked areas (Input / Output / Cache read /
  Cache write) filling the left Y-axis, a cost line on a right-hand Y-axis, an
  X-axis of shortened bucket labels (`MM-DD` by default; `HH:00` when the range
  is `Today`), a legend naming all five series, and a tooltip showing token
  counts (and `$X.XX` for `Est. cost`). Switching the range to `Today` re-buckets
  to hourly labels; an empty result shows `No data`.
- Two tables below it — `By model` and `By project` — each with columns
  Model/Project, Input, Output, Cache, Requests, Est. cost. Project rows show the
  path basename with the full absolute path on hover (`title`); model rows show
  the raw model string verbatim. A row for an unpriced model/project shows
  `unpriced` in its cost cell. Both tables track the active filters.

- [ ] **Step 6: Commit**

```bash
git add src/components/TrendChart.tsx src/components/BreakdownTables.tsx src/App.tsx
git commit -m "feat: add TrendChart and BreakdownTables components"
```


---

### Task 14: StatusFooter + PriceOverrideDialog

**Files:**
- Create: `src/components/StatusFooter.tsx`
- Create: `src/components/PriceOverrideDialog.tsx`
- Modify: `src/App.tsx`

**Interfaces:**
- Consumes (from Task 11 `src/types.ts`, mirroring the serde camelCase IPC structs):
  ```ts
  export interface SourceStatus {
    source: string;          // "claude" | "codex" | "gemini" | "hermes"
    eventsInserted: number;
    linesSkipped: number;
    error: string | null;
  }
  export interface ScanStatus {
    sources: SourceStatus[];
    scannedAt: number;       // epoch seconds
  }
  export interface OverrideRates {
    input: number | null;
    output: number | null;
    cacheRead: number | null;
    cacheWrite: number | null; // all per-token; null = 0 on the Rust side
  }
  export interface Summary {
    inputTokens: number; outputTokens: number; cacheReadTokens: number;
    cacheWriteTokens: number; totalTokens: number; requests: number;
    cost: number | null; hasUnpriced: boolean; unpricedModels: string[];
    cacheHitRate: number;
  }
  ```
- Consumes (from Task 11 `src/api.ts`, thin invoke wrappers over the Task-10 commands):
  ```ts
  export function setPriceOverride(model: string, rates: OverrideRates): Promise<void>;
  export function deletePriceOverride(model: string): Promise<void>;
  ```
- Consumes (from Task 11 `src/App.tsx`): App holds fetched data and scan state.
  This task expects these identifiers to exist in the App body:
  `summary: Summary | null`, `scanStatus: ScanStatus | null`, `scanning: boolean`,
  and `refetch: () => void` (the function that re-runs the `Promise.all` of
  summary/trend/breakdown fetches). See Step 3's reconciliation note if Task 11
  named them differently or did not persist `scanStatus` / `scanning`.
- Produces (App consumes these default exports and prop types):
  ```ts
  // StatusFooter.tsx
  export interface StatusFooterProps {
    scanStatus: ScanStatus | null;
    scanning: boolean;
    unpricedModels: string[];
    onSetPrice: (model: string) => void;
  }
  export default function StatusFooter(props: StatusFooterProps): JSX.Element;
  // PriceOverrideDialog.tsx
  export interface PriceOverrideDialogProps {
    model: string;
    onSave: (model: string, rates: OverrideRates) => void;
    onDelete: (model: string) => void;
    onClose: () => void;
  }
  export default function PriceOverrideDialog(props: PriceOverrideDialogProps): JSX.Element;
  ```

> Presentational task: both components are pure functions of props (no data
> fetching, no IPC — the IPC calls and refetch live in App). Class names are
> functional placeholders; the dark theme lands in Task 15 (`index.css`). No
> unit tests — verification is typecheck/build + a visual check in
> `npm run tauri dev`.

- [ ] **Step 1: Create `src/components/StatusFooter.tsx`**

Footer shows the scan state (`scanning…` while a scan is in flight, else
`last scan <relative time>`), per-source inserted/skipped counts with the error
surfaced as a tooltip, and the unpriced-models list where each model has a
`set price…` action. The exact UI copy strings (`scanning…`, `last scan …`,
`set price…`) come from the spec.

```tsx
import type { ScanStatus } from '../types';

export interface StatusFooterProps {
  scanStatus: ScanStatus | null;
  scanning: boolean;
  unpricedModels: string[];
  onSetPrice: (model: string) => void;
}

function relativeTime(epochSec: number, nowMs: number): string {
  const diff = Math.max(0, Math.floor(nowMs / 1000) - epochSec);
  if (diff < 60) return `${diff}s ago`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  return `${Math.floor(diff / 86400)}d ago`;
}

export default function StatusFooter({
  scanStatus,
  scanning,
  unpricedModels,
  onSetPrice,
}: StatusFooterProps) {
  const scanText = scanning
    ? 'scanning…'
    : scanStatus
    ? `last scan ${relativeTime(scanStatus.scannedAt, Date.now())}`
    : 'not scanned yet';

  return (
    <div className="status-footer">
      <div className="footer-scan">{scanText}</div>

      <div className="footer-sources">
        {scanStatus?.sources.map((s) => (
          <span
            key={s.source}
            className={s.error ? 'source-stat error' : 'source-stat'}
            title={s.error ?? undefined}
          >
            {s.source}: {s.eventsInserted} in / {s.linesSkipped} skipped
            {s.error ? ' · error' : ''}
          </span>
        ))}
      </div>

      {unpricedModels.length > 0 && (
        <div className="footer-unpriced">
          <span className="footer-unpriced-label">Unpriced models:</span>
          {unpricedModels.map((m) => (
            <span key={m} className="unpriced-model">
              {m}
              <button
                type="button"
                className="set-price"
                onClick={() => onSetPrice(m)}
              >
                set price…
              </button>
            </span>
          ))}
        </div>
      )}
    </div>
  );
}
```

- [ ] **Step 2: Create `src/components/PriceOverrideDialog.tsx`**

A small modal with four numeric inputs labeled `$ / 1M tokens`
(input/output/cache read/cache write). On save it converts each entered
per-1M-tokens value to a per-token rate (`perTok = perM / 1_000_000`); a blank
field becomes `null` (the Rust side treats `None` as 0). Delete removes any
existing override for the model. There is no fetch-override IPC command, so the
fields open blank — entering values sets a fresh override; the `* 1e6` display
direction is the inverse of the save conversion and is not needed here since
nothing is prefilled.

```tsx
import { useState } from 'react';
import type { OverrideRates } from '../types';

export interface PriceOverrideDialogProps {
  model: string;
  onSave: (model: string, rates: OverrideRates) => void;
  onDelete: (model: string) => void;
  onClose: () => void;
}

// $ per 1M tokens (display) -> per-token (stored). Blank / invalid -> null.
function toPerTok(perM: string): number | null {
  const v = parseFloat(perM);
  return Number.isFinite(v) ? v / 1_000_000 : null;
}

export default function PriceOverrideDialog({
  model,
  onSave,
  onDelete,
  onClose,
}: PriceOverrideDialogProps) {
  const [input, setInput] = useState('');
  const [output, setOutput] = useState('');
  const [cacheRead, setCacheRead] = useState('');
  const [cacheWrite, setCacheWrite] = useState('');

  const handleSave = () => {
    onSave(model, {
      input: toPerTok(input),
      output: toPerTok(output),
      cacheRead: toPerTok(cacheRead),
      cacheWrite: toPerTok(cacheWrite),
    });
  };

  return (
    <div className="dialog-backdrop" onClick={onClose}>
      <div
        className="dialog"
        role="dialog"
        aria-label={`Set price for ${model}`}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="dialog-title">Set price for {model}</div>
        <div className="dialog-sub">$ / 1M tokens</div>

        <label className="dialog-field">
          <span>Input</span>
          <input
            type="number"
            min="0"
            step="any"
            value={input}
            onChange={(e) => setInput(e.target.value)}
          />
        </label>
        <label className="dialog-field">
          <span>Output</span>
          <input
            type="number"
            min="0"
            step="any"
            value={output}
            onChange={(e) => setOutput(e.target.value)}
          />
        </label>
        <label className="dialog-field">
          <span>Cache read</span>
          <input
            type="number"
            min="0"
            step="any"
            value={cacheRead}
            onChange={(e) => setCacheRead(e.target.value)}
          />
        </label>
        <label className="dialog-field">
          <span>Cache write</span>
          <input
            type="number"
            min="0"
            step="any"
            value={cacheWrite}
            onChange={(e) => setCacheWrite(e.target.value)}
          />
        </label>

        <div className="dialog-actions">
          <button
            type="button"
            className="dialog-delete"
            onClick={() => onDelete(model)}
          >
            Delete override
          </button>
          <button type="button" className="dialog-cancel" onClick={onClose}>
            Cancel
          </button>
          <button type="button" className="dialog-save" onClick={handleSave}>
            Save
          </button>
        </div>
      </div>
    </div>
  );
}
```

- [ ] **Step 3: Wire the components into `src/App.tsx`**

Add these imports near the top of `src/App.tsx`:

```tsx
import StatusFooter from './components/StatusFooter';
import PriceOverrideDialog from './components/PriceOverrideDialog';
import { setPriceOverride, deletePriceOverride } from './api';
import type { OverrideRates } from './types';
```

Add dialog state and the save/delete handlers inside the App component body
(alongside the other `useState` hooks from Task 11):

```tsx
const [dialogModel, setDialogModel] = useState<string | null>(null);

const handleSaveOverride = async (model: string, rates: OverrideRates) => {
  await setPriceOverride(model, rates);
  setDialogModel(null);
  refetch();
};

const handleDeleteOverride = async (model: string) => {
  await deletePriceOverride(model);
  setDialogModel(null);
  refetch();
};
```

Add the footer and dialog to the returned JSX (footer at the bottom of the
dashboard; the dialog renders only when a model is selected):

```tsx
<StatusFooter
  scanStatus={scanStatus}
  scanning={scanning}
  unpricedModels={summary?.unpricedModels ?? []}
  onSetPrice={setDialogModel}
/>
{dialogModel && (
  <PriceOverrideDialog
    model={dialogModel}
    onSave={handleSaveOverride}
    onDelete={handleDeleteOverride}
    onClose={() => setDialogModel(null)}
  />
)}
```

> Reconciliation with Task 11: this task consumes `summary`, `scanStatus`,
> `scanning`, and `refetch` from App.
> - If Task 11 named the data-fetch function something other than `refetch`
>   (e.g. `loadData`), call that name in the two handlers above.
> - If Task 11 did not persist the scan result, add
>   `const [scanStatus, setScanStatus] = useState<ScanStatus | null>(null);`
>   and `const [scanning, setScanning] = useState(false);`
>   (import `ScanStatus` from `./types`), then in the existing scan runner set
>   `setScanning(true)` before `await scan()`, store its result with
>   `setScanStatus(result)`, and `setScanning(false)` in a `finally` — then
>   `refetch()` so the fresh events show. The component prop names are fixed;
>   only the App-side identifiers vary.

- [ ] **Step 4: Typecheck + build**

```bash
npm run build
```

Expected: `tsc` reports no type errors and `vite build` completes
(`✓ built in …`). If `tsc` errors with e.g. `Cannot find name 'scanStatus'`
or `Cannot find name 'refetch'`, the Task-11 App identifiers differ — reconcile
per the Step 3 note, then re-run.

- [ ] **Step 5: Visual check**

```bash
npm run tauri dev
```

Expected in the running window (unstyled — full dark theme arrives in Task 15;
here we verify structure + interactivity):
- A status footer at the bottom reading `scanning…` while the launch scan runs,
  then `last scan <N>s ago` once it resolves.
- Per-source stats after the scan: one entry per source
  (`claude: <n> in / <n> skipped`, likewise codex/gemini/hermes); a source with
  an error shows a trailing `· error` and reveals the error message on hover.
- An `Unpriced models:` list (Hermes custom-endpoint models and
  `codex-auto-review` appear here on this machine), each followed by a
  `set price…` button.
- Clicking `set price…` opens a dialog titled `Set price for <model>` with the
  sub-label `$ / 1M tokens` and four numeric fields (Input, Output, Cache read,
  Cache write) plus `Delete override` / `Cancel` / `Save`. Entering e.g. `3`
  for Input and `15` for Output and clicking `Save` closes the dialog and the
  data refreshes (the model leaves the unpriced list and its cost appears in the
  breakdowns). `Delete override` returns it to unpriced. Clicking the backdrop
  or `Cancel` closes without saving.

- [ ] **Step 6: Commit**

```bash
git add src/components/StatusFooter.tsx src/components/PriceOverrideDialog.tsx src/App.tsx
git commit -m "feat: add StatusFooter and PriceOverrideDialog components"
```


---

### Task 15: Dark theme polish

Presentational task. Deliverable is the complete dark stylesheet in
`src/index.css` (the app's single global stylesheet, created empty/default by
the Task 1 scaffold). It defines the theme's CSS variables, the Inter/system
font stack, the card grid, tabular numerals, and the chart color palette that
the Recharts series in `TrendChart.tsx` (Task 13) read via CSS variables.

Theming is done primarily with **element selectors** (`body`, `button`,
`select`, `input`, `table`, `th`, `td`) plus a small documented **class
contract** the Task 12–14 components use (`.app`, `.card`, `.filter-bar`,
`.segmented`, `.hero`, `.stat-grid`, `.stat-card`, `.progress`,
`.breakdown-grid`, `.num`, `.status-footer`, `.dialog-backdrop`, `.dialog`).
This keeps the theme robust and self-contained: numbers get `tabular-nums`
from `body`, form controls and tables are styled by tag, and cards are the one
class every panel carries.

**Files:**
- Modify: `src/index.css`

**Interfaces:**
- Consumes: nothing at runtime (no IPC). Class-name contract used by the
  Task 12–14 components (see class list above). Recharts series in
  `TrendChart.tsx` reference the chart palette via CSS variables:
  `var(--chart-input)`, `var(--chart-output)`, `var(--chart-cache-read)`,
  `var(--chart-cache-write)`, `var(--chart-cost)` (passed as Recharts
  `stroke`/`fill` props — modern SVG resolves CSS custom properties).
- Produces: the CSS variable set on `:root` (theme + chart palette) and the
  styled class contract above; no exported symbols.

- [ ] **Step 1: Replace `src/index.css` with the complete dark theme**

Overwrite the entire file with this content:

```css
/* TokenLedger — dark theme (single screen, dark-only) */

:root {
  /* surface + text */
  --bg: #0a0a0c;
  --card: #131318;
  --border: #26262e;
  --text: #e8e8ee;
  --muted: #8b8b96;

  /* brand */
  --accent: #7c5cff;
  --green: #2fbf71;
  --cost: #ff5c7a;

  /* chart palette (read via props by TrendChart / Recharts) */
  --chart-input: #7c5cff;
  --chart-output: #2fbf71;
  --chart-cache-read: #3aa0ff;
  --chart-cache-write: #f0a03c;
  --chart-cost: #ff5c7a;

  --radius: 10px;
  --font: Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto,
    Helvetica, Arial, sans-serif;
}

* {
  box-sizing: border-box;
}

html,
body,
#root {
  height: 100%;
}

body {
  margin: 0;
  background: var(--bg);
  color: var(--text);
  font-family: var(--font);
  font-size: 14px;
  line-height: 1.45;
  /* every number in the app aligns in columns */
  font-variant-numeric: tabular-nums;
  -webkit-font-smoothing: antialiased;
  text-rendering: optimizeLegibility;
}

/* ---- layout ---- */

.app {
  max-width: 1180px;
  margin: 0 auto;
  padding: 20px 24px 40px;
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.card {
  background: var(--card);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 16px 18px;
}

/* ---- filter bar ---- */

.filter-bar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 12px;
}

.segmented {
  display: inline-flex;
  background: var(--card);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 3px;
  gap: 3px;
}

.segmented button {
  border: none;
  background: transparent;
  color: var(--muted);
  padding: 6px 14px;
  border-radius: 7px;
  cursor: pointer;
  font: inherit;
}

.segmented button.active {
  background: var(--accent);
  color: #fff;
}

/* form controls (element selectors — no class needed) */

button,
select,
input {
  font: inherit;
  color: var(--text);
}

select,
input {
  background: var(--card);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 7px 10px;
}

select:focus,
input:focus,
.segmented button:focus-visible,
button:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 1px;
}

/* ---- hero card ---- */

.hero {
  display: flex;
  align-items: baseline;
  flex-wrap: wrap;
  gap: 28px;
}

.hero-number {
  font-size: 40px;
  font-weight: 700;
  letter-spacing: -0.02em;
  line-height: 1;
}

.hero-label {
  color: var(--muted);
  font-size: 13px;
}

.hero-cost {
  font-size: 22px;
  font-weight: 600;
  color: var(--green);
}

.hero-sub {
  color: var(--muted);
  font-size: 12px;
}

/* ---- stat cards ---- */

.stat-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
  gap: 12px;
}

.stat-card {
  background: var(--card);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 14px 16px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.stat-label {
  color: var(--muted);
  font-size: 12px;
  text-transform: uppercase;
  letter-spacing: 0.04em;
}

.stat-value {
  font-size: 22px;
  font-weight: 600;
}

.progress {
  height: 6px;
  background: var(--border);
  border-radius: 3px;
  overflow: hidden;
}

.progress-fill {
  height: 100%;
  background: var(--green);
}

/* ---- breakdown tables ---- */

.breakdown-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(340px, 1fr));
  gap: 16px;
}

.breakdown-grid .card {
  overflow-x: auto;
}

table {
  width: 100%;
  border-collapse: collapse;
}

th,
td {
  padding: 7px 10px;
  text-align: left;
  border-bottom: 1px solid var(--border);
  white-space: nowrap;
}

th {
  color: var(--muted);
  font-weight: 500;
  font-size: 12px;
  text-transform: uppercase;
  letter-spacing: 0.03em;
}

tr:last-child td {
  border-bottom: none;
}

/* right-aligned numeric cells */
.num {
  text-align: right;
  font-variant-numeric: tabular-nums;
}

/* ---- status footer ---- */

.status-footer {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 14px;
  color: var(--muted);
  font-size: 12px;
  padding: 12px 18px;
}

.source-status {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.source-status.error {
  color: var(--cost);
}

.link-button {
  background: none;
  border: none;
  color: var(--accent);
  cursor: pointer;
  padding: 0;
  font: inherit;
}

.link-button:hover {
  text-decoration: underline;
}

/* ---- price override dialog ---- */

.dialog-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.6);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 10;
}

.dialog {
  background: var(--card);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 20px 22px;
  width: 360px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.dialog label {
  display: flex;
  flex-direction: column;
  gap: 4px;
  color: var(--muted);
  font-size: 12px;
}

.dialog-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 4px;
}

.dialog-actions button {
  background: var(--card);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 7px 14px;
  cursor: pointer;
}

.dialog-actions button.primary {
  background: var(--accent);
  border-color: var(--accent);
  color: #fff;
}

/* ---- Recharts theming (axes/grid) ---- */

.recharts-cartesian-axis-tick text {
  fill: var(--muted);
  font-size: 11px;
}

.recharts-cartesian-grid line {
  stroke: var(--border);
}

.recharts-tooltip-wrapper .recharts-default-tooltip {
  background: var(--card) !important;
  border: 1px solid var(--border) !important;
  border-radius: 8px;
}
```

- [ ] **Step 2: Type-check / build**

```bash
cd /Users/brianwong/Project/usage && npm run build
```

Expected: `vite build` (which runs `tsc` then bundles) completes with no
errors and emits `dist/`. CSS-only change, so no TypeScript diagnostics from
this file.

- [ ] **Step 3: Visual check**

```bash
cd /Users/brianwong/Project/usage && npm run tauri dev
```

Expected in the launched TokenLedger window (dark-only theme):
- Page background is near-black `#0a0a0c`; every panel is a `#131318` card
  with a thin `#26262e` border and 10px rounded corners.
- Text is off-white `#e8e8ee`; secondary labels are muted grey `#8b8b96`;
  body font is Inter (falling back to the system UI font).
- The tool segmented control shows the selected segment filled purple
  `#7c5cff` with white text; unselected segments are muted grey.
- The hero total renders as a large (40px) bold number and the est. cost is
  green `#2fbf71`, both with tabular (fixed-width) digits so they don't jitter
  on refresh; the "at API list prices — not billed" sub-label is muted.
- Stat cards sit in a responsive row; the cache-hit-rate card shows a green
  progress bar on a grey track.
- The trend chart's stacked areas are purple (input), green (output), blue
  `#3aa0ff` (cache read), amber `#f0a03c` (cache write), with a pink `#ff5c7a`
  cost line on the right axis; axis ticks and gridlines are muted/border grey.
- Breakdown tables have muted uppercase headers, border-separated rows, and
  right-aligned tabular numeric columns.
- The status footer is small muted text; a source reporting an error shows in
  pink `#ff5c7a`; each unpriced model's "set price…" is a purple link-button.

Close the window when the appearance matches.

- [ ] **Step 4: Commit**

```bash
git add src/index.css
git commit -m "feat: dark theme polish — CSS variables, card grid, chart palette, tabular numerals"
```


---

### Task 16: End-to-end verification + README

This task has no new automated tests. The deliverable is (a) a manual
end-to-end verification of the whole app against the real logs on this
machine, (b) a ccusage cross-check of Claude totals, and (c) a `README.md`.
It depends on every prior task being merged and green.

**Files:**
- Create: `README.md`
- Verify (read-only, no edits): the running app via `npm run tauri dev`

**Interfaces:**
- Consumes (already built by Tasks 1–15; no new code — these are the surfaces you exercise):
  - `pub fn run_scan(conn: &mut Connection, roots: &SourceRoots) -> ScanStatus;`
  - `impl SourceRoots { pub fn default_roots() -> Self; }` (under `dirs::home_dir()`)
  - `#[tauri::command] fn scan(state) -> Result<ScanStatus, String>;`
  - `#[tauri::command] fn summary(state, filters: Filters) -> Result<Summary, String>;`
  - `#[tauri::command] fn breakdown(state, by: String, filters: Filters) -> Result<Vec<BreakdownRow>, String>;`
  - `#[tauri::command] fn set_price_override(state, model: String, rates: OverrideRates) -> Result<(), String>;`
  - `#[tauri::command] fn delete_price_override(state, model: String) -> Result<(), String>;`
- Produces: nothing consumed by later tasks (this is the final task).

---

- [ ] **Step 1: Full regression — the whole suite must be green before touching the real app**

  Run every automated check from a clean tree. All three must pass; if any
  fails, stop and fix (or re-run the responsible task) before continuing.

  ```bash
  cd /Users/brianwong/Project/usage
  cargo test --manifest-path src-tauri/Cargo.toml
  npx vitest run
  npm run build
  ```

  Expected: `cargo test` → `test result: ok.` for every module (`claude::`,
  `codex::`, `gemini::`, `hermes::`, `db::`, `pricing::`, `queries::`,
  `scan::`); `vitest` → all `dateRange` and `format` tests pass; `npm run
  build` → `vite build` completes with no TypeScript errors.

- [ ] **Step 2: Capture the ground-truth Claude totals with ccusage (local-time buckets)**

  ccusage reads the same `~/.claude/projects/**/*.jsonl` transcripts and, like
  TokenLedger, buckets in local time — so its all-time totals are the
  reference for the Claude source. Capture them before launching the app so
  you have exact numbers to compare against.

  ```bash
  npx ccusage@latest --json > /tmp/ccusage.json
  # Sum every day's token fields across the whole history:
  python3 - <<'PY'
  import json
  d = json.load(open("/tmp/ccusage.json"))
  days = d.get("daily", d) if isinstance(d, dict) else d
  if isinstance(days, dict): days = days.get("daily", [])
  tot = {"input":0,"output":0,"cache_create":0,"cache_read":0}
  for day in days:
      tot["input"]        += day.get("inputTokens", 0)
      tot["output"]       += day.get("outputTokens", 0)
      tot["cache_create"] += day.get("cacheCreationTokens", 0)
      tot["cache_read"]   += day.get("cacheReadTokens", 0)
  print("ccusage Claude totals (all time):")
  for k, v in tot.items():
      print(f"  {k:13} {v:,}")
  print(f"  {'TOTAL':13} {sum(tot.values()):,}")
  PY
  ```

  Expected: four token subtotals plus a grand total print out. Record these
  five numbers — you compare them in Step 4. (If the `ccusage` JSON shape
  differs, adjust the field names; the point is the four token categories
  summed over all days.)

- [ ] **Step 3: Launch the real app and run the ingestion + display checklist**

  ```bash
  cd /Users/brianwong/Project/usage
  npm run tauri dev
  ```

  The window opens (title **TokenLedger**, 1100×780). It scans on launch;
  the footer shows `scanning…` then flips to `last scan just now`. Set the
  tool filter to **All** and the range to **All**. Walk this checklist,
  ticking each item only after you have visually confirmed it in the running
  app:

  - [ ] **All four sources ingested.** Breakdown-by-tool (or the model table
    filtered per tool) shows non-zero events for **claude**, **codex**,
    **gemini**, and **hermes**. No source silently missing.
  - [ ] **Per-source footer status.** The status footer lists each source's
    inserted / skipped counts. Claude shows some skipped lines (the
    `<synthetic>` all-zero placeholders) and no error. Hermes shows no lock
    error under normal conditions.
  - [ ] **Hero coherence.** Hero total tokens = input + output + cache write +
    cache read (matches the sum of the four stat cards). Requests count is
    populated. Cost carries the sub-label `at API list prices — not billed`.
  - [ ] **Hermes models are unpriced.** The footer's unpriced-models list
    contains the Hermes (self-hosted) model name(s), each with a `set price…`
    action. `codex-auto-review` also legitimately appears here.
  - [ ] **Partial-pricing marker.** With unpriced tokens in range, the hero
    cost renders `≥ $<amount> · <N> unpriced models` (not a bare `$X`).
  - [ ] **Filters are live.** Switching the tool segmented control (e.g. to
    **Codex** only) and the range (Today / 7d / 30d) re-queries and updates
    the hero, stat cards, trend chart, and both breakdown tables. Range =
    **Today** switches the trend chart to hourly buckets.
  - [ ] **Project rollup.** The project breakdown table keys rows by absolute
    path and displays the basename; the same repo used from two tools appears
    as one row (title attribute shows the full path on hover).

- [ ] **Step 4: ccusage cross-check — Claude tokens must match closely**

  In the running app, set tool filter = **Claude**, range = **All**. Read the
  four stat-card token values (input, output, cache write, cache read) and the
  hero total. Compare against the ccusage numbers from Step 2.

  Expected and acceptable:
  - **Token categories match closely.** TokenLedger's cache-write stat = ccusage
    `cacheCreationTokens` (TokenLedger splits it into 5m + 1h internally, but
    the displayed "cache write" stat is the combined 5m + 1h, so the totals
    line up). input / output / cache read should match to within rounding of
    dedup differences.
  - **Small deltas are expected and OK.** Minor divergence can come from
    TokenLedger's durable ledger holding history that Claude Code has since
    pruned (>~30 days old), or from ccusage's own dedup heuristics. A few
    percent is fine; an order-of-magnitude gap is a bug — investigate the
    Claude adapter (Task 3) if you see one.
  - **Cost is expected to differ and is NOT compared.** ccusage applies flat
    cache pricing; TokenLedger prices 5m vs 1h cache writes separately
    (measured ~11.6% higher Claude cost on this machine). Cost divergence is
    by design, not a defect.

  Tick only when the four Claude token categories match ccusage within a few
  percent.

- [ ] **Step 5: Price-override end-to-end, including persistence across restart**

  Still in the running app:

  1. In the footer, click `set price…` on a Hermes (unpriced) model. The
     dialog opens with four numeric inputs labeled `$ / 1M tokens`
     (input / output / cache read / cache write).
  2. Enter e.g. input `3`, output `15`, cache read `0.30`, cache write `3.75`.
     Save.
  3. Confirm the app refetches: that model leaves the unpriced-models footer
     list, the hero cost increases, and the model's breakdown row now shows a
     dollar cost instead of `unpriced`.
  4. **Quit the app entirely** (close the window / stop `npm run tauri dev`),
     then relaunch with `npm run tauri dev`.
  5. Confirm the override **persisted**: the same model still shows a priced
     cost and is absent from the unpriced list on the fresh launch (the
     override lives in the `price_overrides` table in `tokenledger.db`, not in
     memory).
  6. Delete the override (re-open the dialog and clear/delete it). Confirm the
     model returns to the unpriced footer list and its cost cell reverts to
     `unpriced`.

  Tick only after the override survives a full app restart and delete works.

- [ ] **Step 6: Write `README.md`**

  Create `/Users/brianwong/Project/usage/README.md` with exactly this content:

  ````markdown
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
  ````

- [ ] **Step 7: Rebuild once more, then final commit**

  Re-run the build to confirm the tree is still green after adding the README
  (README-only change, but confirm nothing else drifted), then commit.

  ```bash
  cd /Users/brianwong/Project/usage
  npm run build
  git add README.md
  git commit -m "docs: add README and complete end-to-end verification"
  ```

  Expected: `npm run build` succeeds; the commit lands. This is the final
  task — the whole plan (Tasks 1–16) is now complete and merged.


---
