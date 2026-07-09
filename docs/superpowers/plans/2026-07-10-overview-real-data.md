# Overview Real-Data Wiring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the mock data behind the mounted "TokenTracker · Overview" view (`src/overview/Overview8b.tsx`) with real Ledger data, adding the backend capabilities it needs: per-source time series, conversation counts, reasoning tokens, and the Cache-Estimated marker.

**Architecture:** Schema v2 adds nullable `session_id`/`reasoning_tokens` columns backfilled by a forced full re-scan. A new `series` IPC command returns per-bucket × per-source rows — the real-data twin of the mock's `DAYS` array. The frontend gets a `data.ts` layer that reshapes series/summary/breakdown responses into the exact shapes the 8b components already consume.

**Tech Stack:** Rust (rusqlite, serde, Tauri v2), React 18 + TypeScript (strict), vitest. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-07-10-overview-real-data-design.md`

## Global Constraints

- Reasoning tokens are a **display subset of output tokens**: never priced, never added to totals. `NULL` means "source doesn't report it" and renders as `—`, never as `0`.
- Convs = `COUNT(DISTINCT session_id)` (SQL ignores NULLs). Never summed across days in the UI.
- Migration + backfill re-scan must not change any token count (e2e: Claude totals still match ccusage < 0.5%).
- All time bucketing is local time: `strftime(fmt, timestamp, 'unixepoch', 'localtime')` — same as the existing `trend`.
- Rust structs use `#[serde(rename_all = "camelCase")]`; TS types in `src/types.ts` mirror them field-for-field.
- Cost display: `null` → `"unpriced"`, partial → `≥ $X` — use the existing `formatCost` in `src/lib/format.ts`.
- The Ledger never deletes events. The v2 migration clears only `scanned_files` (scan state), never `events`.
- Commands: `cargo test` runs from `src-tauri/`; `npm test` / `npm run build` from the repo root.
- The old dashboard (`src/App.tsx`, `src/components/`) and the 8a variant (`src/overview/Overview.tsx`, `FocusPanel.tsx`, `TrendBars.tsx`, `ContextBreakdown.tsx`, `mock.ts`) stay in the repo and must keep compiling, but get only the minimal edits this plan names.
- Commit after every task. Commit messages end with:
  `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`

## File Structure

Backend (`src-tauri/src/`):
- Modify `types.rs` — `UsageEvent` gains `session_id: Option<String>`, `reasoning_tokens: Option<i64>`
- Modify `db.rs` — v2 migration, 14-column insert SQL, backfilling conflict clauses
- Modify `adapters/claude.rs`, `adapters/codex.rs`, `adapters/gemini.rs`, `adapters/hermes.rs` — emit the two new fields
- Modify `queries.rs` — new `SeriesPoint` + `series()`; `Summary.cache_estimated_models`; `BreakdownRow` extensions
- Modify `lib.rs` — register the `series` IPC command

Frontend (`src/`):
- Modify `types.ts`, `api.ts` — `SeriesPoint`, `fetchSeries`, extended `Summary`/`BreakdownRow`
- Modify `lib/format.ts` (+ `lib/format.test.ts`) — gains `fmtTok`, `fmtUSD`, `fmtPct`, `fmtDate`, `fmtIsoDate`
- Create `overview/data.ts` (+ `overview/data.test.ts`) — shared meta (TOOLS, Day, Bucket, …) + real-data reshaping
- Modify `overview/mock.ts` — imports meta/formatters from `data.ts`/`lib/format`, re-exports them; `Day` gains `cost`
- Modify `overview/Heatmap.tsx` — takes `days: Day[]` prop
- Create `overview/TokenBreakdown.tsx` — real per-tool token-category panel (replaces ContextBreakdown in 8b only)
- Modify `overview/ModelsList.tsx` — pure-presentational `models: ModelBar[]` prop
- Modify `overview/BreakdownTable.tsx` — takes real `dailyRows`/`projectRows` props
- Modify `overview/Overview8b.tsx` — fetch orchestration, real wiring, loading/error states
- Modify `overview/Overview.tsx`, `overview/FocusPanel.tsx` — one-line call-site adaptations (8a keeps rendering mock data)
- Modify `overview/overview.css` — `.tt-tag`, `.tt-error`, loading dim

---

### Task 1: Schema v2 migration + UsageEvent fields + backfilling inserts

**Files:**
- Modify: `src-tauri/src/types.rs`
- Modify: `src-tauri/src/db.rs`
- Modify (compile-only placeholder fields): `src-tauri/src/adapters/claude.rs`, `codex.rs`, `gemini.rs`, `hermes.rs`, `src-tauri/src/queries.rs` (test helper)

**Interfaces:**
- Consumes: existing `UsageEvent`, `open_db`, insert functions.
- Produces: `UsageEvent { …, session_id: Option<String>, reasoning_tokens: Option<i64> }`; `events` table with `session_id TEXT`, `reasoning_tokens INTEGER` columns; `PRAGMA user_version = 2`; all insert paths upsert the two new columns on dedup-key conflict.

- [ ] **Step 1: Write the failing tests** — add to the `tests` module in `src-tauri/src/db.rs`:

```rust
#[test]
fn v1_db_migrates_to_v2_preserving_events() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    // Build a genuine v1 database by hand (SCHEMA still writes user_version 1).
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO events (dedup_key, source, timestamp, model, project, api_calls, \
             input_tokens, output_tokens, cache_read_tokens, cache_write_5m_tokens, \
             cache_write_1h_tokens, source_file) \
             VALUES ('claude:old:1','claude',1,'m',NULL,1,10,20,0,0,0,'f')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO scanned_files (path, size, mtime, byte_offset) VALUES ('f',1,1,1)",
            [],
        )
        .unwrap();
    }
    let conn = open_db(&path).unwrap();
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(v, 2);
    // Old row intact, new columns NULL.
    let (input, sid, rt): (i64, Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT input_tokens, session_id, reasoning_tokens FROM events \
             WHERE dedup_key='claude:old:1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(input, 10);
    assert_eq!(sid, None);
    assert_eq!(rt, None);
    // Scan state cleared so the next scan re-parses every log (backfill).
    let files: i64 = conn
        .query_row("SELECT COUNT(*) FROM scanned_files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(files, 0);
}

#[test]
fn insert_backfills_new_columns_on_existing_rows() {
    let (_dir, mut conn) = temp_db();
    let mut old = sample_event("claude:a:1", "f1.jsonl");
    old.session_id = None;
    old.reasoning_tokens = None;
    insert_events(&mut conn, &[old]).unwrap();
    // A re-scan delivers the same event, now carrying the new fields.
    let mut new = sample_event("claude:a:1", "f1.jsonl");
    new.session_id = Some("sess-1".to_string());
    new.reasoning_tokens = Some(7);
    let inserted = insert_events(&mut conn, &[new]).unwrap();
    assert_eq!(inserted, 0, "backfill update is not a new event");
    let (sid, rt, out): (Option<String>, Option<i64>, i64) = conn
        .query_row(
            "SELECT session_id, reasoning_tokens, output_tokens FROM events \
             WHERE dedup_key='claude:a:1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(sid, Some("sess-1".to_string()));
    assert_eq!(rt, Some(7));
    assert_eq!(out, 50, "token counts unchanged by backfill");
}

#[test]
fn keep_max_backfills_session_on_equal_output() {
    let (_dir, mut conn) = temp_db();
    let mut old = sample_event("claude:x:1", "f1.jsonl");
    old.session_id = None;
    insert_events_keep_max_output(&mut conn, &[old]).unwrap();
    // Same output_tokens (50): the >= conflict clause must still backfill.
    let mut new = sample_event("claude:x:1", "f1.jsonl");
    new.session_id = Some("sess-x".to_string());
    let n = insert_events_keep_max_output(&mut conn, &[new]).unwrap();
    assert_eq!(n, 0);
    let sid: Option<String> = conn
        .query_row(
            "SELECT session_id FROM events WHERE dedup_key='claude:x:1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(sid, Some("sess-x".to_string()));
}
```

Also update the two existing version asserts: in `fresh_db_has_tables_and_user_version` and `open_is_idempotent`, change `assert_eq!(version, 1)` to `assert_eq!(version, 2)`.

- [ ] **Step 2: Run tests to verify they fail**

Run (from `src-tauri/`): `cargo test db::`
Expected: FAIL — `no field session_id on type UsageEvent` (compile error).

- [ ] **Step 3: Implement**

In `src-tauri/src/types.rs`, add to `UsageEvent` (after `source_file`):

```rust
    pub session_id: Option<String>,
    pub reasoning_tokens: Option<i64>,
```

In `src-tauri/src/db.rs`:

```rust
// After the SCHEMA const. Clearing scanned_files forces the next scan to
// re-parse every log so existing rows get session_id/reasoning backfilled
// via the ON CONFLICT clauses below. Events are never deleted (Ledger rule).
const SCHEMA_V2: &str = "\
BEGIN;
ALTER TABLE events ADD COLUMN session_id TEXT;
ALTER TABLE events ADD COLUMN reasoning_tokens INTEGER;
DELETE FROM scanned_files;
PRAGMA user_version = 2;
COMMIT;";
```

```rust
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 1 {
        conn.execute_batch(SCHEMA)?;
    }
    if version < 2 {
        conn.execute_batch(SCHEMA_V2)?;
    }
    Ok(())
}
```

Replace `INSERT_SQL` and `REPLACE_SQL`:

```rust
// On dedup conflict, refresh only the v2 columns: token counts are immutable
// (Ledger), but a backfill re-scan must fill session_id/reasoning on old rows.
const INSERT_SQL: &str = "INSERT INTO events \
(dedup_key, source, timestamp, model, project, api_calls, input_tokens, output_tokens, \
cache_read_tokens, cache_write_5m_tokens, cache_write_1h_tokens, source_file, session_id, reasoning_tokens) \
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14) \
ON CONFLICT(dedup_key) DO UPDATE SET \
  session_id = excluded.session_id, reasoning_tokens = excluded.reasoning_tokens";

const REPLACE_SQL: &str = "INSERT OR REPLACE INTO events \
(dedup_key, source, timestamp, model, project, api_calls, input_tokens, output_tokens, \
cache_read_tokens, cache_write_5m_tokens, cache_write_1h_tokens, source_file, session_id, reasoning_tokens) \
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)";
```

`insert_events` must now count distinct new rows via COUNT diff (an UPDATE also reports `changes() == 1`, which would inflate the count):

```rust
pub fn insert_events(conn: &mut Connection, events: &[UsageEvent]) -> rusqlite::Result<u64> {
    let tx = conn.transaction()?;
    let before: i64 = tx.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
    {
        let mut stmt = tx.prepare(INSERT_SQL)?;
        for e in events {
            stmt.execute(params![
                e.dedup_key, e.source, e.timestamp, e.model, e.project, e.api_calls,
                e.input_tokens, e.output_tokens, e.cache_read_tokens,
                e.cache_write_5m_tokens, e.cache_write_1h_tokens, e.source_file,
                e.session_id, e.reasoning_tokens
            ])?;
        }
    }
    let after: i64 = tx.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
    tx.commit()?;
    Ok((after - before).max(0) as u64)
}
```

In `insert_events_keep_max_output`, extend the statement's column list, VALUES, and SET list with the two new columns, and change the guard from `>` to `>=` so a backfill re-scan (equal output) still updates:

```rust
        let mut stmt = tx.prepare(
            "INSERT INTO events (dedup_key, source, timestamp, model, project, api_calls, \
             input_tokens, output_tokens, cache_read_tokens, cache_write_5m_tokens, cache_write_1h_tokens, source_file, session_id, reasoning_tokens) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14) \
             ON CONFLICT(dedup_key) DO UPDATE SET \
               source=excluded.source, input_tokens=excluded.input_tokens, output_tokens=excluded.output_tokens, \
               cache_read_tokens=excluded.cache_read_tokens, cache_write_5m_tokens=excluded.cache_write_5m_tokens, \
               cache_write_1h_tokens=excluded.cache_write_1h_tokens, project=excluded.project, \
               timestamp=excluded.timestamp, model=excluded.model, api_calls=excluded.api_calls, \
               source_file=excluded.source_file, session_id=excluded.session_id, reasoning_tokens=excluded.reasoning_tokens \
             WHERE excluded.output_tokens >= events.output_tokens",
        )?;
```

and add `e.session_id, e.reasoning_tokens` to its `params![…]` and to the `params![…]` lists in `upsert_events` and `replace_file_events`.

Fix every `UsageEvent { … }` struct literal so the crate compiles — add these two lines to each:

```rust
            session_id: None,
            reasoning_tokens: None,
```

Sites: `db.rs` `sample_event`, `queries.rs` test helper `ev`, `adapters/claude.rs` `parse_line`, `adapters/codex.rs` `scan_file`, `adapters/gemini.rs` `process_file`, `adapters/hermes.rs` `scan_hermes`. (Tasks 2–5 replace the adapter `None`s with real values.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: all PASS (including the three new tests and every pre-existing test).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/types.rs src-tauri/src/db.rs src-tauri/src/adapters src-tauri/src/queries.rs
git commit -m "feat(db): schema v2 — session_id + reasoning_tokens with backfilling inserts"
```

---

### Task 2: Claude adapter emits session_id

**Files:**
- Modify: `src-tauri/src/adapters/claude.rs`

**Interfaces:**
- Consumes: `UsageEvent` from Task 1.
- Produces: Claude events with `session_id = Some(<line's sessionId>)` (None when the field is absent) and `reasoning_tokens = None` (Claude never reports reasoning separately — spec).

- [ ] **Step 1: Write the failing test** — add to the `tests` module in `claude.rs`:

```rust
#[test]
fn captures_session_id_when_present() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open_db(&dir.path().join("t.db")).unwrap();
    let root = dir.path().join("projects");
    let proj = root.join("x");
    std::fs::create_dir_all(&proj).unwrap();

    let with_sid = r#"{"type":"assistant","sessionId":"sess-cl-1","requestId":"req_s1","timestamp":"2026-06-05T09:00:00.000Z","cwd":"/Users/dev/projects/x","message":{"id":"msg_s1","model":"claude-opus-4-8","usage":{"input_tokens":10,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    let without_sid = r#"{"type":"assistant","requestId":"req_s2","timestamp":"2026-06-05T09:01:00.000Z","cwd":"/Users/dev/projects/x","message":{"id":"msg_s2","model":"claude-opus-4-8","usage":{"input_tokens":20,"output_tokens":2,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    std::fs::write(proj.join("s.jsonl"), format!("{with_sid}\n{without_sid}\n")).unwrap();

    let res = scan_claude(&mut conn, &root);
    assert_eq!(res.events_inserted, 2);

    let (sid, rt): (Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT session_id, reasoning_tokens FROM events WHERE dedup_key='claude:msg_s1:req_s1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(sid, Some("sess-cl-1".to_string()));
    assert_eq!(rt, None, "Claude does not report reasoning separately");

    let sid2: Option<String> = conn
        .query_row(
            "SELECT session_id FROM events WHERE dedup_key='claude:msg_s2:req_s2'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(sid2, None);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test adapters::claude::tests::captures_session_id_when_present`
Expected: FAIL — `assertion failed: sid == Some("sess-cl-1")` (adapter still emits None).

- [ ] **Step 3: Implement** — in `parse_line`, before the `Ok(Some(UsageEvent { … }))`, add:

```rust
    let session_id = v
        .get("sessionId")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());
```

and in the struct literal replace `session_id: None,` with `session_id,` (keep `reasoning_tokens: None`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test adapters::claude`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/adapters/claude.rs
git commit -m "feat(claude): capture sessionId per line"
```

---

### Task 3: Codex adapter emits session_id + reasoning deltas

**Files:**
- Modify: `src-tauri/src/adapters/codex.rs`

**Interfaces:**
- Consumes: `UsageEvent` from Task 1.
- Produces: Codex events with `session_id = Some(<rollout file stem>)` and `reasoning_tokens = Some(Δ reasoning_output_tokens)` per snapshot (same max(0,delta) rule as the other cumulative fields), `None` when the snapshot lacks the field.

- [ ] **Step 1: Write the failing tests** — add to the `tests` module in `codex.rs`:

```rust
fn write_rollout(dir: &std::path::Path, name: &str, lines: &[&str]) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, lines.join("\n") + "\n").unwrap();
    p
}

#[test]
fn codex_reasoning_deltas_and_session_id() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("sessions");
    write_rollout(&root, "rollout-2026-04-23-abc.jsonl", &[
        r#"{"type":"session_meta","timestamp":"2026-04-23T12:23:20.000Z","payload":{"id":"sess-1","cwd":"/Users/dev/projects/alpha"}}"#,
        r#"{"type":"turn_context","timestamp":"2026-04-23T12:23:25.000Z","payload":{"model":"gpt-5.4"}}"#,
        r#"{"type":"event_msg","timestamp":"2026-04-23T12:23:28.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":50,"reasoning_output_tokens":10,"total_tokens":150}}}}"#,
        r#"{"type":"event_msg","timestamp":"2026-04-23T12:23:35.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":200,"cached_input_tokens":60,"output_tokens":90,"reasoning_output_tokens":25,"total_tokens":290}}}}"#,
    ]);
    let mut conn = open_db(&tmp.path().join("t.db")).unwrap();
    let r = scan_codex(&mut conn, &root);
    assert_eq!(r.events_inserted, 2);

    let rows: Vec<(Option<String>, Option<i64>)> = {
        let mut stmt = conn
            .prepare("SELECT session_id, reasoning_tokens FROM events WHERE source='codex' ORDER BY timestamp")
            .unwrap();
        let it = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        it.collect::<rusqlite::Result<Vec<_>>>().unwrap()
    };
    assert_eq!(rows[0], (Some("rollout-2026-04-23-abc".to_string()), Some(10)));
    assert_eq!(rows[1], (Some("rollout-2026-04-23-abc".to_string()), Some(15)));
}

#[test]
fn codex_missing_reasoning_field_is_null() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("sessions");
    write_rollout(&root, "rollout-2026-04-24-def.jsonl", &[
        r#"{"type":"event_msg","timestamp":"2026-04-24T09:00:00.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":50,"total_tokens":150}}}}"#,
    ]);
    let mut conn = open_db(&tmp.path().join("t.db")).unwrap();
    let r = scan_codex(&mut conn, &root);
    assert_eq!(r.events_inserted, 1);
    let rt: Option<i64> = conn
        .query_row("SELECT reasoning_tokens FROM events WHERE source='codex'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rt, None, "absent field means not-reported, never 0");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test adapters::codex`
Expected: the two new tests FAIL on the session_id/reasoning asserts.

- [ ] **Step 3: Implement** — in `scan_file`:

Add a cumulative tracker next to `prev_output`:

```rust
    let mut prev_reasoning: i64 = 0;
```

In the `token_count` branch, after the `d_output` block, add:

```rust
                // reasoning_output_tokens is cumulative like the other fields.
                // Absent field => this source/build doesn't report reasoning => None.
                let reasoning = usage
                    .get("reasoning_output_tokens")
                    .and_then(|x| x.as_i64())
                    .map(|cur| {
                        let d = (cur - prev_reasoning).max(0);
                        prev_reasoning = cur;
                        d
                    });
```

In the `UsageEvent` literal, replace the placeholders:

```rust
                    session_id: Some(file_stem.clone()),
                    reasoning_tokens: reasoning,
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test adapters::codex`
Expected: all PASS (including the pre-existing fixture test).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/adapters/codex.rs
git commit -m "feat(codex): session_id from file stem, reasoning via snapshot deltas"
```

---

### Task 4: Gemini adapter emits session_id + thoughts as reasoning

**Files:**
- Modify: `src-tauri/src/adapters/gemini.rs`

**Interfaces:**
- Consumes: `UsageEvent` from Task 1.
- Produces: Gemini events with `session_id = Some(sessionId)` and `reasoning_tokens = Some(tokens.thoughts)`.

- [ ] **Step 1: Write the failing assertions** — extend `test_scan_gemini_extracts_and_maps` (after the m1 assertions):

```rust
        // v2 columns: session id and thoughts-as-reasoning.
        let (sid, rt): (Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT session_id, reasoning_tokens FROM events WHERE dedup_key = 'gemini:sess-alpha:m1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(sid, Some("sess-alpha".to_string()));
        assert_eq!(rt, Some(50), "thoughts reported as reasoning subset");
        let rt2: Option<i64> = conn
            .query_row(
                "SELECT reasoning_tokens FROM events WHERE dedup_key = 'gemini:sess-alpha:m2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rt2, Some(0), "reported zero, not NULL");
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test adapters::gemini`
Expected: FAIL on `sid == Some("sess-alpha")`.

- [ ] **Step 3: Implement** — in `process_file`'s `UsageEvent` literal, replace the placeholders:

```rust
            session_id: Some(session.session_id.clone()),
            reasoning_tokens: Some(tokens.thoughts),
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test adapters::gemini`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/adapters/gemini.rs
git commit -m "feat(gemini): session_id + thoughts as reasoning tokens"
```

---

### Task 5: Hermes adapter emits session_id + reasoning

**Files:**
- Modify: `src-tauri/src/adapters/hermes.rs`

**Interfaces:**
- Consumes: `UsageEvent` from Task 1.
- Produces: Hermes events with `session_id = Some(<sessions.id>)` and `reasoning_tokens = Some(reasoning_tokens)` (the column value; still ALSO folded into output — unchanged).

- [ ] **Step 1: Write the failing assertions** — extend `extracts_and_normalizes_sessions` (after the s1 block):

```rust
        // v2 columns.
        let (sid, rt): (Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT session_id, reasoning_tokens FROM events WHERE dedup_key = 'hermes:s1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(sid, Some("s1".to_string()));
        assert_eq!(rt, Some(50));
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test adapters::hermes`
Expected: FAIL on the session assert.

- [ ] **Step 3: Implement** — in the `UsageEvent` literal (note: build `dedup_key` first, then reuse `id`):

```rust
            dedup_key: format!("hermes:{id}"),
            ...
            session_id: Some(id.clone()),
            reasoning_tokens: Some(reasoning),
```

`id` is consumed by `format!` via reference, so `Some(id.clone())` works; alternatively clone before the literal. Keep `output_tokens: output + reasoning` exactly as-is.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test adapters::hermes`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/adapters/hermes.rs
git commit -m "feat(hermes): session_id + reasoning column passthrough"
```

---

### Task 6: `queries::series` — per-bucket per-source rows

**Files:**
- Modify: `src-tauri/src/queries.rs`

**Interfaces:**
- Consumes: `Filters`, `build_where`, `RateMap`, v2 `events` columns.
- Produces:

```rust
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesPoint {
    pub bucket: String,               // "YYYY-MM-DD" (day) or "YYYY-MM-DD HH:00" (hour)
    pub source: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,      // 5m + 1h combined
    pub total_tokens: i64,
    pub reasoning_tokens: Option<i64>, // None = no source-reported value in bucket
    pub cost: f64,                    // priced tokens only; unpriced contribute 0
    pub requests: i64,
    pub convs: i64,                   // COUNT(DISTINCT session_id) in bucket
}

pub fn series(conn: &Connection, f: &Filters, bucket: &str) -> rusqlite::Result<Vec<SeriesPoint>>
```

- [ ] **Step 1: Write the failing tests** — add to the `tests` module in `queries.rs`:

```rust
    // Events with v2 fields for series tests.
    fn ev_s(
        key: &str, source: &str, ts: i64, model: &str,
        session: Option<&str>, reasoning: Option<i64>,
    ) -> UsageEvent {
        let mut e = ev(key, source, ts, model, None, 1, 100, 50, 0, 0, 0);
        e.session_id = session.map(|s| s.to_string());
        e.reasoning_tokens = reasoning;
        e
    }

    #[test]
    fn series_groups_by_day_and_source() {
        std::env::set_var("TZ", "UTC");
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        let events = vec![
            ev_s("c1", "codex", DAY1_TS, "gpt-5.4", Some("sa"), Some(5)),
            ev_s("c2", "codex", DAY1_TS, "gpt-5.4", Some("sa"), Some(3)),
            ev_s("c3", "codex", DAY1_TS, "gpt-5.4-mini", Some("sb"), None),
            ev_s("h1", "hermes", DAY1_TS, "hermes-local", Some("hs"), Some(0)),
            ev_s("c4", "codex", DAY2_TS, "gpt-5.4", None, None),
        ];
        db::insert_events(&mut conn, &events).unwrap();
        conn.execute(
            "INSERT INTO prices (model, input_per_tok, output_per_tok, cache_read_per_tok, cache_write_5m_per_tok, cache_write_1h_per_tok) \
             VALUES ('gpt-5.4', 0.000002, 0.000010, 0.0000005, 0.0000025, 0.000004)",
            [],
        ).unwrap();

        let pts = series(&conn, &Filters::default(), "day").unwrap();
        assert_eq!(pts.len(), 3); // (day1,codex), (day1,hermes), (day2,codex)

        let d1c = pts.iter().find(|p| p.bucket == "2026-07-01" && p.source == "codex").unwrap();
        assert_eq!(d1c.total_tokens, 450); // 3 events × (100 input + 50 output)
        assert_eq!(d1c.requests, 3);
        assert_eq!(d1c.convs, 2, "sa + sb, distinct across models within the source");
        assert_eq!(d1c.reasoning_tokens, Some(8), "5 + 3; the NULL event does not zero it");
        // Only the two gpt-5.4 events price: 200×2e-6 + 100×1e-5.
        approx(d1c.cost, 0.0014);

        let d1h = pts.iter().find(|p| p.bucket == "2026-07-01" && p.source == "hermes").unwrap();
        assert_eq!(d1h.reasoning_tokens, Some(0), "reported zero ≠ not reported");
        approx(d1h.cost, 0.0);

        let d2c = pts.iter().find(|p| p.bucket == "2026-07-02").unwrap();
        assert_eq!(d2c.convs, 0, "NULL session ids count zero distinct");
        assert_eq!(d2c.reasoning_tokens, None, "nothing reported that day");
    }

    #[test]
    fn series_hour_buckets_local_time() {
        std::env::set_var("TZ", "UTC");
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        db::insert_events(&mut conn, &[ev_s("a", "codex", DAY1_TS, "gpt-5.4", None, None)]).unwrap();
        let pts = series(&conn, &Filters::default(), "hour").unwrap();
        assert_eq!(pts.len(), 1);
        assert_eq!(pts[0].bucket, "2026-07-01 12:00");
    }

    #[test]
    fn series_day_sums_match_summary() {
        let (_dir, conn) = seed();
        let pts = series(&conn, &Filters::default(), "day").unwrap();
        let s = summary(&conn, &Filters::default()).unwrap();
        let total: i64 = pts.iter().map(|p| p.total_tokens).sum();
        assert_eq!(total, s.total_tokens);
        let cost: f64 = pts.iter().map(|p| p.cost).sum();
        approx(cost, s.cost.unwrap());
        let reqs: i64 = pts.iter().map(|p| p.requests).sum();
        assert_eq!(reqs, s.requests);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test queries::`
Expected: FAIL — `cannot find function series` (compile error).

- [ ] **Step 3: Implement** — add after `trend`:

```rust
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesPoint {
    pub bucket: String,
    pub source: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub total_tokens: i64,
    pub reasoning_tokens: Option<i64>,
    pub cost: f64,
    pub requests: i64,
    pub convs: i64,
}

// Per-(bucket, source) series — the real-data twin of the frontend mock's DAYS.
pub fn series(conn: &Connection, f: &Filters, bucket: &str) -> rusqlite::Result<Vec<SeriesPoint>> {
    let fmt = if bucket == "hour" { "%Y-%m-%d %H:00" } else { "%Y-%m-%d" };
    let rates = RateMap::load(conn)?;
    let (where_sql, params) = build_where(f);

    // Tokens/cost need per-model rows for rate resolution.
    let sql = format!(
        "SELECT strftime('{fmt}', timestamp, 'unixepoch', 'localtime') AS bucket, source, model, \
         SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens), \
         SUM(cache_write_5m_tokens), SUM(cache_write_1h_tokens), SUM(api_calls), SUM(reasoning_tokens) \
         FROM events {where_sql} GROUP BY bucket, source, model ORDER BY bucket, source"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        Ok((
            r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?, r.get::<_, i64>(4)?, r.get::<_, i64>(5)?,
            r.get::<_, i64>(6)?, r.get::<_, i64>(7)?, r.get::<_, i64>(8)?,
            r.get::<_, Option<i64>>(9)?,
        ))
    })?;

    let mut idx: HashMap<(String, String), usize> = HashMap::new();
    let mut points: Vec<SeriesPoint> = Vec::new();
    for row in rows {
        let (bucket, source, model, in_, out, cr, w5, w1, calls, reasoning) = row?;
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
        let key = (bucket, source);
        let i = match idx.get(&key) {
            Some(&i) => i,
            None => {
                points.push(SeriesPoint {
                    bucket: key.0.clone(),
                    source: key.1.clone(),
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                    total_tokens: 0,
                    reasoning_tokens: None,
                    cost: 0.0,
                    requests: 0,
                    convs: 0,
                });
                idx.insert(key, points.len() - 1);
                points.len() - 1
            }
        };
        let p = &mut points[i];
        p.input_tokens += in_;
        p.output_tokens += out;
        p.cache_read_tokens += cr;
        p.cache_write_tokens += w5 + w1;
        p.total_tokens += in_ + out + cr + w5 + w1;
        p.requests += calls;
        p.cost += c;
        if let Some(r) = reasoning {
            p.reasoning_tokens = Some(p.reasoning_tokens.unwrap_or(0) + r);
        }
    }

    // Convs need distinct-count at (bucket, source) — a session can span
    // models, so distinct-per-model counts cannot be summed.
    let sql2 = format!(
        "SELECT strftime('{fmt}', timestamp, 'unixepoch', 'localtime') AS bucket, source, \
         COUNT(DISTINCT session_id) FROM events {where_sql} GROUP BY bucket, source"
    );
    let mut stmt2 = conn.prepare(&sql2)?;
    let crows = stmt2.query_map(params_from_iter(params.iter()), |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
    })?;
    for row in crows {
        let (bucket, source, convs) = row?;
        if let Some(&i) = idx.get(&(bucket, source)) {
            points[i].convs = convs;
        }
    }
    Ok(points)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test queries::`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/queries.rs
git commit -m "feat(queries): series — per-bucket per-source tokens, cost, convs, reasoning"
```

---

### Task 7: Summary cache-estimated + breakdown extensions

**Files:**
- Modify: `src-tauri/src/queries.rs`

**Interfaces:**
- Consumes: `RateMap`, v2 columns.
- Produces:
  - `Summary` gains `pub cache_estimated_models: Vec<String>` (serialized `cacheEstimatedModels`).
  - `BreakdownRow` gains `pub source: Option<String>` (populated only for `by="model"`), `pub reasoning_tokens: Option<i64>`, `pub convs: i64`, `pub cache_estimated: bool`.
  - `breakdown(conn, "model", f)` rows are grouped by (model, source).

- [ ] **Step 1: Write the failing tests** — add to `queries.rs` tests:

```rust
    #[test]
    fn summary_flags_cache_estimated_models() {
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        // Absent catalog cache rates are stored as 0.0 (see pricing::write_price_row).
        conn.execute(
            "INSERT INTO prices (model, input_per_tok, output_per_tok, cache_read_per_tok, cache_write_5m_per_tok, cache_write_1h_per_tok) \
             VALUES ('half-priced', 0.000001, 0.000002, 0, 0, 0)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO prices (model, input_per_tok, output_per_tok, cache_read_per_tok, cache_write_5m_per_tok, cache_write_1h_per_tok) \
             VALUES ('full-priced', 0.000001, 0.000002, 0.0000001, 0.000001, 0.000001)",
            [],
        ).unwrap();
        let events = vec![
            ev("a", "codex", DAY1_TS, "half-priced", None, 1, 100, 50, 40, 10, 0),
            ev("b", "codex", DAY1_TS, "full-priced", None, 1, 100, 50, 40, 10, 0),
        ];
        db::insert_events(&mut conn, &events).unwrap();
        let s = summary(&conn, &Filters::default()).unwrap();
        assert_eq!(s.cache_estimated_models, vec!["half-priced".to_string()]);
        assert!(!s.has_unpriced);
        assert!(s.cost.is_some());
    }

    #[test]
    fn cache_estimated_requires_cache_tokens() {
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        conn.execute(
            "INSERT INTO prices (model, input_per_tok, output_per_tok, cache_read_per_tok, cache_write_5m_per_tok, cache_write_1h_per_tok) \
             VALUES ('half-priced', 0.000001, 0.000002, 0, 0, 0)",
            [],
        ).unwrap();
        // No cache tokens at all -> nothing is missing from the estimate.
        db::insert_events(&mut conn, &[ev("a", "codex", DAY1_TS, "half-priced", None, 1, 100, 50, 0, 0, 0)]).unwrap();
        let s = summary(&conn, &Filters::default()).unwrap();
        assert!(s.cache_estimated_models.is_empty());
    }

    #[test]
    fn breakdown_model_rows_carry_source_convs_reasoning_and_flag() {
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        conn.execute(
            "INSERT INTO prices (model, input_per_tok, output_per_tok, cache_read_per_tok, cache_write_5m_per_tok, cache_write_1h_per_tok) \
             VALUES ('half-priced', 0.000001, 0.000002, 0, 0, 0)",
            [],
        ).unwrap();
        let mut e1 = ev("a", "codex", DAY1_TS, "half-priced", None, 1, 100, 50, 40, 0, 0);
        e1.session_id = Some("sa".to_string());
        e1.reasoning_tokens = Some(5);
        let mut e2 = ev("b", "codex", DAY1_TS, "half-priced", None, 1, 100, 50, 0, 0, 0);
        e2.session_id = Some("sa".to_string());
        e2.reasoning_tokens = Some(3);
        // Same model name from a different source -> its own row.
        let mut e3 = ev("c", "hermes", DAY1_TS, "half-priced", None, 1, 100, 50, 0, 0, 0);
        e3.session_id = Some("hs".to_string());
        db::insert_events(&mut conn, &[e1, e2, e3]).unwrap();

        let rows = breakdown(&conn, "model", &Filters::default()).unwrap();
        assert_eq!(rows.len(), 2, "model rows split by source");
        let codex = rows.iter().find(|r| r.source == Some("codex".to_string())).unwrap();
        assert_eq!(codex.key, "half-priced");
        assert_eq!(codex.convs, 1, "one distinct session");
        assert_eq!(codex.reasoning_tokens, Some(8));
        assert!(codex.cache_estimated, "cache tokens present but cache rate is 0");
        let hermes = rows.iter().find(|r| r.source == Some("hermes".to_string())).unwrap();
        assert_eq!(hermes.convs, 1);
        assert_eq!(hermes.reasoning_tokens, None);
        assert!(!hermes.cache_estimated, "no cache tokens used");
    }

    #[test]
    fn breakdown_project_carries_convs_and_null_source() {
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        let mut e1 = ev("a", "codex", DAY1_TS, "gpt-5.4", Some("/p/alpha"), 1, 100, 50, 0, 0, 0);
        e1.session_id = Some("sa".to_string());
        let mut e2 = ev("b", "codex", DAY1_TS, "gpt-5.4-mini", Some("/p/alpha"), 1, 100, 50, 0, 0, 0);
        e2.session_id = Some("sa".to_string());
        db::insert_events(&mut conn, &[e1, e2]).unwrap();
        let rows = breakdown(&conn, "project", &Filters::default()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, None, "source only set for model mode");
        assert_eq!(rows[0].convs, 1, "distinct across models within the project");
    }
```

Also extend the existing `breakdown_by_model_sorted_desc_with_none_cost` with:

```rust
        assert_eq!(rows[0].source, Some("codex".to_string()));
        assert_eq!(rows[1].source, Some("hermes".to_string()));
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test queries::`
Expected: FAIL — `no field cache_estimated_models` (compile error).

- [ ] **Step 3: Implement**

`Summary` gains (after `unpriced_models`):

```rust
    pub cache_estimated_models: Vec<String>,
```

In `summary()`, add `let mut cache_estimated_models: Vec<String> = Vec::new();` and rework the match arms:

```rust
        match rates.resolve(&model) {
            Some(rt) => {
                cost += in_ as f64 * rt.input
                    + out as f64 * rt.output
                    + cr as f64 * rt.cache_read
                    + w5 as f64 * rt.cache_write_5m
                    + w1 as f64 * rt.cache_write_1h;
                priced_tokens += tokens;
                // ponytail: prices store an absent cache rate as 0.0, so "no
                // rate" == 0.0 here; distinguish-explicit-zero needs nullable
                // price columns — add if a catalog ever prices cache at $0.
                let cache_gap = (cr > 0 && rt.cache_read == 0.0)
                    || (w5 > 0 && rt.cache_write_5m == 0.0)
                    || (w1 > 0 && rt.cache_write_1h == 0.0);
                if cache_gap {
                    cache_estimated_models.push(model);
                }
            }
            None => {
                if tokens > 0 {
                    unpriced_models.push(model);
                }
            }
        }
```

and add `cache_estimated_models,` to the returned `Summary`.

`BreakdownRow` gains:

```rust
    pub source: Option<String>,
    pub reasoning_tokens: Option<i64>,
    pub convs: i64,
    pub cache_estimated: bool,
```

Rework `breakdown()` — uniform row shape via a constant-NULL source column for non-model modes:

```rust
pub fn breakdown(conn: &Connection, by: &str, f: &Filters) -> rusqlite::Result<Vec<BreakdownRow>> {
    let group_col = match by {
        "tool" => "source",
        "project" => "project",
        _ => "model",
    };
    // Model rows additionally split by source so the UI can scope models to a
    // tool; a constant NULL leaves other modes' grouping untouched.
    let src_expr = if group_col == "model" { "source" } else { "NULL" };
    let rates = RateMap::load(conn)?;
    let (where_sql, params) = build_where(f);
    let sql = format!(
        "SELECT {group_col} AS grp, {src_expr} AS src, model, \
         SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens), \
         SUM(cache_write_5m_tokens), SUM(cache_write_1h_tokens), SUM(api_calls), SUM(reasoning_tokens) \
         FROM events {where_sql} GROUP BY grp, src, model"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        Ok((
            r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?, r.get::<_, i64>(4)?, r.get::<_, i64>(5)?,
            r.get::<_, i64>(6)?, r.get::<_, i64>(7)?, r.get::<_, i64>(8)?,
            r.get::<_, Option<i64>>(9)?,
        ))
    })?;

    let mut map: HashMap<(String, Option<String>), Agg> = HashMap::new();
    for row in rows {
        let (grp, src, model, in_, out, cr, w5, w1, calls, reasoning) = row?;
        let key = (grp.unwrap_or_else(|| "unknown".to_string()), src);
        let a = map.entry(key).or_default();
        a.input += in_;
        a.output += out;
        a.cache_read += cr;
        a.cache_write += w5 + w1;
        a.total += in_ + out + cr + w5 + w1;
        a.requests += calls;
        if let Some(r) = reasoning {
            a.reasoning = Some(a.reasoning.unwrap_or(0) + r);
        }
        if let Some(rt) = rates.resolve(&model) {
            a.cost += in_ as f64 * rt.input
                + out as f64 * rt.output
                + cr as f64 * rt.cache_read
                + w5 as f64 * rt.cache_write_5m
                + w1 as f64 * rt.cache_write_1h;
            a.priced += in_ + out + cr + w5 + w1;
            a.cache_estimated |= (cr > 0 && rt.cache_read == 0.0)
                || (w5 > 0 && rt.cache_write_5m == 0.0)
                || (w1 > 0 && rt.cache_write_1h == 0.0);
        }
    }

    // Convs at the row's own grain (distinct sessions can span models).
    let sql2 = format!(
        "SELECT {group_col} AS grp, {src_expr} AS src, COUNT(DISTINCT session_id) \
         FROM events {where_sql} GROUP BY grp, src"
    );
    let mut stmt2 = conn.prepare(&sql2)?;
    let crows = stmt2.query_map(params_from_iter(params.iter()), |r| {
        Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?))
    })?;
    for row in crows {
        let (grp, src, convs) = row?;
        let key = (grp.unwrap_or_else(|| "unknown".to_string()), src);
        if let Some(a) = map.get_mut(&key) {
            a.convs = convs;
        }
    }

    let mut out: Vec<BreakdownRow> = map
        .into_iter()
        .map(|((key, source), a)| BreakdownRow {
            key,
            source,
            input_tokens: a.input,
            output_tokens: a.output,
            cache_read_tokens: a.cache_read,
            cache_write_tokens: a.cache_write,
            total_tokens: a.total,
            requests: a.requests,
            cost: if a.priced > 0 { Some(a.cost) } else { None },
            reasoning_tokens: a.reasoning,
            convs: a.convs,
            cache_estimated: a.cache_estimated,
        })
        .collect();
    out.sort_by(|x, y| y.total_tokens.cmp(&x.total_tokens));
    Ok(out)
}
```

`Agg` gains:

```rust
    reasoning: Option<i64>,
    convs: i64,
    cache_estimated: bool,
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: all PASS (including the whole pre-existing suite — `e2e_real_logs` compiles because it only reads existing fields).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/queries.rs
git commit -m "feat(queries): cache-estimated models; breakdown source/convs/reasoning"
```

---

### Task 8: `series` IPC command + TS types/api

**Files:**
- Modify: `src-tauri/src/lib.rs`
- Modify: `src/types.ts`
- Modify: `src/api.ts`

**Interfaces:**
- Consumes: `queries::series`, `queries::SeriesPoint`.
- Produces: invokable `series(filters, bucket)` command; TS `SeriesPoint`, `fetchSeries(filters, bucket)`; TS `Summary.cacheEstimatedModels`, `BreakdownRow.{source, reasoningTokens, convs, cacheEstimated}`.

- [ ] **Step 1: Implement Rust command** — in `src-tauri/src/lib.rs`, extend the import:

```rust
use queries::{BreakdownRow, Filters, SeriesPoint, Summary, TrendPoint};
```

add after the `trend` command:

```rust
#[tauri::command]
fn series(
    state: State<'_, AppState>,
    filters: Filters,
    bucket: String,
) -> Result<Vec<SeriesPoint>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    queries::series(&db, &filters, &bucket).map_err(|e| e.to_string())
}
```

and add `series,` to the `generate_handler![…]` list (after `trend`).

- [ ] **Step 2: Verify Rust compiles and tests pass**

Run: `cargo test`
Expected: all PASS.

- [ ] **Step 3: Implement TS types** — in `src/types.ts`:

Add to `Summary` (after `unpricedModels`):

```ts
  cacheEstimatedModels: string[];
```

Add to `BreakdownRow` (after `cost`):

```ts
  source: string | null;          // set only for by='model'
  reasoningTokens: number | null; // null = not reported by the source(s)
  convs: number;
  cacheEstimated: boolean;
```

Add after `TrendPoint`:

```ts
export interface SeriesPoint {
  bucket: string;                 // 'YYYY-MM-DD' (day) or 'YYYY-MM-DD HH:00' (hour)
  source: string;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  totalTokens: number;
  reasoningTokens: number | null;
  cost: number;
  requests: number;
  convs: number;
}
```

In `src/api.ts`, add `SeriesPoint` to the type import and:

```ts
export function fetchSeries(
  filters: Filters,
  bucket: 'day' | 'hour',
): Promise<SeriesPoint[]> {
  return invoke('series', { filters, bucket });
}
```

- [ ] **Step 4: Verify frontend compiles**

Run (repo root): `npm run build`
Expected: PASS (tsc + vite build).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/lib.rs src/types.ts src/api.ts
git commit -m "feat(ipc): series command + frontend types"
```

---

### Task 9: Shared formatters + real-data layer (`data.ts`)

**Files:**
- Modify: `src/lib/format.ts`
- Modify: `src/lib/format.test.ts`
- Create: `src/overview/data.ts`
- Create: `src/overview/data.test.ts`
- Modify: `src/overview/mock.ts`

**Interfaces:**
- Consumes: `SeriesPoint`, `BreakdownRow`, `Filters` from `src/types.ts`; `rangeToBounds` from `src/lib/dateRange.ts`.
- Produces (used by Tasks 10–14):
  - `src/lib/format.ts`: `fmtTok(n)`, `fmtUSD(n)`, `fmtPct(x)`, `fmtDate(d)`, `fmtIsoDate(iso)` (bodies identical to today's mock versions).
  - `src/overview/data.ts`: `ToolKey`, `ToolMeta`, `TOOLS`, `CATEGORIES`, `THEMES`, `THEME_OPTIONS`, `Day` (now with `cost: number`), `Bucket`, `TableRow` (`reasoning: number | null`), `Range8b`, `RANGES_8B`, `emptyByTool()`, `seriesToDays(points, today?)`, `windowOf(range, from, to, today?)`, `pointsIn(points, win)`, `granularityOf(range, spanDays)`, `bucketsFromPoints(pts, per)`, `smallMultiples(bks)`, `toolTotalsOfPoints(pts)`, `sumPoints(pts)`, `catTotals(pts, tool)`, `dailyTableRows(pts)`, `projectTableRows(rows)`, `modelBars(rows, tool, toolTokens)`, `rangeToFilters(range, from, to)`.
  - `src/overview/mock.ts` re-exports all moved names so 8a files compile unchanged.

- [ ] **Step 1: Move formatters** — append to `src/lib/format.ts` (bodies copied verbatim from `mock.ts`):

```ts
// Compact token formatter for the Overview design (K/M/B with adaptive precision).
export function fmtTok(n: number): string {
  if (n >= 1e9) return (n / 1e9).toFixed(2) + 'B';
  if (n >= 1e6) return (n / 1e6).toFixed(n >= 1e7 ? 1 : 2) + 'M';
  if (n >= 1e3) return (n / 1e3).toFixed(n >= 1e5 ? 0 : 1) + 'K';
  return String(n);
}
export function fmtUSD(n: number): string {
  return '$' + n.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}
export function fmtPct(x: number): string {
  return (x * 100).toFixed(x < 0.1 ? 1 : 0) + '%';
}
export function fmtDate(d: Date): string {
  return d.toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
}
export function fmtIsoDate(iso: string): string {
  const [y, m, d] = iso.split('-').map(Number);
  return fmtDate(new Date(y, m - 1, d));
}
```

In `src/lib/format.test.ts`, extend the existing import from `'./format'` with `fmtTok, fmtPct, fmtIsoDate` (the file already imports `describe`/`it`/`expect` — reuse its existing style) and append:

```ts
describe('overview formatters', () => {
  it('fmtTok scales K/M/B', () => {
    expect(fmtTok(950)).toBe('950');
    expect(fmtTok(1500)).toBe('1.5K');
    expect(fmtTok(2_340_000)).toBe('2.34M');
    expect(fmtTok(1_200_000_000)).toBe('1.20B');
  });
  it('fmtPct adapts precision below 10%', () => {
    expect(fmtPct(0.5)).toBe('50%');
    expect(fmtPct(0.043)).toBe('4.3%');
  });
  it('fmtIsoDate renders a local date', () => {
    expect(fmtIsoDate('2026-07-04')).toBe('Jul 4');
  });
});
```

(Match this file's existing import style for `describe`/`it`/`expect`.)

- [ ] **Step 2: Write the failing data-layer tests** — create `src/overview/data.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import type { SeriesPoint, BreakdownRow } from '../types';
import {
  seriesToDays,
  windowOf,
  pointsIn,
  bucketsFromPoints,
  dailyTableRows,
  projectTableRows,
  modelBars,
  catTotals,
  rangeToFilters,
} from './data';

function pt(over: Partial<SeriesPoint>): SeriesPoint {
  return {
    bucket: '2026-07-09',
    source: 'claude',
    inputTokens: 100,
    outputTokens: 50,
    cacheReadTokens: 200,
    cacheWriteTokens: 30,
    totalTokens: 380,
    reasoningTokens: null,
    cost: 0.5,
    requests: 2,
    convs: 1,
    ...over,
  };
}

const TODAY = new Date(2026, 6, 10); // 2026-07-10 local

describe('seriesToDays', () => {
  it('builds a trailing 365-day window ending today', () => {
    const days = seriesToDays([], TODAY);
    expect(days).toHaveLength(365);
    expect(days[364].iso).toBe('2026-07-10');
    expect(days[0].iso).toBe('2025-07-11');
    expect(days.every((d) => d.tokens === 0 && d.level === 0)).toBe(true);
  });
  it('fills byTool, cost, and quartile levels', () => {
    const pts = [
      pt({ bucket: '2026-07-09', source: 'claude', totalTokens: 100, cost: 0.1 }),
      pt({ bucket: '2026-07-09', source: 'codex', totalTokens: 300, cost: 0.2 }),
      pt({ bucket: '2026-07-08', source: 'claude', totalTokens: 1000, cost: 1 }),
    ];
    const days = seriesToDays(pts, TODAY);
    const d9 = days.find((d) => d.iso === '2026-07-09')!;
    expect(d9.tokens).toBe(400);
    expect(d9.byTool.claude).toBe(100);
    expect(d9.byTool.codex).toBe(300);
    expect(d9.cost).toBeCloseTo(0.3);
    const d8 = days.find((d) => d.iso === '2026-07-08')!;
    expect(d8.level).toBeGreaterThanOrEqual(d9.level);
    expect(d9.level).toBeGreaterThan(0);
  });
});

describe('windowOf + pointsIn', () => {
  const pts = [
    pt({ bucket: '2026-07-10' }),
    pt({ bucket: '2026-07-04' }),
    pt({ bucket: '2026-05-01' }),
  ];
  it('day = today only', () => {
    const win = windowOf('day', '', '', TODAY);
    expect(pointsIn(pts, win).map((p) => p.bucket)).toEqual(['2026-07-10']);
  });
  it('week = trailing 7 days', () => {
    const win = windowOf('week', '', '', TODAY);
    expect(pointsIn(pts, win)).toHaveLength(2);
  });
  it('total = everything', () => {
    expect(pointsIn(pts, windowOf('total', '', '', TODAY))).toHaveLength(3);
  });
  it('custom = inclusive bounds', () => {
    const win = windowOf('custom', '2026-05-01', '2026-07-04', TODAY);
    expect(pointsIn(pts, win)).toHaveLength(2);
  });
});

describe('bucketsFromPoints', () => {
  it('daily buckets keep per-tool splits', () => {
    const bks = bucketsFromPoints(
      [pt({ bucket: '2026-07-09', source: 'claude', totalTokens: 10 }),
       pt({ bucket: '2026-07-09', source: 'codex', totalTokens: 5 }),
       pt({ bucket: '2026-07-10', source: 'claude', totalTokens: 7 })],
      'day',
    );
    expect(bks).toHaveLength(2);
    expect(bks[0].byTool.claude).toBe(10);
    expect(bks[0].total).toBe(15);
  });
  it('hour buckets label by hour', () => {
    const bks = bucketsFromPoints([pt({ bucket: '2026-07-10 09:00' })], 'hour');
    expect(bks[0].label).toBe('9');
  });
});

describe('tables', () => {
  it('dailyTableRows keeps reasoning null when never reported', () => {
    const rows = dailyTableRows([
      pt({ bucket: '2026-07-09', reasoningTokens: null }),
      pt({ bucket: '2026-07-09', source: 'codex', reasoningTokens: 5 }),
      pt({ bucket: '2026-07-08', reasoningTokens: null }),
    ]);
    const d9 = rows.find((r) => r.label === '2026-07-09')!;
    expect(d9.reasoning).toBe(5);
    expect(d9.convs).toBe(2);
    const d8 = rows.find((r) => r.label === '2026-07-08')!;
    expect(d8.reasoning).toBeNull();
  });
  it('projectTableRows maps breakdown rows', () => {
    const row: BreakdownRow = {
      key: '/p/alpha', inputTokens: 1, outputTokens: 2, cacheReadTokens: 3,
      cacheWriteTokens: 4, totalTokens: 10, requests: 5, cost: null,
      source: null, reasoningTokens: null, convs: 2, cacheEstimated: false,
    };
    expect(projectTableRows([row])[0]).toEqual({
      label: '/p/alpha', total: 10, input: 1, output: 2, cached: 3, reasoning: null, convs: 2,
    });
  });
});

describe('modelBars + catTotals + rangeToFilters', () => {
  it('modelBars filters by source and carries the flag', () => {
    const rows: BreakdownRow[] = [
      { key: 'claude-opus-4-8', inputTokens: 10, outputTokens: 10, cacheReadTokens: 60,
        cacheWriteTokens: 20, totalTokens: 100, requests: 1, cost: 1.5,
        source: 'claude', reasoningTokens: null, convs: 1, cacheEstimated: true },
      { key: 'gpt-5.4', inputTokens: 1, outputTokens: 1, cacheReadTokens: 1,
        cacheWriteTokens: 1, totalTokens: 4, requests: 1, cost: null,
        source: 'codex', reasoningTokens: null, convs: 1, cacheEstimated: false },
    ];
    const bars = modelBars(rows, 'claude', 200);
    expect(bars).toHaveLength(1);
    expect(bars[0].share).toBeCloseTo(0.5);
    expect(bars[0].cacheEstimated).toBe(true);
    expect(bars[0].segs.map((s) => s.frac)).toEqual([0.1, 0.1, 0.6, 0.2]);
  });
  it('catTotals sums one tool', () => {
    const t = catTotals(
      [pt({ source: 'claude' }), pt({ source: 'codex', inputTokens: 999 })],
      'claude',
    );
    expect(t).toEqual({ input: 100, output: 50, cacheRead: 200, cacheWrite: 30 });
  });
  it('rangeToFilters maps presets through rangeToBounds', () => {
    expect(rangeToFilters('total', '', '')).toEqual({ tools: [], models: [], project: null });
    const f = rangeToFilters('custom', '2026-07-01', '2026-07-02');
    expect(f.startTs).toBeDefined();
    expect(f.endTs).toBeDefined();
  });
});
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `npx vitest run src/overview/data.test.ts`
Expected: FAIL — cannot resolve `./data`.

- [ ] **Step 4: Implement `src/overview/data.ts`:**

```ts
// Real-data layer for the Overview: shared design meta plus pure reshaping of
// backend responses (SeriesPoint/BreakdownRow) into the shapes the components
// consume. No fetching here — Overview8b orchestrates IPC calls.
import type { BreakdownRow, Filters, SeriesPoint, DateRange } from '../types';
import { rangeToBounds } from '../lib/dateRange';

export type ToolKey = 'claude' | 'codex' | 'gemini' | 'hermes';

export interface ToolMeta {
  key: ToolKey;
  label: string;
  source: string; // full source name, e.g. "Claude Code"
  color: string;
}

export const TOOLS: ToolMeta[] = [
  { key: 'claude', label: 'Claude', source: 'Claude Code', color: '#3b82f6' },
  { key: 'codex', label: 'Codex', source: 'Codex', color: '#37c98b' },
  { key: 'gemini', label: 'Gemini', source: 'Gemini CLI', color: '#e2a63b' },
  { key: 'hermes', label: 'Hermes', source: 'Hermes', color: '#f472b6' },
];

// The four canonical token categories (CONTEXT.md).
export const CATEGORIES = [
  { key: 'input', label: 'Input', color: '#7c5cff' },
  { key: 'output', label: 'Output', color: '#2fbf71' },
  { key: 'cacheRead', label: 'Cache read', color: '#3aa0ff' },
  { key: 'cacheWrite', label: 'Cache write', color: '#f0a03c' },
] as const;

// Heatmap ramps: index 0 = empty cell, 1..4 = ascending intensity.
export const THEMES: Record<string, string[]> = {
  ocean: ['#12161f', '#173a63', '#1f5aa6', '#2f80ed', '#63a4ff'],
  emerald: ['#12161f', '#14503a', '#1a7d55', '#25a56f', '#4ad991'],
  neon: ['#12161f', '#312a63', '#4b3aa6', '#6d4fed', '#9a7cff'],
  amber: ['#12161f', '#5a4114', '#8a6417', '#c98f25', '#f0b84a'],
};
export const THEME_OPTIONS = [
  { value: 'ocean', label: 'Blue' },
  { value: 'emerald', label: 'Green' },
  { value: 'neon', label: 'Violet' },
  { value: 'amber', label: 'Amber' },
];

export interface Day {
  index: number;
  date: Date;
  iso: string;
  weekday: number; // 0 = Sun
  col: number;
  row: number;
  tokens: number;
  cost: number;
  level: 0 | 1 | 2 | 3 | 4;
  byTool: Record<ToolKey, number>;
}

export interface Bucket {
  label: string;
  byTool: Record<ToolKey, number>;
  total: number;
}

export interface TableRow {
  label: string; // iso date (daily) or project path — also the sort key
  total: number;
  input: number;
  output: number;
  cached: number;            // cache read tokens
  reasoning: number | null;  // null = no contributing source reported it
  convs: number;
}

export type Range8b = 'day' | 'week' | 'month' | 'total' | 'custom';
export const RANGES_8B: { key: Range8b; label: string; long: string }[] = [
  { key: 'day', label: 'Day', long: 'Today' },
  { key: 'week', label: 'Week', long: 'Last 7 days' },
  { key: 'month', label: 'Month', long: 'Last 30 days' },
  { key: 'total', label: 'Total', long: 'All time' },
  { key: 'custom', label: 'Custom', long: 'Custom range' },
];

export function emptyByTool(): Record<ToolKey, number> {
  return { claude: 0, codex: 0, gemini: 0, hermes: 0 };
}

function isoOf(d: Date): string {
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return `${d.getFullYear()}-${m}-${day}`;
}

// ---- heatmap days ----

// Trailing 365 local days ending today, filled from per-(day, source) rows.
// Intensity levels come from the quartiles of the nonzero-day distribution.
export function seriesToDays(points: SeriesPoint[], today: Date = new Date()): Day[] {
  const byDate = new Map<string, SeriesPoint[]>();
  for (const p of points) {
    const arr = byDate.get(p.bucket);
    if (arr) arr.push(p);
    else byDate.set(p.bucket, [p]);
  }
  const end = new Date(today.getFullYear(), today.getMonth(), today.getDate());
  const start = new Date(end);
  start.setDate(start.getDate() - 364);
  const startDow = start.getDay();

  const days: Day[] = [];
  for (let i = 0; i < 365; i++) {
    const date = new Date(start);
    date.setDate(start.getDate() + i);
    const iso = isoOf(date);
    const byTool = emptyByTool();
    let tokens = 0;
    let cost = 0;
    for (const p of byDate.get(iso) ?? []) {
      if (p.source in byTool) byTool[p.source as ToolKey] += p.totalTokens;
      tokens += p.totalTokens;
      cost += p.cost;
    }
    const cell = i + startDow;
    days.push({
      index: i, date, iso, weekday: date.getDay(),
      col: Math.floor(cell / 7), row: cell % 7,
      tokens, cost, level: 0, byTool,
    });
  }

  const nonzero = days.filter((d) => d.tokens > 0).map((d) => d.tokens).sort((a, b) => a - b);
  const q = (f: number) => nonzero[Math.min(nonzero.length - 1, Math.floor(f * nonzero.length))] ?? 0;
  const [q1, q2, q3] = [q(0.25), q(0.5), q(0.75)];
  for (const d of days) {
    d.level = d.tokens <= 0 ? 0 : d.tokens <= q1 ? 1 : d.tokens <= q2 ? 2 : d.tokens <= q3 ? 3 : 4;
  }
  return days;
}

// ---- range windows over series points ----

export interface Window {
  fromIso?: string; // inclusive
  toIso?: string;   // inclusive
}

// Must agree with rangeToFilters/rangeToBounds: day = today, week = trailing 7
// local days, month = trailing 30, total = unbounded, custom = [from, to].
export function windowOf(range: Range8b, customFrom: string, customTo: string, today: Date = new Date()): Window {
  const end = new Date(today.getFullYear(), today.getMonth(), today.getDate());
  const iso = isoOf(end);
  const back = (n: number) => {
    const d = new Date(end);
    d.setDate(d.getDate() - n);
    return isoOf(d);
  };
  switch (range) {
    case 'day': return { fromIso: iso, toIso: iso };
    case 'week': return { fromIso: back(6), toIso: iso };
    case 'month': return { fromIso: back(29), toIso: iso };
    case 'total': return {};
    case 'custom': {
      const lo = customFrom <= customTo ? customFrom : customTo;
      const hi = customFrom <= customTo ? customTo : customFrom;
      return { fromIso: lo, toIso: hi };
    }
  }
}

export function pointsIn(points: SeriesPoint[], win: Window): SeriesPoint[] {
  return points.filter(
    (p) => (!win.fromIso || p.bucket >= win.fromIso) && (!win.toIso || p.bucket <= win.toIso.slice(0, 10) + '~'),
  );
}

export function rangeToFilters(range: Range8b, customFrom: string, customTo: string): Filters {
  const dr: DateRange =
    range === 'day' ? 'today'
    : range === 'week' ? '7d'
    : range === 'month' ? '30d'
    : range === 'total' ? 'all'
    : { start: customFrom, end: customTo };
  return { tools: [], models: [], project: null, ...rangeToBounds(dr) };
}

// ---- trend buckets ----

export type Granularity = 'hour' | 'day' | 'week' | 'month';
const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

// Adaptive granularity: hourly for a single day, daily up to a month,
// weekly up to ~a quarter, monthly beyond.
export function granularityOf(range: Range8b, spanDays: number): Granularity {
  if (range === 'day') return 'hour';
  if (range === 'week' || range === 'month') return 'day';
  return spanDays <= 31 ? 'day' : spanDays <= 120 ? 'week' : 'month';
}

function weekKey(iso: string): string {
  const [y, m, d] = iso.split('-').map(Number);
  const date = new Date(y, m - 1, d);
  date.setDate(date.getDate() - date.getDay()); // back to Sunday
  return isoOf(date);
}

export function bucketsFromPoints(pts: SeriesPoint[], per: Granularity): Bucket[] {
  const keyOf = (p: SeriesPoint) =>
    per === 'hour' ? p.bucket
    : per === 'day' ? p.bucket.slice(0, 10)
    : per === 'week' ? weekKey(p.bucket.slice(0, 10))
    : p.bucket.slice(0, 7); // YYYY-MM
  const map = new Map<string, Record<ToolKey, number>>();
  for (const p of pts) {
    const k = keyOf(p);
    const by = map.get(k) ?? emptyByTool();
    if (p.source in by) by[p.source as ToolKey] += p.totalTokens;
    map.set(k, by);
  }
  const labelOf = (k: string) =>
    per === 'hour' ? String(parseInt(k.slice(11, 13), 10))
    : per === 'day' ? String(parseInt(k.slice(8, 10), 10))
    : per === 'month' ? MONTHS[parseInt(k.slice(5, 7), 10) - 1]
    : k; // week: placeholder, renumbered below
  const out = [...map.entries()]
    .sort((a, b) => (a[0] < b[0] ? -1 : 1))
    .map(([k, by]) => ({
      label: labelOf(k),
      byTool: by,
      total: (Object.values(by) as number[]).reduce((a, b) => a + b, 0),
    }));
  if (per === 'week') out.forEach((b, i) => (b.label = 'W' + (i + 1)));
  return out;
}

// ---- aggregations ----

export function sumPoints(pts: SeriesPoint[]): number {
  return pts.reduce((a, p) => a + p.totalTokens, 0);
}

export function toolTotalsOfPoints(pts: SeriesPoint[]): Record<ToolKey, number> {
  const out = emptyByTool();
  for (const p of pts) if (p.source in out) out[p.source as ToolKey] += p.totalTokens;
  return out;
}

// Per-tool sparkline series over the buckets (small multiples).
export function smallMultiples(bks: Bucket[]) {
  const totals = emptyByTool();
  for (const b of bks) for (const t of TOOLS) totals[t.key] += b.byTool[t.key];
  const grand = (Object.values(totals) as number[]).reduce((a, b) => a + b, 0) || 1;
  return TOOLS.map((t) => ({
    ...t,
    total: totals[t.key],
    share: totals[t.key] / grand,
    series: bks.map((b) => b.byTool[t.key]),
  }));
}

export interface CatTotals {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
}

export function catTotals(pts: SeriesPoint[], tool: ToolKey): CatTotals {
  const t: CatTotals = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 };
  for (const p of pts) {
    if (p.source !== tool) continue;
    t.input += p.inputTokens;
    t.output += p.outputTokens;
    t.cacheRead += p.cacheReadTokens;
    t.cacheWrite += p.cacheWriteTokens;
  }
  return t;
}

// ---- table rows ----

export function dailyTableRows(pts: SeriesPoint[]): TableRow[] {
  const byDay = new Map<string, TableRow>();
  for (const p of pts) {
    const label = p.bucket.slice(0, 10);
    const r = byDay.get(label) ?? {
      label, total: 0, input: 0, output: 0, cached: 0, reasoning: null, convs: 0,
    };
    r.total += p.totalTokens;
    r.input += p.inputTokens;
    r.output += p.outputTokens;
    r.cached += p.cacheReadTokens;
    if (p.reasoningTokens != null) r.reasoning = (r.reasoning ?? 0) + p.reasoningTokens;
    // Sessions never span sources, so summing per-source convs within one day is safe.
    r.convs += p.convs;
    byDay.set(label, r);
  }
  return [...byDay.values()];
}

export function projectTableRows(rows: BreakdownRow[]): TableRow[] {
  return rows.map((r) => ({
    label: r.key,
    total: r.totalTokens,
    input: r.inputTokens,
    output: r.outputTokens,
    cached: r.cacheReadTokens,
    reasoning: r.reasoningTokens,
    convs: r.convs,
  }));
}

// ---- models panel ----

export interface ModelBar {
  name: string;
  tokens: number;
  cost: number | null; // null = unpriced
  share: number;       // of the tool's range total
  segs: { key: string; color: string; frac: number }[];
  cacheEstimated: boolean;
}

export function modelBars(rows: BreakdownRow[], tool: ToolKey, toolTokens: number): ModelBar[] {
  return rows
    .filter((r) => r.source === tool)
    .map((r) => {
      const total = Math.max(1, r.totalTokens);
      const cats = [r.inputTokens, r.outputTokens, r.cacheReadTokens, r.cacheWriteTokens];
      return {
        name: r.key,
        tokens: r.totalTokens,
        cost: r.cost,
        share: r.totalTokens / Math.max(1, toolTokens),
        segs: CATEGORIES.map((c, i) => ({ key: c.key, color: c.color, frac: cats[i] / total })),
        cacheEstimated: r.cacheEstimated,
      };
    });
}
```

Note on `pointsIn`: hour buckets look like `2026-07-10 09:00`; the `toIso + '~'` upper bound keeps them inside their day because `'~'` sorts after any digit or space. Day buckets are unaffected.

- [ ] **Step 5: Rewire `mock.ts`** — delete from `src/overview/mock.ts` the now-shared definitions and re-export them instead. At the top, replace the deleted blocks with:

```ts
import {
  TOOLS, CATEGORIES,
  type Day, type ToolKey, type Bucket, type TableRow, type Range8b,
} from './data';

export {
  TOOLS, CATEGORIES, THEMES, THEME_OPTIONS, RANGES_8B,
  type ToolKey, type ToolMeta, type Day, type Bucket, type TableRow, type Range8b,
} from './data';
export { fmtTok, fmtUSD, fmtPct, fmtDate, fmtIsoDate } from '../lib/format';
```

(The formatter names are re-exported only — mock.ts's own code never calls them, and an unused local import fails the strict build.)

Delete these now-duplicated definitions from `mock.ts`: `ToolKey`, `ToolMeta`, `TOOLS`, `CATEGORIES`, `THEMES`, `THEME_OPTIONS`, `Day`, `Bucket`, `Range8b`, `RANGES_8B`, `TableRow`, `fmtTok`, `fmtUSD`, `fmtPct`, `fmtDate`, `fmtIsoDate`. Keep everything else (PRNG, `buildDays`, `costOf`, `contextBreakdown`, `MODELS`, `categorySplit`, slicing helpers, table fakes, `INTERVALS`/`buckets` for 8a).

In `buildDays`, the `Day` literal now needs `cost` — since `costOf` is declared later in the file (function hoisting does not apply to `const`… `costOf` is a `function`? It is `export function costOf` — hoisted, safe):

```ts
    days.push({
      index: i,
      date,
      iso: isoOf(date),
      weekday,
      col: Math.floor(cell / 7),
      row: cell % 7,
      tokens,
      cost: costOf(tokens),
      level: levelOf(tokens),
      byTool: splitTools(tokens),
    });
```

Add the 8a adapter for the new ModelsList prop shape (Task 12 consumes this):

```ts
// 8a adapter: fake ModelBar rows from the static MODELS shares.
export function mockModelBars(tool: ToolKey, toolTokens: number) {
  return MODELS[tool].map((m) => {
    const tokens = Math.round(toolTokens * m.share);
    const segs = categorySplit(tool, tokens);
    const segTotal = Math.max(1, segs.reduce((a, c) => a + c.tokens, 0));
    return {
      name: m.name,
      tokens,
      cost: costOf(tokens),
      share: m.share,
      segs: segs.map((c) => ({ key: c.key, color: c.color, frac: c.tokens / segTotal })),
      cacheEstimated: false,
    };
  });
}
```

- [ ] **Step 6: Run tests and typecheck**

Run: `npx vitest run` then `npm run build`
Expected: all vitest suites PASS; build PASS.

- [ ] **Step 7: Commit**

```bash
git add src/lib/format.ts src/lib/format.test.ts src/overview/data.ts src/overview/data.test.ts src/overview/mock.ts
git commit -m "feat(overview): real-data layer (data.ts) + shared formatters"
```

---

### Task 10: Heatmap takes real days via props

**Files:**
- Modify: `src/overview/Heatmap.tsx`
- Modify: `src/overview/Overview.tsx` (8a call site)

**Interfaces:**
- Consumes: `Day`, `TOOLS`, `THEMES`, `THEME_OPTIONS` from `./data`; formatters from `../lib/format`.
- Produces: `<Heatmap days={Day[]} compact?={boolean} />` — `days` is required and always 365 entries (both `seriesToDays` and mock `DAYS` guarantee this).

- [ ] **Step 1: Implement** — in `Heatmap.tsx`:

Replace the import block:

```tsx
import { useEffect, useMemo, useRef, useState } from 'react';
import { TOOLS, THEMES, THEME_OPTIONS, type Day } from './data';
import { fmtTok, fmtUSD, fmtDate } from '../lib/format';
```

Change the signature:

```tsx
export default function Heatmap({ days, compact = false }: { days: Day[]; compact?: boolean }) {
```

Add derived stats after the state declarations (replacing every use of the old module constants):

```tsx
  const cols = useMemo(() => Math.max(1, ...days.map((d) => d.col)) + 1, [days]);
  const stats = useMemo(() => {
    const totalTokens = days.reduce((a, d) => a + d.tokens, 0);
    const activeDays = days.filter((d) => d.tokens > 0).length;
    let streak = 0, run = 0;
    for (const d of days) {
      if (d.tokens > 0) { run += 1; streak = Math.max(streak, run); } else run = 0;
    }
    const bestDay = days.reduce((a, d) => (d.tokens > a.tokens ? d : a), days[0]);
    return { totalTokens, activeDays, streak, bestDay };
  }, [days]);
```

Then mechanical renames throughout the component body: `DAYS` → `days` (three sites: `monthLabels` loop, 2D `days.map`, 3D `days.map` inside `three`), `COLS` → `cols` (two sites: `view2d`, `three`'s `cx`), `TOTAL_TOKENS` → `stats.totalTokens`, `ACTIVE_DAYS` → `stats.activeDays`, `LONGEST_STREAK` → `stats.streak`, and the best-day stat becomes:

```tsx
            <div className="tt-stat">
              <b>{fmtUSD(stats.bestDay.cost)}</b>
              <span>best · {fmtDate(stats.bestDay.date)}</span>
            </div>
```

Update the memo dependency arrays that now read props: `monthLabels` → `[days]`, `three` → `[yaw, ramp, days, cols]`.

Both call sites must pass the now-required prop to keep the build green:

In `src/overview/Overview.tsx` (8a), keep it rendering mock data:

```tsx
import { DAYS } from './mock';
...
            <Heatmap days={DAYS} />
```

In `src/overview/Overview8b.tsx`, add `DAYS` to its mock import and pass it temporarily (Task 14 replaces this with real days):

```tsx
              <Heatmap days={DAYS} compact />
```

- [ ] **Step 2: Verify**

Run: `npm run build`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/overview/Heatmap.tsx src/overview/Overview.tsx
git commit -m "refactor(overview): Heatmap takes days via props"
```

---

### Task 11: TokenBreakdown panel (real category breakdown)

**Files:**
- Create: `src/overview/TokenBreakdown.tsx`

**Interfaces:**
- Consumes: `CATEGORIES`, `CatTotals`, `ToolMeta` from `./data`; `fmtTok`, `fmtPct` from `../lib/format`.
- Produces: `<TokenBreakdown tool={ToolMeta} cats={CatTotals} />` — replaces `ContextBreakdown` in 8b only (`ContextBreakdown.tsx` stays untouched for 8a).

- [ ] **Step 1: Implement** — create `src/overview/TokenBreakdown.tsx`:

```tsx
import { CATEGORIES, type CatTotals, type ToolMeta } from './data';
import { fmtTok, fmtPct } from '../lib/format';

// Real per-tool token breakdown (8b right column): the four canonical token
// categories from the Ledger. Replaces the speculative context-content panel.
export default function TokenBreakdown({ tool, cats }: { tool: ToolMeta; cats: CatTotals }) {
  const rows = CATEGORIES.map((c) => ({ ...c, tokens: cats[c.key] }));
  const total = rows.reduce((a, r) => a + r.tokens, 0);
  const denomTotal = Math.max(1, total);
  const max = Math.max(1, ...rows.map((r) => r.tokens));
  // Cache Hit Rate (CONTEXT.md): cacheRead / (input + cacheRead + cacheWrite).
  const denom = cats.input + cats.cacheRead + cats.cacheWrite;
  const hit = denom > 0 ? cats.cacheRead / denom : 0;

  return (
    <>
      <div className="tt-ctx-title">
        <span className="dot" style={{ background: tool.color }} />
        {tool.source} Token Breakdown
      </div>
      <div className="tt-ctx-sub">
        Cache hit rate <b>{fmtPct(hit)}</b> · <b>{fmtTok(cats.cacheRead)}</b> reused /{' '}
        <b>{fmtTok(total)}</b> total
      </div>
      {rows.map((r) => (
        <div className="tt-ctx-row" key={r.key}>
          <span className="bar" style={{ width: (r.tokens / max) * 100 + '%', background: r.color }} />
          <span className="name">
            <span className="dot" style={{ background: r.color }} />
            {r.label}
          </span>
          <span className="vals">
            <span className="val">{fmtTok(r.tokens)}</span>
            <span className="rpct">{fmtPct(r.tokens / denomTotal)}</span>
          </span>
        </div>
      ))}
    </>
  );
}
```

- [ ] **Step 2: Verify**

Run: `npm run build`
Expected: PASS (component not yet mounted; Task 14 mounts it).

- [ ] **Step 3: Commit**

```bash
git add src/overview/TokenBreakdown.tsx
git commit -m "feat(overview): TokenBreakdown — real per-tool category panel"
```

---

### Task 12: ModelsList becomes pure-presentational

**Files:**
- Modify: `src/overview/ModelsList.tsx`
- Modify: `src/overview/FocusPanel.tsx` (8a call site)
- Modify: `src/overview/overview.css` (`.tt-tag`)

**Interfaces:**
- Consumes: `ModelBar`, `CATEGORIES`, `ToolMeta` from `./data`; formatters from `../lib/format`; `formatCost` from `../lib/format`.
- Produces: `<ModelsList tool toolTokens models={ModelBar[]} showCost? />`. 8b passes `modelBars(...)` (data.ts), 8a passes `mockModelBars(...)` (mock.ts, added in Task 9).

- [ ] **Step 1: Implement** — replace `src/overview/ModelsList.tsx` with:

```tsx
import { CATEGORIES, type ModelBar, type ToolMeta } from './data';
import { fmtTok, fmtPct, formatCost } from '../lib/format';

// Per-model token breakdown for one source. Each bar's filled width is the
// model's share of the source; inner segments are the four token categories.
export default function ModelsList({
  tool,
  toolTokens,
  models,
  showCost = true,
}: {
  tool: ToolMeta;
  toolTokens: number;
  models: ModelBar[];
  showCost?: boolean;
}) {
  return (
    <>
      <div className="tt-models-head">
        <div className="lbl">
          <span className="dot" style={{ background: tool.color }} />
          Models <span className="count">· {models.length}</span>
        </div>
        <span className="tot">{fmtTok(toolTokens)}</span>
      </div>
      {models.map((m) => (
        <div className="tt-model" key={m.name}>
          <div className="top">
            <span className="name">
              {m.name}
              {m.cacheEstimated && <span className="tt-tag">cache est.</span>}
            </span>
            <span className="figs">
              <span className="tok">{fmtTok(m.tokens)}</span>
              {showCost && <span className="cost">{formatCost(m.cost, false)}</span>}
              <span className="pct">{fmtPct(m.share)}</span>
            </span>
          </div>
          <div className="track">
            <div className="segs" style={{ width: m.share * 100 + '%' }}>
              {m.segs.map((c) => (
                <div key={c.key} style={{ width: c.frac * 100 + '%', background: c.color }} />
              ))}
            </div>
          </div>
        </div>
      ))}
      <div className="tt-legend" style={{ marginTop: 12, paddingTop: 12, borderTop: '1px solid rgba(255,255,255,.06)' }}>
        {CATEGORIES.map((c) => (
          <span className="item" key={c.key}>
            <span className="sw" style={{ background: c.color }} />
            {c.label}
          </span>
        ))}
      </div>
    </>
  );
}
```

In `src/overview/FocusPanel.tsx`, extend the mock import with `mockModelBars` and change the call site:

```tsx
import { TOOLS, TOTAL_TOKENS, TOTAL_COST, TOOL_TOTALS, contextBreakdown, mockModelBars, fmtTok, fmtUSD, fmtPct, type ToolKey } from './mock';
...
        <ModelsList tool={tool} toolTokens={toolTotal} models={mockModelBars(sel, toolTotal)} showCost={false} />
```

In `src/overview/Overview8b.tsx`, add `mockModelBars` to its mock import and update its call site temporarily (Task 14 replaces this with real rows):

```tsx
                <ModelsList tool={tool} toolTokens={view.toolTotals[sel]} models={mockModelBars(sel, view.toolTotals[sel])} />
```

Append to `src/overview/overview.css`:

```css
/* Small inline marker: model is Cache-Estimated (cache tokens unpriced). */
.tt-tag {
  display: inline-block;
  margin-left: 6px;
  padding: 1px 5px;
  border-radius: 4px;
  font-size: 9px;
  letter-spacing: 0.02em;
  color: #e2a63b;
  background: rgba(226, 166, 59, 0.12);
  border: 1px solid rgba(226, 166, 59, 0.3);
  vertical-align: 1px;
}
```

- [ ] **Step 2: Verify**

Run: `npm run build`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/overview/ModelsList.tsx src/overview/FocusPanel.tsx src/overview/overview.css
git commit -m "refactor(overview): ModelsList takes ModelBar rows; cache-est tag"
```

---

### Task 13: BreakdownTable takes real rows

**Files:**
- Modify: `src/overview/BreakdownTable.tsx`

**Interfaces:**
- Consumes: `TableRow` from `./data`; `fmtIsoDate` from `../lib/format`.
- Produces: `<BreakdownTable dailyRows={TableRow[]} projectRows={TableRow[]} />` (8b-only component; no other call sites).

- [ ] **Step 1: Implement** — replace imports and the component head of `src/overview/BreakdownTable.tsx`:

```tsx
import { useMemo, useState } from 'react';
import { type TableRow } from './data';
import { fmtIsoDate } from '../lib/format';
```

```tsx
// Daily breakdown / project usage — a tabbed, click-to-sort table (design 8b).
export default function BreakdownTable({
  dailyRows,
  projectRows,
}: {
  dailyRows: TableRow[];
  projectRows: TableRow[];
}) {
  const [tab, setTab] = useState<Tab>('daily');
  const [sort, setSort] = useState<{ key: SortKey; dir: 'asc' | 'desc' }>({ key: 'label', dir: 'desc' });

  const rows = tab === 'daily' ? dailyRows : projectRows;

  const sorted = useMemo(() => {
    const { key, dir } = sort;
    const num = (v: number | null) => (v == null ? -1 : v); // '—' sorts below 0
    return [...rows].sort((a, b) => {
      const av = a[key];
      const bv = b[key];
      const cmp =
        typeof av === 'string'
          ? av.localeCompare(bv as string)
          : num(av as number | null) - num(bv as number | null);
      return dir === 'asc' ? cmp : -cmp;
    });
  }, [rows, sort]);
```

In the header row, give the Reasoning column its availability tooltip — replace the header `cols.map` button with:

```tsx
          {cols.map((c) => (
            <button
              key={c.key}
              className={sort.key === c.key ? 'active' : ''}
              onClick={() => clickCol(c.key)}
              title={c.key === 'reasoning' ? 'Claude does not report reasoning separately' : undefined}
            >
              {c.label}
              <span className="arrow">{sort.key === c.key ? (sort.dir === 'asc' ? '▲' : '▼') : ''}</span>
            </button>
          ))}
```

And in the body row, render null reasoning as an em dash:

```tsx
            <span>{r.reasoning == null ? '—' : fmtInt(r.reasoning)}</span>
```

Everything else (tabs, `switchTab`, `clickCol`, `NUMCOLS`, `fmtInt`) stays as-is.

In `src/overview/Overview8b.tsx`, update the call site in the same task so the build stays green — temporarily pass empty rows (Task 14 wires the real ones), and drop the now-unused `dailyTableRows`/`projectTableRows` names from its mock import if tsc flags them:

```tsx
          <BreakdownTable dailyRows={[]} projectRows={[]} />
```

- [ ] **Step 2: Verify**

Run: `npm run build`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/overview/BreakdownTable.tsx src/overview/Overview8b.tsx
git commit -m "refactor(overview): BreakdownTable takes real TableRow props"
```

---

### Task 14: Wire Overview8b to the real backend

**Files:**
- Modify: `src/overview/Overview8b.tsx`
- Modify: `src/overview/overview.css` (`.tt-error`, loading dim)

**Interfaces:**
- Consumes: `scan`, `fetchSeries`, `fetchSummary`, `fetchBreakdown` from `../api`; everything from `./data`; `formatCost`, `fmtTok`, `fmtUSD`, `fmtPct` from `../lib/format`; `Heatmap` (days prop), `TokenBreakdown`, `ModelsList` (models prop), `BreakdownTable` (rows props).
- Produces: the mounted Overview rendering only real Ledger data. No import from `./mock` remains in `Overview8b.tsx`.

- [ ] **Step 1: Implement** — replace the import block and the `Overview8b` component in `src/overview/Overview8b.tsx` (keep `AggTrend` and `SmallMultiples` bodies unchanged apart from imports):

New imports:

```tsx
import { useEffect, useMemo, useState } from 'react';
import './overview.css';
import Heatmap from './Heatmap';
import TokenBreakdown from './TokenBreakdown';
import ModelsList from './ModelsList';
import BreakdownTable from './BreakdownTable';
import { scan, fetchSeries, fetchSummary, fetchBreakdown } from '../api';
import type { BreakdownRow, SeriesPoint, Summary } from '../types';
import {
  TOOLS,
  RANGES_8B,
  seriesToDays,
  windowOf,
  pointsIn,
  granularityOf,
  bucketsFromPoints,
  smallMultiples,
  toolTotalsOfPoints,
  sumPoints,
  catTotals,
  dailyTableRows,
  projectTableRows,
  modelBars,
  rangeToFilters,
  type Range8b,
  type ToolKey,
  type Bucket,
} from './data';
import { fmtTok, fmtPct, fmtIsoDate, formatCost } from '../lib/format';
```

New component body:

```tsx
const NAV = ['Overview', 'Insights', 'Models', 'Settings'];
const EMPTY_FILTERS = { tools: [], models: [], project: null };

function isoToday(): string {
  const d = new Date();
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return `${d.getFullYear()}-${m}-${day}`;
}

// Design 8b — "App · Overview", wired to the real Ledger. One unbounded daily
// series powers heatmap/trends/tables via client-side slicing; summary and
// breakdowns re-fetch per range; an hourly series serves the Day view.
export default function Overview8b() {
  const [nav, setNav] = useState('Overview');
  const [range, setRange] = useState<Range8b>('total');
  const [sel, setSel] = useState<ToolKey>('claude');
  const [customFrom, setCustomFrom] = useState('');
  const [customTo, setCustomTo] = useState('');

  const [allPoints, setAllPoints] = useState<SeriesPoint[] | null>(null);
  const [hourPoints, setHourPoints] = useState<SeriesPoint[]>([]);
  const [sum, setSum] = useState<Summary | null>(null);
  const [modelRows, setModelRows] = useState<BreakdownRow[]>([]);
  const [projRows, setProjRows] = useState<BreakdownRow[]>([]);
  const [error, setError] = useState<string | null>(null);

  // Mount: scan the logs, then load the whole ledger's daily series once.
  useEffect(() => {
    (async () => {
      try {
        const status = await scan();
        const errs = status.sources.filter((s) => s.error).map((s) => `${s.source}: ${s.error}`);
        if (errs.length) setError(errs.join(' · '));
      } catch (e) {
        setError(String(e));
      }
      try {
        setAllPoints(await fetchSeries(EMPTY_FILTERS, 'day'));
      } catch (e) {
        setError(String(e));
        setAllPoints([]);
      }
    })();
  }, []);

  const firstIso = allPoints?.length ? allPoints.reduce((a, p) => (p.bucket < a ? p.bucket : a), allPoints[0].bucket) : isoToday();
  const lastIso = isoToday();
  const cf = customFrom || firstIso;
  const ct = customTo || lastIso;

  // Per-range data: authoritative cost + right column + project table (+ hourly on Day).
  useEffect(() => {
    if (allPoints === null) return;
    const filters = rangeToFilters(range, cf, ct);
    fetchSummary(filters).then(setSum).catch((e) => setError(String(e)));
    fetchBreakdown('model', filters).then(setModelRows).catch((e) => setError(String(e)));
    fetchBreakdown('project', filters).then(setProjRows).catch((e) => setError(String(e)));
    if (range === 'day') {
      fetchSeries(filters, 'hour').then(setHourPoints).catch((e) => setError(String(e)));
    }
  }, [allPoints, range, cf, ct]);

  const view = useMemo(() => {
    const pts = allPoints ?? [];
    const days = seriesToDays(pts);
    const win = windowOf(range, cf, ct);
    const rpts = pointsIn(pts, win);
    const spanDays = new Set(rpts.map((p) => p.bucket.slice(0, 10))).size || 1;
    const per = granularityOf(range, spanDays);
    const trend = per === 'hour' ? bucketsFromPoints(hourPoints, 'hour') : bucketsFromPoints(rpts, per);
    return {
      days,
      rpts,
      total: sumPoints(rpts),
      toolTotals: toolTotalsOfPoints(rpts),
      per,
      trend,
      sparks: smallMultiples(trend),
      cats: catTotals(rpts, sel),
      dailyRows: dailyTableRows(rpts),
    };
  }, [allPoints, hourPoints, range, cf, ct, sel]);

  const rangeLabel =
    range === 'custom' ? `${fmtIsoDate(cf)} – ${fmtIsoDate(ct)}` : RANGES_8B.find((r) => r.key === range)!.long;
  const grand = view.total || 1;
  const tool = TOOLS.find((t) => t.key === sel)!;
  const loading = allPoints === null;

  return (
    <div className="tt">
      <div className={'tt-app' + (loading ? ' tt-loading' : '')}>
        <div className="tt-top">
          <div className="tt-brand">
            <div className="tt-logo">
              <i>T</i>
              <b>tokentracker</b>
            </div>
            <div className="tt-nav">
              {NAV.map((n) => (
                <button key={n} className={n === nav ? 'active' : ''} onClick={() => setNav(n)}>
                  {n}
                </button>
              ))}
            </div>
          </div>
          <div className="tt-top-right">
            <div className="tt-seg">
              {RANGES_8B.map((r) => (
                <button key={r.key} className={range === r.key ? 'active' : ''} onClick={() => setRange(r.key)}>
                  {r.label}
                </button>
              ))}
            </div>
            <span className="tt-avatar">BW</span>
          </div>
        </div>

        {error && <div className="tt-error">{error}</div>}

        {range === 'custom' && (
          <div className="tt-custom-row">
            <span className="lbl">Custom range</span>
            <input
              type="date"
              value={cf}
              min={firstIso}
              max={ct}
              onChange={(e) => e.target.value && setCustomFrom(e.target.value)}
            />
            <span className="to">to</span>
            <input
              type="date"
              value={ct}
              min={cf}
              max={lastIso}
              onChange={(e) => e.target.value && setCustomTo(e.target.value)}
            />
          </div>
        )}

        <div className="tt-b8-body">
          <div className="tt-b8-head">
            <div className="tt-eyebrow">Total tokens · {rangeLabel}</div>
            <div className="tt-b8-total">{fmtTok(sum?.totalTokens ?? view.total)}</div>
            <div className="tt-b8-cost">
              {sum ? formatCost(sum.cost, sum.hasUnpriced) : '…'} est.
              {sum?.hasUnpriced && (
                <span title={sum.unpricedModels.join(', ')}> · {sum.unpricedModels.length} unpriced</span>
              )}
            </div>
          </div>

          <div className="tt-split">
            {TOOLS.map((t) => (
              <div key={t.key} style={{ width: fmtPct(view.toolTotals[t.key] / grand), background: t.color }} />
            ))}
          </div>

          <div className="tt-toolcards">
            {TOOLS.map((t) => {
              const active = t.key === sel;
              const nModels = modelRows.filter((r) => r.source === t.key).length;
              return (
                <button
                  key={t.key}
                  className={'tt-toolcard' + (active ? ' active' : '')}
                  onClick={() => setSel(t.key)}
                  style={active ? { borderColor: t.color, background: t.color + '1e' } : undefined}
                >
                  <div className="lbl">
                    <span className="dot" style={{ background: t.color }} />
                    {t.label}
                  </div>
                  <div className="num">{fmtPct(view.toolTotals[t.key] / grand)}</div>
                  <div className="sub">{nModels} model{nModels === 1 ? '' : 's'}</div>
                </button>
              );
            })}
          </div>

          <div className="tt-b8-grid">
            <div className="tt-b8-col">
              <Heatmap days={view.days} compact />
              <AggTrend data={view.trend} per={view.per} rangeLabel={rangeLabel} />
              <SmallMultiples items={view.sparks} rangeLabel={rangeLabel} />
            </div>

            <div className="tt-b8-col">
              <div>
                <TokenBreakdown tool={tool} cats={view.cats} />
              </div>
              <div>
                <ModelsList
                  tool={tool}
                  toolTokens={view.toolTotals[sel]}
                  models={modelBars(modelRows, sel, view.toolTotals[sel])}
                />
              </div>
            </div>
          </div>

          <BreakdownTable dailyRows={view.dailyRows} projectRows={projectTableRows(projRows)} />
        </div>
      </div>
    </div>
  );
}
```

`AggTrend` and `SmallMultiples` keep their existing bodies — they now resolve `TOOLS`/`Bucket` from the `./data` import and `fmtTok` from `../lib/format` (already imported above) — with ONE fix inside `AggTrend`: real data can be empty (first load, empty range), and `data.reduce(…, data[0])` then crashes on `peak.total`. Replace the `peak` line with:

```tsx
  const peak = data.reduce((a, b) => (b.total > a.total ? b : a), data[0] ?? { label: '—', byTool: { claude: 0, codex: 0, gemini: 0, hermes: 0 }, total: 0 });
```

Delete the entire old `import { … } from './mock'` block (including the `DAYS`/`mockModelBars` temporaries from Tasks 10–13).

Append to `src/overview/overview.css`:

```css
/* Scan/query failure line under the top bar. */
.tt-error {
  margin: 8px 22px 0;
  padding: 6px 10px;
  border-radius: 6px;
  font-size: 11px;
  color: #f0876f;
  background: rgba(240, 135, 111, 0.08);
  border: 1px solid rgba(240, 135, 111, 0.25);
}
/* First-load state: dim until the initial series arrives. */
.tt-loading {
  opacity: 0.55;
  pointer-events: none;
}
```

- [ ] **Step 2: Verify build + unit tests**

Run: `npm run build && npx vitest run`
Expected: both PASS. Also confirm no mock import remains: `grep -n "from './mock'" src/overview/Overview8b.tsx` → no output.

- [ ] **Step 3: Verify against the real app**

Run: `npm run tauri dev` — then check in the opened window:
1. Totals/heatmap/trend show non-zero real data (this machine has ~3 days of Claude usage minimum).
2. Range buttons re-scope everything; Day shows hourly bars; Custom shows the date pickers.
3. Token Breakdown shows the four categories for the selected tool; Models list shows real model names.
4. The table's Daily tab shows real dates with Convs ≥ 1; Reasoning shows `—` for Claude-only days.

- [ ] **Step 4: Commit**

```bash
git add src/overview/Overview8b.tsx src/overview/overview.css
git commit -m "feat(overview): wire Overview8b to the real Ledger"
```

---

### Task 15: End-to-end verification

**Files:**
- No new code; fixes only if verification fails.

- [ ] **Step 1: Full Rust suite**

Run (from `src-tauri/`): `cargo test`
Expected: all PASS.

- [ ] **Step 2: Real-log e2e harness**

Run: `cargo test --release e2e_real_logs -- --ignored --nocapture`
Expected: per-source scans succeed; Claude totals match the values recorded in README/prior runs (parity with ccusage < 0.5%). This run uses a fresh temp DB, so it also proves a from-scratch v2 scan works.

- [ ] **Step 3: Real app-DB migration check**

The app's live DB migrates on next launch. After the Task 14 `npm run tauri dev` run, verify:

```bash
sqlite3 "$HOME/Library/Application Support/com.brianwong.tokenledger/tokenledger.db" \
  "PRAGMA user_version; SELECT COUNT(*), COUNT(session_id) FROM events;"
```

Expected: `2`; total row count unchanged from before (compare against the pre-migration count — record it first with the same query if the app hasn't been launched yet); `COUNT(session_id)` close to `COUNT(*)` (backfill re-scan filled sessions for every log still on disk).

- [ ] **Step 4: Frontend suite + build**

Run (repo root): `npx vitest run && npm run build`
Expected: PASS.

- [ ] **Step 5: Commit any fixes and push**

```bash
git push origin main
```

(Push to main is pre-authorized for this repo.)
