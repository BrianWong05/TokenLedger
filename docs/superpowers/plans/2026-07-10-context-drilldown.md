# Context Breakdown v2 — Exact Buckets + Drill-Downs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade the Context Breakdown panel to exact usage-field primary buckets (Messages ▸ history/new input/response, System prompt, Reasoning) with two-level tool drill-downs (Tool calls ▸ categories ▸ tools), per spec `docs/superpowers/specs/2026-07-10-context-drilldown-design.md`.

**Architecture:** Exact buckets are computed at query time from existing token columns (window function finds each session's first cache-write = system prompt). One new `ctx_tools` weights table (schema v4) is filled during the existing scan with per-(file, tool-name, day) estimated content tokens; the frontend allocates the estimated Tool-calls total down the category/tool tree by those weights (largest-remainder). The v3 estimated composition stays for the secondary section and a Messages tooltip.

**Tech Stack:** Rust (rusqlite with window functions, Tauri 2), React + TypeScript, vitest.

## Global Constraints

- **Honesty rule:** NULL (`Option<i64>` / `number | null`) = "source cannot say", renders "—", never 0. SQLite `SUM` of an empty/all-NULL set is NULL — preserve it end to end.
- **Exact-bucket partition:** per source, `history + new_input + COALESCE(system,0) + response + COALESCE(reasoning,0) == input + cache_read + cache_write_5m + cache_write_1h + output` (total usage), exactly. `response = max(0, output − COALESCE(reasoning,0))`.
- **ctx_tools idempotency contract:** rows are keyed `(source_file, name, day)` and accumulated additively; every parse from byte 0 clears that file's rows FIRST (mirrors the fresh-composition rule); byte-offset resumes add increments only. Violating this double-counts on re-scan.
- **Allocation exactness:** largest-remainder allocation — children sum to their parent at every level, ties broken by key ascending.
- **Ledger rule:** `events` rows never deleted, token counts immutable. `ctx_tools` is scan-state-derived and MAY be cleared/rebuilt.
- TS types mirror Rust serde `rename_all = "camelCase"` exactly (`est_tokens` → `estTokens`, `new_input` → `newInput`).
- No new dependencies. Commit style `feat(scope):` per git log. Backend tests from `src-tauri/`: `cargo test`; frontend from repo root: `npm test`, `npx tsc --noEmit`.

---

### Task 1: Schema v4 — ctx_tools table + db helpers

**Files:**
- Modify: `src-tauri/src/db.rs`

**Interfaces:**
- Produces: table `ctx_tools(source, source_file, name, day, est_tokens, calls, PK(source_file, name, day))`; `PRAGMA user_version = 4`; `pub fn clear_ctx_tools_for_file(conn: &Connection, source_file: &str) -> rusqlite::Result<()>`; `pub fn add_ctx_tool_rows(conn: &mut Connection, source: &str, source_file: &str, rows: &[(String, i64, i64, i64)]) -> rusqlite::Result<()>` where rows are `(name, est_tokens, calls, epoch_ts)`.

- [ ] **Step 1: Write the failing tests** (db.rs tests module)

```rust
#[test]
fn v3_db_migrates_to_v4_with_ctx_tools() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    // Build a genuine v3 database by hand.
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute_batch(SCHEMA_V2).unwrap();
        conn.execute_batch(SCHEMA_V3).unwrap();
        conn.execute(
            "INSERT INTO scanned_files (path, size, mtime, byte_offset) VALUES ('f',1,1,1)",
            [],
        )
        .unwrap();
    }
    let conn = open_db(&path).unwrap();
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(v, 4);
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='ctx_tools'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1);
    // Scan state cleared: the one-time full re-scan populates ctx_tools history.
    let files: i64 = conn
        .query_row("SELECT COUNT(*) FROM scanned_files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(files, 0);
}

#[test]
fn ctx_tool_rows_accumulate_and_clear_per_file() {
    let (_dir, mut conn) = temp_db();
    let ts = 1_782_907_200i64;
    add_ctx_tool_rows(&mut conn, "claude", "f1.jsonl", &[
        ("Bash".to_string(), 100, 1, ts),
        ("Bash".to_string(), 50, 0, ts + 60), // same local day: accumulates
        ("Read".to_string(), 30, 1, ts),
    ]).unwrap();
    add_ctx_tool_rows(&mut conn, "claude", "f2.jsonl", &[
        ("Bash".to_string(), 7, 1, ts),
    ]).unwrap();
    let (est, calls): (i64, i64) = conn
        .query_row(
            "SELECT est_tokens, calls FROM ctx_tools WHERE source_file='f1.jsonl' AND name='Bash'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((est, calls), (150, 1));
    // Clearing one file leaves the other's rows.
    clear_ctx_tools_for_file(&conn, "f1.jsonl").unwrap();
    let left: i64 = conn.query_row("SELECT COUNT(*) FROM ctx_tools", [], |r| r.get(0)).unwrap();
    assert_eq!(left, 1);
    let src: String = conn
        .query_row("SELECT source_file FROM ctx_tools", [], |r| r.get(0))
        .unwrap();
    assert_eq!(src, "f2.jsonl");
}
```

- [ ] **Step 2: Run to verify failure**

Run (from `src-tauri/`): `cargo test db::tests`
Expected: compile FAIL — `add_ctx_tool_rows` / `clear_ctx_tools_for_file` not found (and the v4 assertions can't pass until `SCHEMA_V4` exists).

- [ ] **Step 3: Implement**

Add after `SCHEMA_V3` in db.rs:

```rust
// v4: per-tool drill-down weights. Rows are keyed per (source_file, name,
// local day) so re-parses stay idempotent: any parse from byte 0 clears the
// file's rows first; byte-offset resumes add increments over fresh bytes
// only. ctx_tools is scan-state-derived (unlike events) and may be rebuilt.
// Clearing scan state forces the one-time full re-scan that populates
// history. No BEGIN/COMMIT here: migrate() wraps the batches.
const SCHEMA_V4: &str = "\
CREATE TABLE IF NOT EXISTS ctx_tools (
  source TEXT NOT NULL,
  source_file TEXT NOT NULL,
  name TEXT NOT NULL,
  day TEXT NOT NULL,
  est_tokens INTEGER NOT NULL DEFAULT 0,
  calls INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (source_file, name, day)
);
DELETE FROM scanned_files;
DELETE FROM session_ctx;
PRAGMA user_version = 4;";
```

In `migrate()`, after the `version < 3` block:

```rust
        if version < 4 {
            conn.execute_batch(SCHEMA_V4)?;
        }
```

Add the helpers (near the other pub fns):

```rust
pub fn clear_ctx_tools_for_file(conn: &Connection, source_file: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM ctx_tools WHERE source_file = ?1", [source_file])?;
    Ok(())
}

/// Additive per-(file, name, local-day) upsert of tool weights.
/// Idempotency contract: callers clear the file's rows first whenever they
/// re-parse from byte 0; resumes append increments over fresh bytes only.
pub fn add_ctx_tool_rows(
    conn: &mut Connection,
    source: &str,
    source_file: &str,
    rows: &[(String, i64, i64, i64)], // (name, est_tokens, calls, epoch_ts)
) -> rusqlite::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO ctx_tools (source, source_file, name, day, est_tokens, calls) \
             VALUES (?1, ?2, ?3, strftime('%Y-%m-%d', ?4, 'unixepoch', 'localtime'), ?5, ?6) \
             ON CONFLICT(source_file, name, day) DO UPDATE SET \
               est_tokens = est_tokens + excluded.est_tokens, \
               calls = calls + excluded.calls",
        )?;
        for (name, est, calls, ts) in rows {
            stmt.execute(params![source, source_file, name, ts, est, calls])?;
        }
    }
    tx.commit()
}
```

Update the five existing version assertions from 3 to 4:
`fresh_db_has_tables_and_user_version`, `open_is_idempotent`,
`v1_db_migrates_to_v2_preserving_events` (chains v1→v4 now),
`v2_db_migrates_to_v3_preserving_events` (its final version check),
`concurrent_opens_of_v1_db_both_succeed`. Add `"ctx_tools"` to the table list
in `fresh_db_has_tables_and_user_version`.

- [ ] **Step 4: Run tests** — `cargo test` → PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/db.rs
git commit -m "feat(db): schema v4 — ctx_tools per-tool weights with per-file idempotency"
```

---

### Task 2: Engine — per-tool size sink

**Files:**
- Modify: `src-tauri/src/adapters/claude_ctx.rs`

**Interfaces:**
- Produces (consumed by Task 3): `apply_user_line(comp, v, tool_names, tool_sizes: &mut Vec<(String, i64, i64)>)` and `apply_assistant_content(comp, v, tool_names, resources, tool_sizes: &mut Vec<(String, i64, i64)>)` — each pushed entry is `(tool_name, est_tokens, calls)`: `calls` is 1 for a `tool_use`, 0 for a `tool_result`. Tool results resolve their name via the existing `tool_names` id map, falling back to `"unknown"`. The est values are the same `est(content_bytes(...))` already fed into the composition — sizes and composition never diverge.

- [ ] **Step 1: Write the failing tests**

Add to `claude_ctx.rs` tests (and add a `&mut Vec::new()` final argument to every EXISTING `apply_user_line` / `apply_assistant_content` call in this test module):

```rust
#[test]
fn tool_sizes_reported_for_tool_use_and_matched_result() {
    let mut c = Composition::default();
    let mut names = HashMap::new();
    let mut res: Vec<(&'static str, String)> = Vec::new();
    let mut sizes: Vec<(String, i64, i64)> = Vec::new();
    let line = json!({"type":"assistant","message":{"content":[
        {"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls -la"}}
    ]}});
    apply_assistant_content(&mut c, &line, &mut names, &mut res, &mut sizes);
    assert_eq!(sizes.len(), 1);
    let est_in = est(serde_json::to_string(&json!({"command":"ls -la"})).unwrap().len());
    assert_eq!(sizes[0], ("Bash".to_string(), est_in, 1));

    let mut sizes2: Vec<(String, i64, i64)> = Vec::new();
    let result = json!({"type":"user","message":{"role":"user","content":[
        {"type":"tool_result","tool_use_id":"t1","content":"xxxxxxxxxxxxxxxx"}
    ]}});
    apply_user_line(&mut c, &result, &names, &mut sizes2);
    assert_eq!(sizes2, vec![("Bash".to_string(), 4, 0)], "result attributed via id map, calls 0");
}

#[test]
fn unmatched_tool_result_reports_unknown() {
    let mut c = Composition::default();
    let mut sizes: Vec<(String, i64, i64)> = Vec::new();
    let line = json!({"type":"user","message":{"role":"user","content":[
        {"type":"tool_result","tool_use_id":"missing","content":"yyyyyyyy"}
    ]}});
    apply_user_line(&mut c, &line, &HashMap::new(), &mut sizes);
    assert_eq!(sizes, vec![("unknown".to_string(), 2, 0)]);
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test claude_ctx` → compile FAIL (wrong arity).

- [ ] **Step 3: Implement**

`apply_user_line` gains the final parameter `tool_sizes: &mut Vec<(String, i64, i64)>`; in its `tool_result` arm, after the existing counter updates:

```rust
                let name = b["tool_use_id"]
                    .as_str()
                    .and_then(|id| tool_names.get(id))
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());
                tool_sizes.push((name.clone(), n, 0));
```

and rework the existing mcp/skill subset match to use this resolved `name`
(`name.starts_with("mcp__")` / `name == "Skill"`) instead of re-looking-up.

`apply_assistant_content` gains the same final parameter; in its `tool_use`
arm, right after `comp.tool += n;`:

```rust
                tool_sizes.push((name.to_string(), n, 1));
```

Update the two call sites in `src-tauri/src/adapters/claude.rs` minimally so
the crate compiles (pass `&mut Vec::new()` for now — Task 3 wires them for
real), and any other compile errors the arity change surfaces.

- [ ] **Step 4: Run tests** — `cargo test` → PASS (all suites; claude.rs behavior unchanged with throwaway sinks).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/adapters/claude_ctx.rs src-tauri/src/adapters/claude.rs
git commit -m "feat(claude): engine reports per-tool content sizes"
```

---

### Task 3: Claude scan writes ctx_tools

**Files:**
- Modify: `src-tauri/src/adapters/claude.rs`

**Interfaces:**
- Consumes: Task 1 helpers, Task 2 sinks.
- Produces: Claude scans populate `ctx_tools`; full parse (`start == 0`) clears the file's rows first.

- [ ] **Step 1: Write the failing test** (claude.rs tests; single-line JSON fixture style as in existing tests)

```rust
#[test]
fn scan_populates_ctx_tools_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open_db(&dir.path().join("t.db")).unwrap();
    let root = dir.path().join("projects");
    let proj = root.join("x");
    std::fs::create_dir_all(&proj).unwrap();
    let logp = proj.join("s7.jsonl");
    let tooluse = r#"{"type":"assistant","sessionId":"s7","requestId":"r1","timestamp":"2026-07-01T10:00:00.000Z","cwd":"/p/x","message":{"id":"m1","model":"claude-opus-4-8","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls -la"}}],"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    let toolres = r#"{"type":"user","sessionId":"s7","timestamp":"2026-07-01T10:00:01.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"cccccccccccccccccccccccccccccccccccccccc"}]}}"#;
    std::fs::write(&logp, format!("{tooluse}\n{toolres}\n")).unwrap();

    scan_claude(&mut conn, &root);
    let (est1, calls1): (i64, i64) = conn.query_row(
        "SELECT est_tokens, calls FROM ctx_tools WHERE source='claude' AND name='Bash'",
        [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert!(est1 > 0);
    assert_eq!(calls1, 1, "tool_use counts one call; its result adds size only");

    // Unchanged re-scan (resume at EOF): no new bytes, no double counting.
    scan_claude(&mut conn, &root);
    let (est2, calls2): (i64, i64) = conn.query_row(
        "SELECT est_tokens, calls FROM ctx_tools WHERE source='claude' AND name='Bash'",
        [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((est2, calls2), (est1, calls1));

    // Forced full re-parse (heal gesture): rows replaced, not doubled.
    conn.execute("DELETE FROM scanned_files", []).unwrap();
    scan_claude(&mut conn, &root);
    let (est3, calls3): (i64, i64) = conn.query_row(
        "SELECT est_tokens, calls FROM ctx_tools WHERE source='claude' AND name='Bash'",
        [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((est3, calls3), (est1, calls1), "full re-parse replaces the file's rows");
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test adapters::claude::tests::scan_populates_ctx_tools_idempotently` → FAIL (no rows).

- [ ] **Step 3: Implement** in `scan_file`:

Right after `start` is computed and before the read: nothing. After the buffer
is read and BEFORE the line loop, add:

```rust
    // ctx_tools idempotency: a parse from byte 0 rebuilds this file's tool
    // weights from scratch (same rule as the fresh composition).
    if start == 0 {
        crate::db::clear_ctx_tools_for_file(conn, &path_str)?;
    }
    let mut tool_rows: Vec<(String, i64, i64, i64)> = Vec::new();
```

Replace the Task-2 throwaway sinks at the two apply call sites:

```rust
            Some("user") => {
                let mut sizes: Vec<(String, i64, i64)> = Vec::new();
                claude_ctx::apply_user_line(comp, &v, &tool_names, &mut sizes);
                tool_rows.extend(sizes.into_iter().map(|(n, e, c)| (n, e, c, line_ts)));
            }
```

and in the assistant arm (after the existing resources handling):

```rust
                let mut sizes: Vec<(String, i64, i64)> = Vec::new();
                claude_ctx::apply_assistant_content(comp, &v, &mut tool_names, &mut sink, &mut sizes);
                resources.extend(sink.into_iter().map(|(k, n)| (k, n, line_ts)));
                tool_rows.extend(sizes.into_iter().map(|(n, e, c)| (n, e, c, line_ts)));
```

(The user arm currently doesn't compute `line_ts` before the match — ensure
`line_ts` is computed once per line before the `match`, which the current code
already does.)

After `claude_ctx::record_resources(...)`:

```rust
    crate::db::add_ctx_tool_rows(conn, "claude", &path_str, &tool_rows)?;
```

- [ ] **Step 4: Run tests** — `cargo test adapters::claude` then full `cargo test` → PASS (existing tests unaffected: they assert events/ctx, not ctx_tools).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/adapters/claude.rs
git commit -m "feat(claude): scan populates ctx_tools weights"
```

---

### Task 4: Codex scan writes ctx_tools

**Files:**
- Modify: `src-tauri/src/adapters/codex.rs`

**Interfaces:**
- Consumes: Task 1 helpers; `super::claude_ctx::est` (already imported).
- Produces: codex scans populate `ctx_tools` from `function_call` (name, calls 1) and `function_call_output` (name via call_id map, calls 0). Codex always re-parses whole files → always clear first.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn codex_populates_ctx_tools_idempotently() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("sessions");
    write_rollout(&root, "rollout-2026-05-03-tools.jsonl", &[
        r#"{"type":"response_item","timestamp":"2026-05-03T09:00:00.000Z","payload":{"type":"function_call","call_id":"c1","name":"shell","arguments":"{\"command\":[\"ls\"]}"}}"#,
        r#"{"type":"response_item","timestamp":"2026-05-03T09:00:01.000Z","payload":{"type":"function_call_output","call_id":"c1","output":"cccccccccccccccccccc"}}"#,
        r#"{"type":"event_msg","timestamp":"2026-05-03T09:00:02.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":10,"total_tokens":110}}}}"#,
    ]);
    let mut conn = open_db(&tmp.path().join("t.db")).unwrap();
    scan_codex(&mut conn, &root);
    let (est1, calls1): (i64, i64) = conn.query_row(
        "SELECT est_tokens, calls FROM ctx_tools WHERE source='codex' AND name='shell'",
        [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert!(est1 > 0);
    assert_eq!(calls1, 1);

    // Touch the file (size/mtime change) → full re-parse must REPLACE rows.
    let fp = root.join("rollout-2026-05-03-tools.jsonl");
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&fp).unwrap();
        writeln!(f, r#"{{"type":"event_msg","timestamp":"2026-05-03T09:00:03.000Z","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":150,"cached_input_tokens":0,"output_tokens":20,"total_tokens":170}}}}}}}}"#).unwrap();
    }
    scan_codex(&mut conn, &root);
    let (est2, calls2): (i64, i64) = conn.query_row(
        "SELECT est_tokens, calls FROM ctx_tools WHERE source='codex' AND name='shell'",
        [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((est2, calls2), (est1, calls1), "re-parse replaced, not doubled");
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test adapters::codex::tests::codex_populates_ctx_tools_idempotently` → FAIL.

- [ ] **Step 3: Implement** in codex `scan_file`:

After the unchanged-file early-return and before the line loop:

```rust
    // Codex re-parses changed files in full: replace this file's tool rows.
    db::clear_ctx_tools_for_file(conn, &path_str).map_err(|e| e.to_string())?;
    let mut tool_rows: Vec<(String, i64, i64, i64)> = Vec::new();
    let mut call_names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
```

In the `"response_item"` arm, extend the inner match:

```rust
                    Some("function_call") | Some("function_call_output") => {
                        msg_est += est(bytes); // subset rule: tool ⊆ messages
                        tool_est += est(bytes);
                        let ts = v.get("timestamp").and_then(|t| t.as_str()).and_then(iso_to_epoch).unwrap_or(0);
                        let is_call = payload.get("type").and_then(|t| t.as_str()) == Some("function_call");
                        let name = if is_call {
                            let n = payload.get("name").and_then(|n| n.as_str()).unwrap_or("unknown").to_string();
                            if let Some(id) = payload.get("call_id").and_then(|c| c.as_str()) {
                                call_names.insert(id.to_string(), n.clone());
                            }
                            n
                        } else {
                            payload
                                .get("call_id")
                                .and_then(|c| c.as_str())
                                .and_then(|id| call_names.get(id))
                                .cloned()
                                .unwrap_or_else(|| "unknown".to_string())
                        };
                        tool_rows.push((name, est(bytes), if is_call { 1 } else { 0 }, ts));
                    }
```

After the loop's `db::insert_events(...)` call:

```rust
    db::add_ctx_tool_rows(conn, "codex", &path_str, &tool_rows).map_err(|e| e.to_string())?;
```

- [ ] **Step 4: Run tests** — `cargo test adapters::codex` then full `cargo test` → PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/adapters/codex.rs
git commit -m "feat(codex): scan populates ctx_tools weights"
```

---

### Task 5: Queries — ctx_buckets + ctx_tools

**Files:**
- Modify: `src-tauri/src/queries.rs`

**Interfaces:**
- Produces:

```rust
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CtxBuckets {
    pub source: String,
    pub history: i64,          // cache_read + non-first cache writes
    pub new_input: i64,        // fresh input_tokens
    pub system: Option<i64>,   // first cache write per session; NULL when unknowable
    pub response: i64,         // max(0, output − reasoning)
    pub reasoning: Option<i64>,
}
pub fn ctx_buckets(conn: &Connection, f: &Filters) -> rusqlite::Result<Vec<CtxBuckets>>;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CtxToolRow {
    pub source: String,
    pub name: String,
    pub est_tokens: i64,
    pub calls: i64,
}
pub fn ctx_tools(conn: &Connection, f: &Filters) -> rusqlite::Result<Vec<CtxToolRow>>;
```

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn ctx_buckets_exact_partition_and_first_cw() {
        std::env::set_var("TZ", "UTC");
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        // Session sa: first cw event (day1) then a later cw event (day2).
        let mut a = ev("a", "claude", DAY1_TS, "m", None, 1, 100, 50, 0, 900, 0);
        a.session_id = Some("sa".to_string());
        let mut b = ev("b", "claude", DAY2_TS, "m", None, 1, 200, 30, 1500, 250, 0);
        b.session_id = Some("sa".to_string());
        // NULL session id: cache writes count as history, never system.
        let c = ev("c", "claude", DAY1_TS, "m", None, 1, 10, 5, 0, 40, 0);
        // Hermes: aggregated rows — all cw is history, system NULL.
        let mut h = ev("h", "hermes", DAY1_TS, "hermes-local", None, 1, 300, 100, 20, 60, 0);
        h.session_id = Some("hs".to_string());
        h.reasoning_tokens = Some(25);
        db::insert_events(&mut conn, &[a, b, c, h]).unwrap();

        let all = ctx_buckets(&conn, &Filters::default()).unwrap();
        let cl = all.iter().find(|x| x.source == "claude").unwrap();
        assert_eq!(cl.system, Some(900), "session sa's FIRST cache write only");
        assert_eq!(cl.history, 1500 + 250 + 40, "cache_read + later cw + NULL-session cw");
        assert_eq!(cl.new_input, 310);
        assert_eq!(cl.reasoning, None, "claude reasoning not reported");
        assert_eq!(cl.response, 85);
        // Exact partition vs total usage.
        let total = 100 + 50 + 900 + 200 + 30 + 1500 + 250 + 10 + 5 + 40;
        assert_eq!(cl.history + cl.new_input + cl.system.unwrap_or(0) + cl.response
            + cl.reasoning.unwrap_or(0), total);

        let hm = all.iter().find(|x| x.source == "hermes").unwrap();
        assert_eq!(hm.system, None, "hermes aggregates: first-vs-rest unknowable");
        assert_eq!(hm.history, 20 + 60, "all hermes cw is history");
        assert_eq!(hm.reasoning, Some(25));
        assert_eq!(hm.response, 75);

        // Range starting day2: session sa's first cw is OUTSIDE the range →
        // in-range cw counts as history, system is NULL (nothing in range).
        let f = Filters { start_ts: Some(DAY2_START), ..Filters::default() };
        let d2 = ctx_buckets(&conn, &f).unwrap();
        let cl2 = d2.iter().find(|x| x.source == "claude").unwrap();
        assert_eq!(cl2.system, None);
        assert_eq!(cl2.history, 1500 + 250);
    }

    #[test]
    fn ctx_tools_sums_by_source_and_range() {
        std::env::set_var("TZ", "UTC");
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        db::add_ctx_tool_rows(&mut conn, "claude", "f1", &[
            ("Bash".to_string(), 100, 2, DAY1_TS),
            ("Bash".to_string(), 50, 1, DAY2_TS),
            ("Read".to_string(), 30, 1, DAY1_TS),
        ]).unwrap();
        db::add_ctx_tool_rows(&mut conn, "codex", "f2", &[
            ("shell".to_string(), 70, 1, DAY1_TS),
        ]).unwrap();

        let all = ctx_tools(&conn, &Filters::default()).unwrap();
        let bash = all.iter().find(|r| r.name == "Bash").unwrap();
        assert_eq!((bash.est_tokens, bash.calls), (150, 3));

        let f = Filters { start_ts: Some(DAY1_START), end_ts: Some(DAY2_START), ..Filters::default() };
        let d1 = ctx_tools(&conn, &f).unwrap();
        assert_eq!(d1.iter().find(|r| r.name == "Bash").unwrap().est_tokens, 100);

        let f2 = Filters { tools: vec!["codex".to_string()], ..Filters::default() };
        let cx = ctx_tools(&conn, &f2).unwrap();
        assert_eq!(cx.len(), 1);
        assert_eq!(cx[0].name, "shell");
    }
```

- [ ] **Step 2: Run to verify failure** — `cargo test queries` → compile FAIL.

- [ ] **Step 3: Implement**

```rust
// Exact usage-field buckets (spec 2026-07-10-context-drilldown). The window
// runs over the WHOLE table so a session straddling the range still knows
// which cache-write was its first; range/tool/model/project filters apply
// OUTSIDE the window. A first-cw event outside the range means in-range
// writes count as history — conservative, never inflates System.
pub fn ctx_buckets(conn: &Connection, f: &Filters) -> rusqlite::Result<Vec<CtxBuckets>> {
    let (where_sql, params) = build_where(f);
    let sql = format!(
        "WITH ranked AS ( \
           SELECT source, model, project, timestamp, session_id, \
                  input_tokens, output_tokens, cache_read_tokens, reasoning_tokens, \
                  cache_write_5m_tokens + cache_write_1h_tokens AS cw, \
                  ROW_NUMBER() OVER ( \
                    PARTITION BY source, session_id, \
                      CASE WHEN cache_write_5m_tokens + cache_write_1h_tokens > 0 THEN 1 ELSE 0 END \
                    ORDER BY timestamp, dedup_key) AS cw_rank \
           FROM events) \
         SELECT source, \
           SUM(cache_read_tokens) + SUM(CASE WHEN cw > 0 AND NOT (cw_rank = 1 AND session_id IS NOT NULL AND source != 'hermes') THEN cw ELSE 0 END), \
           SUM(input_tokens), \
           SUM(CASE WHEN cw > 0 AND cw_rank = 1 AND session_id IS NOT NULL AND source != 'hermes' THEN cw END), \
           SUM(output_tokens), \
           SUM(reasoning_tokens) \
         FROM ranked {where_sql} GROUP BY source ORDER BY source"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        Ok((
            r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?,
            r.get::<_, Option<i64>>(3)?, r.get::<_, i64>(4)?, r.get::<_, Option<i64>>(5)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (source, history, new_input, system, output, reasoning) = row?;
        out.push(CtxBuckets {
            source,
            history,
            new_input,
            system,
            response: (output - reasoning.unwrap_or(0)).max(0),
            reasoning,
        });
    }
    Ok(out)
}

// Per-tool weights in range: same day-bounds convention as ctx_resources
// (end_ts exclusive → day of end_ts − 1s inclusive). Ignores model/project.
pub fn ctx_tools(conn: &Connection, f: &Filters) -> rusqlite::Result<Vec<CtxToolRow>> {
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
        "SELECT source, name, SUM(est_tokens), SUM(calls) FROM ctx_tools {where_sql} \
         GROUP BY source, name ORDER BY SUM(est_tokens) DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        Ok(CtxToolRow { source: r.get(0)?, name: r.get(1)?, est_tokens: r.get(2)?, calls: r.get(3)? })
    })?;
    rows.collect()
}
```

Note for the test's `ev(...)` helper: its signature is
`ev(key, source, ts, model, project, calls, input, output, cr, w5, w1)` —
the `900`/`250`/`40`/`60` values above are `w5` (5m cache writes).

- [ ] **Step 4: Run tests** — `cargo test` → PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/queries.rs
git commit -m "feat(queries): ctx_buckets exact usage buckets + ctx_tools weights"
```

---

### Task 6: IPC + TS types + api

**Files:**
- Modify: `src-tauri/src/lib.rs`, `src/types.ts`, `src/api.ts`

**Interfaces:**
- Produces: commands `ctx_buckets(filters)`, `ctx_tools(filters)`; TS `CtxBuckets { source: string; history: number; newInput: number; system: number | null; response: number; reasoning: number | null }`, `CtxToolRow { source: string; name: string; estTokens: number; calls: number }`; `fetchCtxBuckets(filters)`, `fetchCtxTools(filters)`.

- [ ] **Step 1: Backend commands** (lib.rs, mirroring the existing `ctx_resources` command exactly)

```rust
#[tauri::command]
fn ctx_buckets(
    state: State<'_, AppState>,
    filters: Filters,
) -> Result<Vec<queries::CtxBuckets>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    queries::ctx_buckets(&db, &filters).map_err(|e| e.to_string())
}

#[tauri::command]
fn ctx_tools(
    state: State<'_, AppState>,
    filters: Filters,
) -> Result<Vec<queries::CtxToolRow>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    queries::ctx_tools(&db, &filters).map_err(|e| e.to_string())
}
```

Register both in `generate_handler![...]` (add `ctx_buckets, ctx_tools,`).

- [ ] **Step 2: TS types + api**

`src/types.ts`:

```ts
export interface CtxBuckets {
  source: string;
  history: number;         // cache reads + non-first cache writes
  newInput: number;        // fresh input tokens
  system: number | null;   // first cache write per session; null = unknowable
  response: number;        // output − reasoning
  reasoning: number | null;
}

export interface CtxToolRow {
  source: string;
  name: string;
  estTokens: number; // allocation weight (estimated content), not display value
  calls: number;
}
```

`src/api.ts`:

```ts
export function fetchCtxBuckets(filters: Filters): Promise<CtxBuckets[]> {
  return invoke('ctx_buckets', { filters });
}

export function fetchCtxTools(filters: Filters): Promise<CtxToolRow[]> {
  return invoke('ctx_tools', { filters });
}
```

(extend the type import list accordingly).

- [ ] **Step 3: Verify** — `cargo test` PASS; `npx tsc --noEmit` clean; `npm test` PASS.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/lib.rs src/types.ts src/api.ts
git commit -m "feat(ipc): ctx_buckets + ctx_tools commands and TS mirrors"
```

---

### Task 7: data.ts — categorizeTool, allocateByWeight, toolTree, bucketRows

**Files:**
- Modify: `src/overview/data.ts`
- Test: `src/overview/data.test.ts`

**Interfaces:**
- Produces:

```ts
export function categorizeTool(name: string): string;
export function allocateByWeight(total: number, entries: { key: string; weight: number }[]): Map<string, number>;
export interface ToolLeaf { name: string; tokens: number; calls: number }
export interface ToolCategory { label: string; tokens: number; tools: ToolLeaf[] }
export function toolTree(rows: CtxToolRow[], toolcallsTotal: number | null): ToolCategory[];
export interface BucketView {
  total: number;                 // total usage (percent denominator)
  messages: number;              // history + newInput + response
  history: number; newInput: number; response: number;
  system: number | null; reasoning: number | null;
}
export function bucketView(b: CtxBuckets | null): BucketView | null;
```

- [ ] **Step 1: Write the failing tests**

```ts
describe('categorizeTool', () => {
  it('maps the known names', () => {
    expect(categorizeTool('Read')).toBe('File Ops');
    expect(categorizeTool('Edit')).toBe('File Ops');
    expect(categorizeTool('Grep')).toBe('Search');
    expect(categorizeTool('Bash')).toBe('Execution');
    expect(categorizeTool('TaskUpdate')).toBe('Task Mgmt');
    expect(categorizeTool('TodoWrite')).toBe('Task Mgmt');
    expect(categorizeTool('Task')).toBe('Agent');
    expect(categorizeTool('Agent')).toBe('Agent');
    expect(categorizeTool('WebFetch')).toBe('Web');
    expect(categorizeTool('mcp__pencil__batch_get')).toBe('MCP: pencil');
    expect(categorizeTool('Skill')).toBe('Skill');
    expect(categorizeTool('SomethingNew')).toBe('Other');
  });
});

describe('allocateByWeight', () => {
  it('children sum exactly to total (largest remainder)', () => {
    const out = allocateByWeight(100, [
      { key: 'a', weight: 1 }, { key: 'b', weight: 1 }, { key: 'c', weight: 1 },
    ]);
    const vals = [...out.values()];
    expect(vals.reduce((x, y) => x + y, 0)).toBe(100);
    expect(Math.max(...vals) - Math.min(...vals)).toBeLessThanOrEqual(1);
  });
  it('zero weights yield zeros', () => {
    const out = allocateByWeight(50, [{ key: 'a', weight: 0 }]);
    expect(out.get('a')).toBe(0);
  });
});

describe('toolTree', () => {
  const rows: CtxToolRow[] = [
    { source: 'claude', name: 'Bash', estTokens: 300, calls: 5 },
    { source: 'claude', name: 'Read', estTokens: 150, calls: 3 },
    { source: 'claude', name: 'Edit', estTokens: 50, calls: 2 },
  ];
  it('allocates parent total down two summing levels', () => {
    const tree = toolTree(rows, 1000);
    expect(tree.reduce((a, c) => a + c.tokens, 0)).toBe(1000);
    for (const cat of tree) {
      expect(cat.tools.reduce((a, t) => a + t.tokens, 0)).toBe(cat.tokens);
    }
    const exec = tree.find((c) => c.label === 'Execution')!;
    expect(exec.tokens).toBe(600); // 300/500 of 1000
  });
  it('null total or no rows → empty tree', () => {
    expect(toolTree(rows, null)).toEqual([]);
    expect(toolTree([], 1000)).toEqual([]);
  });
});

describe('bucketView', () => {
  it('derives messages group and total', () => {
    const v = bucketView({ source: 'claude', history: 800, newInput: 100, system: 50, response: 40, reasoning: null })!;
    expect(v.messages).toBe(940);
    expect(v.total).toBe(990);
    expect(v.reasoning).toBeNull();
  });
  it('null in → null out', () => {
    expect(bucketView(null)).toBeNull();
  });
});
```

- [ ] **Step 2: Run to verify failure** — `npm test` → FAIL (not exported).

- [ ] **Step 3: Implement** (append to data.ts; add `CtxBuckets, CtxToolRow` to the type import)

```ts
// ---- tool drill-down (spec 2026-07-10-context-drilldown) ----

// Category map mirrors TokenTracker's, trimmed to names seen in our logs.
export function categorizeTool(name: string): string {
  if (name === 'Task' || name === 'Agent') return 'Agent';
  if (/^Task(Create|Update|Get|List|Output|Stop)$/.test(name) || name.startsWith('Todo')) return 'Task Mgmt';
  if (/^(Read|Write|Edit|Glob)$/.test(name)) return 'File Ops';
  if (name === 'Grep') return 'Search';
  if (name === 'Bash') return 'Execution';
  if (/^Web(Fetch|Search)$/.test(name)) return 'Web';
  if (name.startsWith('mcp__')) {
    const server = name.split('__')[1] ?? 'unknown';
    return `MCP: ${server}`;
  }
  if (name === 'Skill') return 'Skill';
  return 'Other';
}

// Largest-remainder integer allocation: results sum exactly to total.
// Ties broken by key ascending for determinism.
export function allocateByWeight(
  total: number,
  entries: { key: string; weight: number }[],
): Map<string, number> {
  const out = new Map<string, number>();
  const W = entries.reduce((a, e) => a + Math.max(0, e.weight), 0);
  if (W <= 0 || total <= 0) {
    for (const e of entries) out.set(e.key, 0);
    return out;
  }
  let allocated = 0;
  const rems: { key: string; rem: number }[] = [];
  for (const e of entries) {
    const exact = (total * Math.max(0, e.weight)) / W;
    const base = Math.floor(exact);
    out.set(e.key, base);
    allocated += base;
    rems.push({ key: e.key, rem: exact - base });
  }
  rems.sort((a, b) => b.rem - a.rem || (a.key < b.key ? -1 : 1));
  for (let i = 0; i < total - allocated; i++) {
    const k = rems[i % rems.length].key;
    out.set(k, (out.get(k) ?? 0) + 1);
  }
  return out;
}

export interface ToolLeaf { name: string; tokens: number; calls: number }
export interface ToolCategory { label: string; tokens: number; tools: ToolLeaf[] }

// Allocate the estimated Tool-calls total down category → tool by the stored
// content weights, so children sum to their parent at both levels.
export function toolTree(rows: CtxToolRow[], toolcallsTotal: number | null): ToolCategory[] {
  if (toolcallsTotal == null || rows.length === 0) return [];
  const byCat = new Map<string, CtxToolRow[]>();
  for (const r of rows) {
    const cat = categorizeTool(r.name);
    const arr = byCat.get(cat);
    if (arr) arr.push(r);
    else byCat.set(cat, [r]);
  }
  const catWeights = [...byCat.entries()].map(([label, rs]) => ({
    key: label,
    weight: rs.reduce((a, r) => a + r.estTokens, 0),
  }));
  const catTokens = allocateByWeight(toolcallsTotal, catWeights);
  const tree = [...byCat.entries()].map(([label, rs]) => {
    const tokens = catTokens.get(label) ?? 0;
    const leafTokens = allocateByWeight(
      tokens,
      rs.map((r) => ({ key: r.name, weight: r.estTokens })),
    );
    const tools = rs
      .map((r) => ({ name: r.name, tokens: leafTokens.get(r.name) ?? 0, calls: r.calls }))
      .sort((a, b) => b.tokens - a.tokens);
    return { label, tokens, tools };
  });
  return tree.sort((a, b) => b.tokens - a.tokens);
}

// ---- exact usage buckets ----

export interface BucketView {
  total: number;
  messages: number;
  history: number;
  newInput: number;
  response: number;
  system: number | null;
  reasoning: number | null;
}

export function bucketView(b: CtxBuckets | null): BucketView | null {
  if (!b) return null;
  const messages = b.history + b.newInput + b.response;
  return {
    total: messages + (b.system ?? 0) + (b.reasoning ?? 0),
    messages,
    history: b.history,
    newInput: b.newInput,
    response: b.response,
    system: b.system,
    reasoning: b.reasoning,
  };
}
```

- [ ] **Step 4: Run tests** — `npm test` PASS; `npx tsc --noEmit` clean.

- [ ] **Step 5: Commit**

```bash
git add src/overview/data.ts src/overview/data.test.ts
git commit -m "feat(overview): categorizeTool, allocateByWeight, toolTree, bucketView"
```

---

### Task 8: Expandable ContextBreakdown + Overview8b wiring + 8a adapter

**Files:**
- Modify: `src/overview/ContextBreakdown.tsx` (rewrite render), `src/overview/Overview8b.tsx`, `src/overview/mock.ts`, `src/overview/FocusPanel.tsx`, `src/overview/overview.css` (one indent class)

**Interfaces:**
- Produces: `ContextBreakdown({ tool, ctx, buckets, toolRows, meta }: { tool: ToolMeta; ctx: CtxTotals; buckets: CtxBuckets | null; toolRows: CtxToolRow[]; meta: string })`.

- [ ] **Step 1: Rewrite ContextBreakdown.tsx**

```tsx
import { useState } from 'react';
import { bucketView, toolTree, type CtxTotals, type ToolMeta } from './data';
import type { CtxBuckets, CtxToolRow } from '../types';
import { fmtTok, fmtPct } from '../lib/format';

const EST_TIP = 'estimated share of billed context (content bytes ÷ 4)';

// Context panel v2 (spec 2026-07-10-context-drilldown): exact usage-field
// primaries with expandable Messages; estimated secondary section with a
// two-level Tool-calls drill-down allocated by stored content weights.
// null renders "—" (source cannot say) — never 0.
export default function ContextBreakdown({
  tool,
  ctx,
  buckets,
  toolRows,
  meta,
}: {
  tool: ToolMeta;
  ctx: CtxTotals;
  buckets: CtxBuckets | null;
  toolRows: CtxToolRow[];
  meta: string;
}) {
  const [open, setOpen] = useState<Set<string>>(new Set());
  const toggle = (k: string) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(k)) next.delete(k);
      else next.add(k);
      return next;
    });

  const v = bucketView(buckets);
  const hit = ctx.billed > 0 ? ctx.reused / ctx.billed : 0;
  const denom = Math.max(1, v?.total ?? 0);
  const tree = toolTree(toolRows, ctx.toolcalls);
  const estTip =
    ctx.messages != null && ctx.system != null
      ? `est. content composition: messages ${fmtTok(ctx.messages)} · system ${fmtTok(ctx.system)}`
      : undefined;

  const row = (
    key: string,
    label: string,
    tokens: number | null,
    opts: { pct?: boolean; muted?: boolean; indent?: 0 | 1 | 2; expandable?: boolean; info?: string; calls?: number } = {},
  ) => (
    <div
      className={
        'tt-ctx-row' + (opts.muted ? ' muted' : '') + (opts.indent ? ` indent-${opts.indent}` : '')
      }
      key={key}
      onClick={opts.expandable ? () => toggle(key) : undefined}
      style={opts.expandable ? { cursor: 'pointer' } : undefined}
      title={opts.info}
    >
      <span className="name">
        <span className="dot" style={{ background: opts.muted ? '#4a5262' : tool.color }} />
        {label}
        {opts.expandable && <span className="aff">{open.has(key) ? '▾' : '›'}</span>}
        {opts.info && !opts.expandable && <span className="aff">ⓘ</span>}
      </span>
      <span className="vals">
        {tokens == null ? (
          <span className="val">—</span>
        ) : (
          <>
            <span className="val">{fmtTok(tokens)}</span>
            {opts.pct && <span className="rpct">{fmtPct(tokens / denom)}</span>}
          </>
        )}
      </span>
    </div>
  );

  return (
    <>
      <div className="tt-ctx-title">
        <span className="dot" style={{ background: tool.color }} />
        {tool.source} Context Breakdown
      </div>
      <div className="tt-ctx-sub">
        Cache hit rate <b>{fmtPct(hit)}</b> · <b>{fmtTok(ctx.reused)}</b> reused /{' '}
        <b>{fmtTok(ctx.billed)}</b> input
      </div>

      {row('messages', 'Messages', v ? v.messages : null, { pct: true, expandable: !!v, info: estTip })}
      {v && open.has('messages') && (
        <>
          {row('history', 'Conversation history', v.history, { indent: 1 })}
          {row('newInput', 'New input', v.newInput, {
            indent: 1,
            info: 'uncached input for the newest turn — user text and fresh tool results',
          })}
          {row('response', 'Assistant response', v.response, { indent: 1 })}
        </>
      )}
      {row('system', 'System prompt', v ? v.system : null, {
        pct: true,
        info: 'first cache write of each session',
      })}
      {row('reasoning', 'Reasoning', v ? v.reasoning : null, { pct: true })}

      <div style={{ height: 1, background: 'rgba(255,255,255,.06)', margin: '8px 4px' }} />

      {row('toolcalls', 'Tool calls', ctx.toolcalls, {
        muted: true,
        expandable: tree.length > 0,
        info: EST_TIP,
      })}
      {open.has('toolcalls') &&
        tree.map((cat) => (
          <div key={cat.label}>
            {row(`cat:${cat.label}`, cat.label, cat.tokens, {
              muted: true,
              indent: 1,
              expandable: cat.tools.length > 0,
            })}
            {open.has(`cat:${cat.label}`) &&
              cat.tools.map((t) =>
                row(`tool:${t.name}`, t.name, t.tokens, {
                  muted: true,
                  indent: 2,
                  info: `${t.calls} calls`,
                }),
              )}
          </div>
        ))}
      {row('agents', 'Custom agents', ctx.agents, { muted: true, info: EST_TIP })}
      {row('mcp', 'MCP servers', ctx.mcp, { muted: true, info: EST_TIP })}
      {row('skills', 'Skills', ctx.skills, { muted: true, info: EST_TIP })}

      {meta && <div className="tt-ctx-meta">{meta}</div>}
    </>
  );
}
```

Add to `src/overview/overview.css` next to the existing `.tt-ctx-row` rules:

```css
.tt-ctx-row.indent-1 { padding-left: 22px; }
.tt-ctx-row.indent-2 { padding-left: 40px; }
```

Note the previous component's Hermes "no content attribution" note is
superseded: Hermes now has real bucket sub-rows; its estimated section rows
render "—" as before, which is self-explanatory next to real primaries.

- [ ] **Step 2: mock adapter + FocusPanel**

In `src/overview/mock.ts`, extend `mockCtxTotals(tool)`'s return object with
mock-shaped new props (after the existing `ctx`/`meta` fields):

```ts
    buckets: {
      source: tool,
      history: Math.round(c.reused * 0.98),
      newInput: Math.round(c.input * 0.004),
      system: Math.round(c.input * 0.003),
      response: Math.round(c.input * 0.002),
      reasoning: null,
    },
    toolRows: [
      { source: tool, name: 'Bash', estTokens: 300, calls: 12 },
      { source: tool, name: 'Read', estTokens: 200, calls: 9 },
    ],
```

`FocusPanel.tsx` continues to spread `{...mockCtxTotals(sel)}` — no change
needed beyond compiling.

- [ ] **Step 3: Overview8b wiring**

- Imports: add `fetchCtxBuckets, fetchCtxTools` to the `../api` import; add
  `CtxBuckets, CtxToolRow` to the `../types` type import.
- State:

```tsx
  const [ctxBuckets, setCtxBuckets] = useState<CtxBuckets[]>([]);
  const [ctxToolRows, setCtxToolRows] = useState<CtxToolRow[]>([]);
```

- In the per-range effect's `jobs` array (same stale guard as siblings):

```tsx
        fetchCtxBuckets(filters).then((v) => { if (!stale) setCtxBuckets(v); }),
        fetchCtxTools(filters).then((v) => { if (!stale) setCtxToolRows(v); }),
```

- Render:

```tsx
                <ContextBreakdown
                  tool={tool}
                  ctx={view.ctx}
                  buckets={ctxBuckets.find((b) => b.source === sel) ?? null}
                  toolRows={ctxToolRows.filter((r) => r.source === sel)}
                  meta={ctxMeta(ctxRes, sel)}
                />
```

- [ ] **Step 4: Verify** — `npx tsc --noEmit` clean; `npm test` PASS (40+ tests, no regressions).

- [ ] **Step 5: Commit**

```bash
git add src/overview/ContextBreakdown.tsx src/overview/Overview8b.tsx src/overview/mock.ts src/overview/FocusPanel.tsx src/overview/overview.css
git commit -m "feat(overview): expandable exact buckets + tool drill-down panel"
```

---

### Task 9: E2E invariants on real logs

**Files:**
- Modify: `src-tauri/src/e2e_real_logs.rs`

- [ ] **Step 1: Add invariant checks** (after the existing ctx checks)

```rust
    // Exact-bucket partition (spec 2026-07-10-context-drilldown): per source,
    // history + new_input + system + response + reasoning == total usage.
    let buckets = queries::ctx_buckets(&conn, &all).unwrap();
    for b in &buckets {
        let (tot_in, tot_out, tot_cr, tot_cw): (i64, i64, i64, i64) = conn
            .query_row(
                "SELECT SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens), \
                 SUM(cache_write_5m_tokens + cache_write_1h_tokens) FROM events WHERE source = ?1",
                [&b.source],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        let total = tot_in + tot_out + tot_cr + tot_cw;
        let sum = b.history + b.new_input + b.system.unwrap_or(0) + b.response
            + b.reasoning.unwrap_or(0);
        assert_eq!(sum, total, "bucket partition exact for {}", b.source);
        println!(
            "  {:<8} history={} new_input={} system={:?} response={} reasoning={:?}",
            b.source, b.history, b.new_input, b.system, b.response, b.reasoning
        );
    }

    // Tool weights: print top rows per source for an eyeball check.
    let tools = queries::ctx_tools(&conn, &all).unwrap();
    println!("=== top ctx_tools ===");
    for t in tools.iter().take(12) {
        println!("  {:<8} {:<28} est={:<10} calls={}", t.source, t.name, t.est_tokens, t.calls);
    }
    assert!(
        tools.iter().any(|t| t.source == "claude" && t.est_tokens > 0),
        "expected claude tool weights on real logs"
    );
```

Note: `response = max(0, output − reasoning)` can theoretically clamp; on real
logs reasoning ≤ output per event so the partition holds. If the assert fires
with a small delta, investigate reasoning > output rows before touching the
invariant.

- [ ] **Step 2: Run** — `cargo test --release e2e_real_logs -- --ignored --nocapture` → PASS; capture the printed buckets + top tools.

- [ ] **Step 3: Full gates** — `cargo test`, `npm test`, `npx tsc --noEmit` → all PASS.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/e2e_real_logs.rs
git commit -m "test(e2e): exact-bucket partition + tool-weight checks on real logs"
```

---

## Post-plan notes

- First launch after v4 re-scans all logs once (populates `ctx_tools` history).
- The dev-DB hazard applies: if `npm run tauri dev` runs during implementation,
  an intermediate build may consume the v4 backfill early — harmless now
  (clearing `scanned_files` self-heals since the full-reparse fix), but expect
  to re-clear scan state once at the end if panel data looks thin.
- Out of scope (spec): per-skill-name drill-down, output-side content split,
  Bash command-level drill-down.
