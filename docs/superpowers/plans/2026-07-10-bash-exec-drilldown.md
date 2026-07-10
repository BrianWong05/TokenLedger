# Bash Exec Drill-Down Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a command-level drill-down under Execution → Bash (facet tabs: By type / Executable / Command) per spec `docs/superpowers/specs/2026-07-10-bash-exec-drilldown-design.md`, on branch `feature/context-drilldown`.

**Architecture:** One new `ctx_exec` table (schema v5) stores one row per (file, local day, classified command) with the same clear-on-full-parse idempotency as `ctx_tools`. A new pure module `exec_class.rs` ports TokenTracker's three classifiers (kind / executable / signature) token-wise. One query returns raw rows; the frontend groups them into three facets and allocates the Bash leaf's tokens across each facet with the existing `allocateByWeight`.

**Tech Stack:** Rust (rusqlite, no regex crate — hand-rolled token matching), React + TypeScript, vitest.

## Global Constraints

- **Idempotency contract (same as ctx_tools):** ctx_exec rows keyed `(source_file, day, kind, exe, cmd)`, additive upsert; any parse from byte 0 clears the file's rows FIRST; byte-offset resumes add increments only.
- **Allocation exactness:** each facet's rows sum exactly to the Bash leaf total (largest-remainder via the existing `allocateByWeight`). Raw weights are never displayed.
- **Honesty:** no fabricated facets; a source with no exec rows shows no expansion. NULL/absent → nothing rendered, never 0.
- **Ledger rule:** events immutable; ctx_exec is scan-derived and rebuildable.
- TS mirrors Rust serde camelCase (`est_tokens` → `estTokens`).
- No new dependencies (NO regex crate — token matching only). Commit style `feat(scope):`. Backend tests from `src-tauri/`; frontend from repo root.
- Claude only in v1; classification rules ported in TokenTracker's ORDER (the anywhere-rules for git/http fire before file_mutation/compound).

---

### Task 1: Schema v5 + ctx_exec db helpers

**Files:**
- Modify: `src-tauri/src/db.rs`

**Interfaces:**
- Produces: table `ctx_exec(source, source_file, day, kind, exe, cmd, est_tokens, calls, PK(source_file, day, kind, exe, cmd))`; `PRAGMA user_version = 5`; `pub fn clear_ctx_exec_for_file(conn: &Connection, source_file: &str) -> rusqlite::Result<()>`; `pub fn add_ctx_exec_rows(conn: &mut Connection, source: &str, source_file: &str, rows: &[(String, String, String, i64, i64, i64)]) -> rusqlite::Result<()>` where rows are `(kind, exe, cmd, est_tokens, calls, epoch_ts)`.

- [ ] **Step 1: Write the failing tests** (db.rs tests module)

```rust
    #[test]
    fn v4_db_migrates_to_v5_with_ctx_exec() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        // Build a genuine v4 database by hand.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute_batch(SCHEMA_V2).unwrap();
            conn.execute_batch(SCHEMA_V3).unwrap();
            conn.execute_batch(SCHEMA_V4).unwrap();
            conn.execute(
                "INSERT INTO scanned_files (path, size, mtime, byte_offset) VALUES ('f',1,1,1)",
                [],
            )
            .unwrap();
        }
        let conn = open_db(&path).unwrap();
        let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 5);
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='ctx_exec'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        let files: i64 = conn
            .query_row("SELECT COUNT(*) FROM scanned_files", [], |r| r.get(0))
            .unwrap();
        assert_eq!(files, 0, "scan state cleared for the one-time backfill re-scan");
    }

    #[test]
    fn ctx_exec_rows_accumulate_and_clear_per_file() {
        let (_dir, mut conn) = temp_db();
        let ts = 1_782_907_200i64;
        add_ctx_exec_rows(&mut conn, "claude", "f1.jsonl", &[
            ("git_local".into(), "git".into(), "git add".into(), 100, 1, ts),
            ("git_local".into(), "git".into(), "git add".into(), 40, 0, ts + 60),
            ("test".into(), "npm".into(), "npm test".into(), 30, 1, ts),
        ]).unwrap();
        add_ctx_exec_rows(&mut conn, "claude", "f2.jsonl", &[
            ("git_local".into(), "git".into(), "git add".into(), 7, 1, ts),
        ]).unwrap();
        let (est, calls): (i64, i64) = conn
            .query_row(
                "SELECT est_tokens, calls FROM ctx_exec WHERE source_file='f1.jsonl' AND cmd='git add'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((est, calls), (140, 1), "same day+key accumulates; result adds size only");
        clear_ctx_exec_for_file(&conn, "f1.jsonl").unwrap();
        let left: i64 = conn.query_row("SELECT COUNT(*) FROM ctx_exec", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 1);
    }
```

- [ ] **Step 2: Run to verify failure**

Run (from `src-tauri/`): `cargo test db::tests`
Expected: compile FAIL — `add_ctx_exec_rows` / `clear_ctx_exec_for_file` / `SCHEMA_V4`-chained v5 assertions unresolved.

- [ ] **Step 3: Implement**

After `SCHEMA_V4` in db.rs:

```rust
// v5: Bash command-level facets. One row per (file, local day, classified
// command); kind/exe/cmd are computed at scan time by exec_class. Same
// idempotency contract as ctx_tools: parse-from-byte-0 clears the file's
// rows first; resumes add increments. Scan-state clear forces the one-time
// backfill re-scan. No BEGIN/COMMIT: migrate() wraps the batches.
const SCHEMA_V5: &str = "\
CREATE TABLE IF NOT EXISTS ctx_exec (
  source TEXT NOT NULL,
  source_file TEXT NOT NULL,
  day TEXT NOT NULL,
  kind TEXT NOT NULL,
  exe TEXT NOT NULL,
  cmd TEXT NOT NULL,
  est_tokens INTEGER NOT NULL DEFAULT 0,
  calls INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (source_file, day, kind, exe, cmd)
);
DELETE FROM scanned_files;
DELETE FROM session_ctx;
PRAGMA user_version = 5;";
```

In `migrate()` after the `version < 4` block:

```rust
        if version < 5 {
            conn.execute_batch(SCHEMA_V5)?;
        }
```

Helpers (next to the ctx_tools helpers):

```rust
pub fn clear_ctx_exec_for_file(conn: &Connection, source_file: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM ctx_exec WHERE source_file = ?1", [source_file])?;
    Ok(())
}

/// Additive per-(file, day, kind, exe, cmd) upsert of exec weights. Same
/// idempotency contract as add_ctx_tool_rows: callers clear per file on any
/// parse from byte 0; resumes append increments over fresh bytes only.
pub fn add_ctx_exec_rows(
    conn: &mut Connection,
    source: &str,
    source_file: &str,
    rows: &[(String, String, String, i64, i64, i64)], // (kind, exe, cmd, est, calls, ts)
) -> rusqlite::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO ctx_exec (source, source_file, day, kind, exe, cmd, est_tokens, calls) \
             VALUES (?1, ?2, strftime('%Y-%m-%d', ?3, 'unixepoch', 'localtime'), ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT(source_file, day, kind, exe, cmd) DO UPDATE SET \
               est_tokens = est_tokens + excluded.est_tokens, \
               calls = calls + excluded.calls",
        )?;
        for (kind, exe, cmd, est, calls, ts) in rows {
            stmt.execute(params![source, source_file, ts, kind, exe, cmd, est, calls])?;
        }
    }
    tx.commit()
}
```

Update the SIX existing version assertions from 4 to 5:
`fresh_db_has_tables_and_user_version` (also add `"ctx_exec"` to its table
list), `open_is_idempotent`, `v1_db_migrates_to_v2_preserving_events`,
`v2_db_migrates_to_v3_preserving_events`, `v3_db_migrates_to_v4_with_ctx_tools`,
`concurrent_opens_of_v1_db_both_succeed`.

- [ ] **Step 4: Run tests** — `cargo test` → PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/db.rs
git commit -m "feat(db): schema v5 — ctx_exec command facets with per-file idempotency"
```

---

### Task 2: exec_class — the three classifiers

**Files:**
- Create: `src-tauri/src/adapters/exec_class.rs`
- Modify: `src-tauri/src/adapters/mod.rs` (add `pub mod exec_class;`)

**Interfaces:**
- Produces: `pub fn exec_kind(cmd: &str) -> &'static str`, `pub fn exec_exe(cmd: &str) -> String`, `pub fn exec_cmd(cmd: &str) -> String`.

- [ ] **Step 1: Create the file with tests first** (write the full file below WITHOUT the implementation section, run `cargo test exec_class` to see it fail to compile, then add the implementation)

Complete module:

```rust
// Token-wise port of TokenTracker's exec classifiers (categorizer-utils.js:
// inferExecCommandKind / getExecutableName / sanitizeCommandSignature).
// The regex table is re-expressed over whitespace tokens (trailing ;&|
// trimmed per token); rule ORDER is preserved — the git/http "anywhere"
// rules fire before file_mutation/shell_inspect/compound. The unit tests
// pin equivalence on the rule table's canonical commands.

fn shell_words(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = cmd.chars();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else if q == '"' && c == '\\' {
                    if let Some(n) = chars.next() {
                        cur.push(n);
                    }
                } else {
                    cur.push(c);
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    quote = Some(c);
                } else if c.is_whitespace() {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                } else {
                    cur.push(c);
                }
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

// bash|sh|zsh|fish -lc "<inner>" unwraps to the inner command's words;
// rtk|env|command|xcrun prefixes are stripped recursively.
fn unwrap_shell(words: Vec<String>) -> Vec<String> {
    if words.len() >= 3
        && matches!(words[0].as_str(), "bash" | "sh" | "zsh" | "fish")
        && words[1] == "-lc"
    {
        return shell_words(&words[2..].join(" "));
    }
    if words.len() >= 2 && matches!(words[0].as_str(), "rtk" | "env" | "command" | "xcrun") {
        return unwrap_shell(words[1..].to_vec());
    }
    words
}

fn basename(w: &str) -> String {
    w.rsplit('/').next().unwrap_or(w).to_string()
}

fn is_var_assign(w: &str) -> bool {
    match w.find('=') {
        None => false,
        Some(eq) => {
            let name = &w[..eq];
            !name.is_empty()
                && name.chars().next().is_some_and(|c| c.is_ascii_uppercase() || c == '_')
                && name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        }
    }
}

pub fn exec_exe(cmd: &str) -> String {
    let words = unwrap_shell(shell_words(cmd));
    match words.first() {
        Some(w) if !w.is_empty() => basename(w),
        _ => "unknown".to_string(),
    }
}

pub fn exec_cmd(cmd: &str) -> String {
    let words = unwrap_shell(shell_words(cmd));
    if words.is_empty() {
        return "unknown".to_string();
    }
    let exe = basename(&words[0]);
    let sub = words
        .iter()
        .skip(1)
        .find(|w| !w.is_empty() && !w.starts_with('-') && !is_var_assign(w));
    match sub {
        Some(s) => format!("{exe} {s}"),
        None => exe,
    }
}

// Trailing shell punctuation glued to a token ("ls;", "grep|") must not
// defeat first-word rules.
fn tok(w: &str) -> &str {
    w.trim_end_matches([';', '&', '|'])
}

pub fn exec_kind(raw: &str) -> &'static str {
    let cmd = raw.trim();
    let words: Vec<String> = shell_words(cmd).iter().map(|w| tok(w).to_string()).collect();
    let w0 = words.first().map(String::as_str).unwrap_or("");
    let w1 = words.get(1).map(String::as_str).unwrap_or("");
    let w2 = words.get(2).map(String::as_str).unwrap_or("");

    // package-manager rules (order matters: build before test etc.)
    if matches!(w0, "npm" | "yarn" | "pnpm") {
        let arg = if w1 == "run" { w2 } else { w1 };
        let is_build =
            arg == "build" || arg.starts_with("build:") || arg.ends_with(":build");
        if is_build {
            return "build";
        }
        if w1 == "test" || (w1 == "run" && w2.contains("test")) {
            return "test";
        }
        if w1 == "run" && w2 == "typecheck" {
            return "typecheck";
        }
        if matches!(w1, "install" | "add" | "ci") {
            return "dependency";
        }
        if matches!(w1, "pack" | "publish" | "version") {
            return "package";
        }
        if w1 == "run" && (matches!(w2, "dev" | "serve" | "start") || w2.contains("dev")) {
            return "dev_server";
        }
    }
    let pair = |a: &str, bs: &[&str]| {
        words.windows(2).any(|w| w[0] == a && bs.contains(&w[1].as_str()))
    };
    if pair("node", &["--check"]) {
        return "syntax_check";
    }
    if w0 == "node" && (w1 == "-e" || (w1 == "--input-type=module" && w2 == "-e")) {
        return "node_eval";
    }
    if w0 == "node"
        && words.iter().any(|w| {
            w.contains("query") || w.contains("analyze") || w.contains("report")
        })
    {
        return "node_cli";
    }
    if w0 == "git" && w1 == "status" {
        return "git_status";
    }
    if pair("git", &["push", "pull", "fetch", "clone"]) {
        return "git_remote";
    }
    if pair("git", &["add", "commit", "branch", "config", "remote", "restore"]) {
        return "git_local";
    }
    if words.iter().any(|w| w == "curl" || w == "wget") {
        return "http";
    }
    if matches!(w0, "ps" | "pgrep" | "pkill" | "kill" | "lsof") {
        return "process";
    }
    if w0 == "tmux" {
        return "terminal";
    }
    if matches!(w0, "open" | "osascript") {
        return "browser_control";
    }
    if matches!(w0, "rm" | "mkdir" | "touch" | "chmod" | "cp" | "mv") {
        return "file_mutation";
    }
    if matches!(w0, "pwd" | "ls" | "test") {
        return "shell_inspect";
    }
    if cmd.contains(';') || cmd.contains('&') || cmd.contains('|') {
        return "compound";
    }
    "unknown"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_match_the_ported_rule_table() {
        assert_eq!(exec_kind("npm run build"), "build");
        assert_eq!(exec_kind("pnpm run app:build"), "build");
        assert_eq!(exec_kind("npm test"), "test");
        assert_eq!(exec_kind("npm run test:unit"), "test");
        assert_eq!(exec_kind("npm run typecheck"), "typecheck");
        assert_eq!(exec_kind("npm install"), "dependency");
        assert_eq!(exec_kind("npm publish"), "package");
        assert_eq!(exec_kind("npm run dev"), "dev_server");
        assert_eq!(exec_kind("node --check foo.js"), "syntax_check");
        assert_eq!(exec_kind("node -e 'console.log(1)'"), "node_eval");
        assert_eq!(exec_kind("node scripts/report.js"), "node_cli");
        assert_eq!(exec_kind("git status"), "git_status");
        assert_eq!(exec_kind("git push origin main"), "git_remote");
        assert_eq!(exec_kind("git add ."), "git_local");
        // The anywhere-rules fire before file_mutation/compound.
        assert_eq!(exec_kind("rm x && git add ."), "git_local");
        assert_eq!(exec_kind("echo hi | curl -d @- http://x"), "http");
        assert_eq!(exec_kind("ps aux"), "process");
        assert_eq!(exec_kind("tmux ls"), "terminal");
        assert_eq!(exec_kind("open http://x"), "browser_control");
        assert_eq!(exec_kind("rm -rf dist"), "file_mutation");
        assert_eq!(exec_kind("ls; echo done"), "shell_inspect");
        assert_eq!(exec_kind("cd a && npx tsc"), "compound");
        assert_eq!(exec_kind("cd /some/where"), "unknown");
        assert_eq!(exec_kind(""), "unknown");
        assert_eq!(exec_kind("npx vitest"), "unknown");
    }

    #[test]
    fn exe_and_signature_unwrap_and_skip_flags_and_vars() {
        assert_eq!(exec_exe("git add ."), "git");
        assert_eq!(exec_exe("/usr/bin/python3 x.py"), "python3");
        assert_eq!(exec_exe("bash -lc \"git add .\""), "git");
        assert_eq!(exec_exe("env FOO=1 git add"), "git");
        assert_eq!(exec_exe(""), "unknown");
        assert_eq!(exec_cmd("git add ."), "git add");
        assert_eq!(exec_cmd("npx vitest run"), "npx vitest");
        assert_eq!(exec_cmd("cargo test --release e2e"), "cargo test");
        // Faithful port: TZ=UTC is words[0] (not an unwrap prefix), so it IS
        // the "executable"; the signature scanner then finds "npm" as the
        // first non-flag, non-VAR= subcommand. Same for the env variant,
        // which unwraps "env" and leaves TZ=UTC in front.
        assert_eq!(exec_cmd("TZ=UTC npm test"), "TZ=UTC npm");
        assert_eq!(exec_cmd("env TZ=UTC npm test"), "TZ=UTC npm");
        assert_eq!(exec_cmd("sqlite3"), "sqlite3");
        assert_eq!(exec_cmd("grep -rn foo src"), "grep foo");
    }
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test exec_class` → compile FAIL until the implementation section exists (write tests-only first, or write the whole file and confirm the tests pass on first run — either way run the module's tests in isolation before the full suite).

- [ ] **Step 3: Add `pub mod exec_class;` to `src-tauri/src/adapters/mod.rs`** (alphabetical placement among the existing `pub mod` lines).

- [ ] **Step 4: Run tests** — `cargo test exec_class` then full `cargo test` → PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/adapters/exec_class.rs src-tauri/src/adapters/mod.rs
git commit -m "feat(claude): exec_class — kind/executable/signature classifiers"
```

---

### Task 3: Claude scan writes ctx_exec

**Files:**
- Modify: `src-tauri/src/adapters/claude.rs`, `src-tauri/src/adapters/claude_ctx.rs` (one visibility change)

**Interfaces:**
- Consumes: Task 1 helpers, Task 2 classifiers, `claude_ctx::est` + `claude_ctx::content_bytes` (make the latter `pub`).
- Produces: Claude scans populate `ctx_exec` under the standard idempotency contract.

- [ ] **Step 1: Write the failing test** (claude.rs tests)

```rust
    #[test]
    fn scan_populates_ctx_exec_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open_db(&dir.path().join("t.db")).unwrap();
        let root = dir.path().join("projects");
        let proj = root.join("x");
        std::fs::create_dir_all(&proj).unwrap();
        let logp = proj.join("s8.jsonl");
        let tooluse = r#"{"type":"assistant","sessionId":"s8","requestId":"r1","timestamp":"2026-07-01T10:00:00.000Z","cwd":"/p/x","message":{"id":"m1","model":"claude-opus-4-8","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"git add ."}}],"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        let toolres = r#"{"type":"user","sessionId":"s8","timestamp":"2026-07-01T10:00:01.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"okokokokokokokokokokokokokokokokokokokok"}]}}"#;
        std::fs::write(&logp, format!("{tooluse}\n{toolres}\n")).unwrap();

        scan_claude(&mut conn, &root);
        let (kind, exe, cmd, est1, calls1): (String, String, String, i64, i64) = conn
            .query_row(
                "SELECT kind, exe, cmd, est_tokens, calls FROM ctx_exec WHERE source='claude'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!((kind.as_str(), exe.as_str(), cmd.as_str()), ("git_local", "git", "git add"));
        assert!(est1 > 0, "command + result bytes booked");
        assert_eq!(calls1, 1, "tool_use counts the call; its result adds size only");

        // Unchanged re-scan: resume at EOF, nothing added.
        scan_claude(&mut conn, &root);
        let (est2, calls2): (i64, i64) = conn
            .query_row("SELECT est_tokens, calls FROM ctx_exec", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((est2, calls2), (est1, calls1));

        // Forced full re-parse: rows replaced, not doubled.
        conn.execute("DELETE FROM scanned_files", []).unwrap();
        scan_claude(&mut conn, &root);
        let (est3, calls3): (i64, i64) = conn
            .query_row("SELECT est_tokens, calls FROM ctx_exec", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!((est3, calls3), (est1, calls1));
    }
```

- [ ] **Step 2: Run to verify failure** — `cargo test adapters::claude::tests::scan_populates_ctx_exec_idempotently` → FAIL (no rows).

- [ ] **Step 3: Implement**

In `claude_ctx.rs`, change `fn content_bytes` to `pub fn content_bytes` (no
other change).

In `claude.rs`, add a module-level helper (near `rollup_worktree`):

```rust
// Bash command-level facets (spec 2026-07-10-bash-exec-drilldown): classify
// each Bash tool_use once, remember the classification by tool_use id so the
// paired result's bytes book to the same command. Reads the line only —
// independent of the attribution engine.
fn collect_exec(
    v: &serde_json::Value,
    exec_by_id: &mut HashMap<String, (String, String, String)>,
    exec_rows: &mut Vec<(String, String, String, i64, i64, i64)>,
    line_ts: i64,
) {
    let blocks = match v["message"]["content"].as_array() {
        Some(b) => b,
        None => return,
    };
    for b in blocks {
        match b.get("type").and_then(|t| t.as_str()) {
            Some("tool_use") if b.get("name").and_then(|n| n.as_str()) == Some("Bash") => {
                if let Some(cmd) = b.pointer("/input/command").and_then(|c| c.as_str()) {
                    let kind = exec_class::exec_kind(cmd).to_string();
                    let exe = exec_class::exec_exe(cmd);
                    let sig = exec_class::exec_cmd(cmd);
                    if let Some(id) = b.get("id").and_then(|i| i.as_str()) {
                        exec_by_id
                            .insert(id.to_string(), (kind.clone(), exe.clone(), sig.clone()));
                    }
                    let est = claude_ctx::est(claude_ctx::content_bytes(&b["input"]));
                    exec_rows.push((kind, exe, sig, est, 1, line_ts));
                }
            }
            Some("tool_result") => {
                let hit = b
                    .get("tool_use_id")
                    .and_then(|i| i.as_str())
                    .and_then(|id| exec_by_id.get(id))
                    .cloned();
                if let Some((kind, exe, sig)) = hit {
                    let est = claude_ctx::est(claude_ctx::content_bytes(&b["content"]));
                    exec_rows.push((kind, exe, sig, est, 0, line_ts));
                }
            }
            _ => {}
        }
    }
}
```

Wire into `scan_file`:
- Next to the existing ctx_tools clear (`if start == 0 { ... }`), add
  `crate::db::clear_ctx_exec_for_file(conn, &path_str)?;` inside the same
  `if start == 0` block.
- Next to `tool_rows`, declare:
  `let mut exec_by_id: HashMap<String, (String, String, String)> = HashMap::new();`
  `let mut exec_rows: Vec<(String, String, String, i64, i64, i64)> = Vec::new();`
- In the `Some("user")` arm, after the existing apply/extend lines:
  `collect_exec(&v, &mut exec_by_id, &mut exec_rows, line_ts);`
- In the `Some("assistant")` arm, after the existing apply/extend lines:
  `collect_exec(&v, &mut exec_by_id, &mut exec_rows, line_ts);`
- After `crate::db::add_ctx_tool_rows(...)`:
  `crate::db::add_ctx_exec_rows(conn, "claude", &path_str, &exec_rows)?;`
- Add `use super::exec_class;` (or `use crate::adapters::exec_class;` to
  match the file's existing import style).

- [ ] **Step 4: Run tests** — `cargo test adapters::claude` then full `cargo test` → PASS (existing tests unaffected: none of their fixtures have Bash tool_use blocks except the ctx_tools test, which asserts ctx_tools only — its Bash command `"ls -la"` will also produce a ctx_exec row, harmless).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/adapters/claude.rs src-tauri/src/adapters/claude_ctx.rs
git commit -m "feat(claude): scan populates ctx_exec command facets"
```

---

### Task 4: Query + IPC + TS + api

**Files:**
- Modify: `src-tauri/src/queries.rs`, `src-tauri/src/lib.rs`, `src/types.ts`, `src/api.ts`

**Interfaces:**
- Produces:

```rust
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CtxExecRow {
    pub source: String,
    pub kind: String,
    pub exe: String,
    pub cmd: String,
    pub est_tokens: i64,
    pub calls: i64,
}
pub fn ctx_exec(conn: &Connection, f: &Filters) -> rusqlite::Result<Vec<CtxExecRow>>;
```

TS: `CtxExecRow { source: string; kind: string; exe: string; cmd: string; estTokens: number; calls: number }`; `fetchCtxExec(filters): Promise<CtxExecRow[]>` invoking `'ctx_exec'`.

- [ ] **Step 1: Write the failing query test** (queries.rs tests)

```rust
    #[test]
    fn ctx_exec_sums_by_key_and_range() {
        std::env::set_var("TZ", "UTC");
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        db::add_ctx_exec_rows(&mut conn, "claude", "f1", &[
            ("git_local".into(), "git".into(), "git add".into(), 100, 1, DAY1_TS),
            ("git_local".into(), "git".into(), "git add".into(), 50, 1, DAY2_TS),
            ("test".into(), "npm".into(), "npm test".into(), 30, 1, DAY1_TS),
        ]).unwrap();

        let all = ctx_exec(&conn, &Filters::default()).unwrap();
        let ga = all.iter().find(|r| r.cmd == "git add").unwrap();
        assert_eq!((ga.est_tokens, ga.calls), (150, 2), "summed across days");
        assert_eq!(ga.kind, "git_local");

        let f = Filters { start_ts: Some(DAY1_START), end_ts: Some(DAY2_START), ..Filters::default() };
        let d1 = ctx_exec(&conn, &f).unwrap();
        assert_eq!(d1.iter().find(|r| r.cmd == "git add").unwrap().est_tokens, 100);

        let f2 = Filters { tools: vec!["codex".to_string()], ..Filters::default() };
        assert!(ctx_exec(&conn, &f2).unwrap().is_empty());
    }
```

- [ ] **Step 2: Run to verify failure** — `cargo test queries` → compile FAIL.

- [ ] **Step 3: Implement query** (mirror `ctx_tools` exactly, one more GROUP BY column set):

```rust
// Bash command facets in range: same tool + day-bounds convention as
// ctx_tools/ctx_resources. Ignores model/project (table has neither).
pub fn ctx_exec(conn: &Connection, f: &Filters) -> rusqlite::Result<Vec<CtxExecRow>> {
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<Value> = Vec::new();
    if !f.tools.is_empty() {
        let ph = f.tools.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        clauses.push(format!("source IN ({ph})"));
        for t in &f.tools {
            params.push(Value::Text(t.clone()));
        }
    }
    if let Some(s) = f.start_ts {
        clauses.push("day >= strftime('%Y-%m-%d', ?, 'unixepoch', 'localtime')".to_string());
        params.push(Value::Integer(s));
    }
    if let Some(e) = f.end_ts {
        clauses.push("day <= strftime('%Y-%m-%d', ?, 'unixepoch', 'localtime')".to_string());
        params.push(Value::Integer(e - 1));
    }
    let where_sql = if clauses.is_empty() { String::new() } else { format!("WHERE {}", clauses.join(" AND ")) };
    let sql = format!(
        "SELECT source, kind, exe, cmd, SUM(est_tokens), SUM(calls) FROM ctx_exec {where_sql} \
         GROUP BY source, kind, exe, cmd ORDER BY SUM(est_tokens) DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        Ok(CtxExecRow {
            source: r.get(0)?, kind: r.get(1)?, exe: r.get(2)?, cmd: r.get(3)?,
            est_tokens: r.get(4)?, calls: r.get(5)?,
        })
    })?;
    rows.collect()
}
```

- [ ] **Step 4: IPC + TS** — in lib.rs add the command (identical pattern to `ctx_tools`) and register `ctx_exec` in `generate_handler![...]`; import `CtxExecRow` in the `use queries::{...}` list:

```rust
#[tauri::command]
fn ctx_exec(
    state: State<'_, AppState>,
    filters: Filters,
) -> Result<Vec<queries::CtxExecRow>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    queries::ctx_exec(&db, &filters).map_err(|e| e.to_string())
}
```

types.ts:

```ts
export interface CtxExecRow {
  source: string;
  kind: string;   // classified command kind (git_local, test, compound, …)
  exe: string;    // executable basename
  cmd: string;    // signature: executable + first subcommand
  estTokens: number; // allocation weight, not a display value
  calls: number;
}
```

api.ts:

```ts
export function fetchCtxExec(filters: Filters): Promise<CtxExecRow[]> {
  return invoke('ctx_exec', { filters });
}
```

- [ ] **Step 5: Verify + commit** — `cargo test` PASS, `npx tsc --noEmit` clean, `npm test` PASS.

```bash
git add src-tauri/src/queries.rs src-tauri/src/lib.rs src/types.ts src/api.ts
git commit -m "feat(ipc): ctx_exec query, command, and TS mirror"
```

---

### Task 5: data.ts execFacets

**Files:**
- Modify: `src/overview/data.ts`
- Test: `src/overview/data.test.ts`

**Interfaces:**
- Produces:

```ts
export interface ExecFacetRow { key: string; tokens: number; calls: number }
export interface ExecFacets {
  byType: ExecFacetRow[];
  byExecutable: ExecFacetRow[];
  byCommand: ExecFacetRow[];
}
export function execFacets(rows: CtxExecRow[], bashTotal: number | null): ExecFacets | null;
```

- [ ] **Step 1: Write the failing tests**

```ts
describe('execFacets', () => {
  const rows: CtxExecRow[] = [
    { source: 'claude', kind: 'git_local', exe: 'git', cmd: 'git add', estTokens: 300, calls: 5 },
    { source: 'claude', kind: 'git_local', exe: 'git', cmd: 'git commit', estTokens: 100, calls: 2 },
    { source: 'claude', kind: 'test', exe: 'npm', cmd: 'npm test', estTokens: 100, calls: 3 },
  ];
  it('groups three ways and allocates the bash total exactly per facet', () => {
    const f = execFacets(rows, 1000)!;
    for (const facet of [f.byType, f.byExecutable, f.byCommand]) {
      expect(facet.reduce((a, r) => a + r.tokens, 0)).toBe(1000);
    }
    expect(f.byType.find((r) => r.key === 'git_local')!.tokens).toBe(800); // 400/500
    expect(f.byType.find((r) => r.key === 'git_local')!.calls).toBe(7);
    expect(f.byExecutable.find((r) => r.key === 'git')!.tokens).toBe(800);
    expect(f.byCommand.find((r) => r.key === 'git add')!.tokens).toBe(600);
    expect(f.byCommand[0].key).toBe('git add'); // sorted desc
  });
  it('null total or no rows → null', () => {
    expect(execFacets(rows, null)).toBeNull();
    expect(execFacets([], 1000)).toBeNull();
  });
});
```

- [ ] **Step 2: Run to verify failure** — `npm test` → FAIL (not exported).

- [ ] **Step 3: Implement** (append to data.ts; extend the `CtxExecRow` type import)

```ts
// ---- Bash exec facets (spec 2026-07-10-bash-exec-drilldown) ----

export interface ExecFacetRow { key: string; tokens: number; calls: number }
export interface ExecFacets {
  byType: ExecFacetRow[];
  byExecutable: ExecFacetRow[];
  byCommand: ExecFacetRow[];
}

function facetOf(rows: CtxExecRow[], keyOf: (r: CtxExecRow) => string, total: number): ExecFacetRow[] {
  const groups = new Map<string, { weight: number; calls: number }>();
  for (const r of rows) {
    const k = keyOf(r);
    const g = groups.get(k) ?? { weight: 0, calls: 0 };
    g.weight += r.estTokens;
    g.calls += r.calls;
    groups.set(k, g);
  }
  const alloc = allocateByWeight(
    total,
    [...groups.entries()].map(([key, g]) => ({ key, weight: g.weight })),
  );
  return [...groups.entries()]
    .map(([key, g]) => ({ key, tokens: alloc.get(key) ?? 0, calls: g.calls }))
    .sort((a, b) => b.tokens - a.tokens);
}

// Three parallel views over the same rows; each facet's tokens sum exactly
// to the Bash leaf's allocated total.
export function execFacets(rows: CtxExecRow[], bashTotal: number | null): ExecFacets | null {
  if (bashTotal == null || rows.length === 0) return null;
  return {
    byType: facetOf(rows, (r) => r.kind, bashTotal),
    byExecutable: facetOf(rows, (r) => r.exe, bashTotal),
    byCommand: facetOf(rows, (r) => r.cmd, bashTotal),
  };
}
```

- [ ] **Step 4: Run tests** — `npm test` PASS, `npx tsc --noEmit` clean.

- [ ] **Step 5: Commit**

```bash
git add src/overview/data.ts src/overview/data.test.ts
git commit -m "feat(overview): execFacets — three allocated views over exec rows"
```

---

### Task 6: Panel facet tabs + wiring

**Files:**
- Modify: `src/overview/ContextBreakdown.tsx`, `src/overview/Overview8b.tsx`, `src/overview/mock.ts`, `src/overview/overview.css`

**Interfaces:**
- Produces: `ContextBreakdown` gains prop `execRows: CtxExecRow[]`; the Bash leaf expands into facet tabs.

- [ ] **Step 1: ContextBreakdown changes**

Add to imports: `execFacets, type ExecFacets` from `./data`; `CtxExecRow` to the `../types` type import; extend the props:

```tsx
  execRows,
}: {
  ...existing props...
  execRows: CtxExecRow[];
}
```

Add local state next to `open`:

```tsx
  const [execTab, setExecTab] = useState<'type' | 'exe' | 'cmd'>('type');
```

Inside the tools map, replace the current unconditional leaf render for
tools with a Bash special case. The current code renders leaves via:

```tsx
            {open.has(`cat:${cat.label}`) &&
              cat.tools.map((t) =>
                row(`tool:${t.name}`, t.name, t.tokens, { muted: true, indent: 2, info: `${t.calls} calls` }),
              )}
```

Change to:

```tsx
            {open.has(`cat:${cat.label}`) &&
              cat.tools.map((t) => {
                const facets = t.name === 'Bash' ? execFacets(execRows, t.tokens) : null;
                return (
                  <div key={`leaf:${t.name}`}>
                    {row(`tool:${t.name}`, t.name, t.tokens, {
                      muted: true,
                      indent: 2,
                      info: `${t.calls} calls`,
                      expandable: !!facets,
                    })}
                    {facets && open.has(`tool:${t.name}`) && (
                      <ExecTable facets={facets} tab={execTab} onTab={setExecTab} />
                    )}
                  </div>
                );
              })}
```

Add the table subcomponent at the bottom of the file:

```tsx
const EXEC_TABS = [
  { key: 'type', label: 'By type' },
  { key: 'exe', label: 'Executable' },
  { key: 'cmd', label: 'Command' },
] as const;
const EXEC_TOP_N = 20;

function ExecTable({
  facets,
  tab,
  onTab,
}: {
  facets: ExecFacets;
  tab: 'type' | 'exe' | 'cmd';
  onTab: (t: 'type' | 'exe' | 'cmd') => void;
}) {
  const rows =
    tab === 'type' ? facets.byType : tab === 'exe' ? facets.byExecutable : facets.byCommand;
  const shown = rows.slice(0, EXEC_TOP_N);
  const hidden = rows.length - shown.length;
  return (
    <div className="tt-exec">
      <div className="tt-exec-tabs">
        {EXEC_TABS.map((t) => (
          <button
            key={t.key}
            className={t.key === tab ? 'active' : ''}
            onClick={() => onTab(t.key)}
          >
            {t.label}
          </button>
        ))}
      </div>
      <div className="tt-exec-table">
        <div className="hd">
          <span>Type</span>
          <span>Calls</span>
          <span>Total</span>
        </div>
        {shown.map((r) => (
          <div className="tr" key={r.key}>
            <span className="k" title={r.key}>{r.key}</span>
            <span>{r.calls}</span>
            <span>{fmtTok(r.tokens)}</span>
          </div>
        ))}
        {hidden > 0 && <div className="more">+{hidden} more</div>}
      </div>
    </div>
  );
}
```

- [ ] **Step 2: CSS** (append to `src/overview/overview.css` near the ctx rules)

```css
.tt-exec { margin: 4px 4px 8px 44px; }
.tt-exec-tabs { display: flex; gap: 4px; margin-bottom: 6px; }
.tt-exec-tabs button {
  background: none; border: 1px solid rgba(255,255,255,.08); border-radius: 6px;
  color: var(--tt-mut2); font-size: 10.5px; padding: 2px 8px; cursor: pointer;
}
.tt-exec-tabs button.active { color: var(--tt-text); background: rgba(255,255,255,.06); }
.tt-exec-table .hd, .tt-exec-table .tr {
  display: grid; grid-template-columns: 1fr 52px 72px; gap: 8px;
  font-size: 11px; padding: 2px 0;
}
.tt-exec-table .hd { color: var(--tt-mut3); text-transform: uppercase; font-size: 9.5px; }
.tt-exec-table .tr .k { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.tt-exec-table .tr span:nth-child(2), .tt-exec-table .tr span:nth-child(3),
.tt-exec-table .hd span:nth-child(2), .tt-exec-table .hd span:nth-child(3) { text-align: right; }
.tt-exec-table .more { color: var(--tt-mut3); font-size: 10.5px; padding: 2px 0; }
```

(If `--tt-mut2`/`--tt-mut3`/`--tt-text` variable names differ in this file,
match whatever the existing `.tt-ctx-*` rules use — read them first.)

- [ ] **Step 3: Overview8b + mock**

Overview8b: import `fetchCtxExec` and `CtxExecRow`; state
`const [ctxExecRows, setCtxExecRows] = useState<CtxExecRow[]>([]);`; inside
the per-range effect's jobs array (same stale guard):
`fetchCtxExec(filters).then((v) => { if (!stale) setCtxExecRows(v); }),`;
pass `execRows={ctxExecRows.filter((r) => r.source === sel)}`.

mock.ts: add `execRows: [],` to the `mockCtxTotals` return object (typed via
the existing return-shape usage; FocusPanel's spread then satisfies the new
prop).

- [ ] **Step 4: Verify** — `npx tsc --noEmit` clean; `npm test` PASS (no new vitest cases; view logic tested in Task 5).

- [ ] **Step 5: Commit**

```bash
git add src/overview/ContextBreakdown.tsx src/overview/Overview8b.tsx src/overview/mock.ts src/overview/overview.css
git commit -m "feat(overview): Bash facet tabs — by type / executable / command"
```

---

### Task 7: E2E on real logs

**Files:**
- Modify: `src-tauri/src/e2e_real_logs.rs`

- [ ] **Step 1: Append after the existing ctx_tools block**

```rust
    // Bash exec facets (spec 2026-07-10-bash-exec-drilldown): rows must exist
    // for claude on real logs; print the top kinds and executables.
    let exec = queries::ctx_exec(&conn, &all).unwrap();
    assert!(
        exec.iter().any(|r| r.source == "claude" && r.est_tokens > 0),
        "expected claude ctx_exec rows on real logs"
    );
    let mut by_kind: std::collections::HashMap<&str, (i64, i64)> = std::collections::HashMap::new();
    let mut by_exe: std::collections::HashMap<&str, (i64, i64)> = std::collections::HashMap::new();
    for r in exec.iter().filter(|r| r.source == "claude") {
        let k = by_kind.entry(r.kind.as_str()).or_insert((0, 0));
        k.0 += r.est_tokens;
        k.1 += r.calls;
        let e = by_exe.entry(r.exe.as_str()).or_insert((0, 0));
        e.0 += r.est_tokens;
        e.1 += r.calls;
    }
    let mut kinds: Vec<_> = by_kind.into_iter().collect();
    kinds.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    println!("=== top exec kinds (claude) ===");
    for (k, (est, calls)) in kinds.iter().take(8) {
        println!("  {:<16} est={:<12} calls={}", k, est, calls);
    }
    let mut exes: Vec<_> = by_exe.into_iter().collect();
    exes.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    println!("=== top exec executables (claude) ===");
    for (e, (est, calls)) in exes.iter().take(8) {
        println!("  {:<16} est={:<12} calls={}", e, est, calls);
    }
```

- [ ] **Step 2: Run** — `cargo test --release e2e_real_logs -- --ignored --nocapture` → PASS; capture the printed kind/executable tables.

- [ ] **Step 3: Full gates** — `cargo test`, `npx tsc --noEmit`, `npm test` → all PASS.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/e2e_real_logs.rs
git commit -m "test(e2e): ctx_exec presence + top-kind/executable prints on real logs"
```

---

## Post-plan notes

- v5's scan-state clear means the next app launch re-scans once more (same
  as v4); the dev-DB hazard note from the previous plan still applies.
- Out of scope (spec): Codex `shell` classification; Duration/Output/By-exit
  facets; drill-into-output.
