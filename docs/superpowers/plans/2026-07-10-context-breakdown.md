# Context Breakdown (real attribution, v3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore the Overview's Context Breakdown panel (Messages / System prompt / Reasoning / Tool calls / Custom agents / MCP servers / Skills) fed by real attribution estimated from each source's logs at scan time.

**Architecture:** Seven nullable `ctx_*` columns on the `events` table hold each event's attributed share of billed context (`input + cache_read + cache_write`), computed by the adapters during the existing scan (schema migration v2→v3). A `ctx_resources` table records distinct skills/MCP servers/agents/memory files per day for the meta line, and a Claude-only `session_ctx` table persists running per-session composition across byte-offset resumes. The `series` IPC payload gains summed ctx fields; a new `ctx_resources` command returns distinct counts; the frontend `ContextBreakdown.tsx` is repurposed to real props and mounted above TokenBreakdown in Overview8b.

**Tech Stack:** Rust (rusqlite, serde_json, Tauri 2), React + TypeScript, vitest.

**Spec:** `docs/superpowers/specs/2026-07-10-context-breakdown-design.md`

## Global Constraints

- **Honesty rule (v2, unchanged):** NULL means "source cannot attribute" and renders "—" — NEVER 0. SQLite `SUM` of all-NULL is NULL; preserve that through every layer (`Option<i64>` in Rust, `number | null` in TS).
- **Billed context** = `input_tokens + cache_read_tokens + cache_write_5m_tokens + cache_write_1h_tokens` (Task 1 amends the spec to include cache writes; a session's first call carries the system prompt as cache WRITE, not input, so the heuristic requires it).
- **Primary partition (exact):** where attribution exists, `ctx_messages + ctx_system + ctx_reasoning == billed` (construction makes it exact, not approximate: messages takes the rounding remainder).
- **Secondary subset:** `ctx_toolcalls`, `ctx_agents`, `ctx_mcp`, `ctx_skills` are each ≤ `ctx_messages` (overlapping subsets; they never sum to anything).
- **Estimator:** tokens ≈ bytes/4 of content (strings verbatim; non-string JSON serialized). Reported values beat estimates (Gemini `tokens.tool`).
- **Ledger rule:** events are never deleted; token counts on existing rows are immutable. ctx columns backfill via COALESCE/keep-max patterns only.
- `src/types.ts` mirrors Rust IPC structs with serde `rename_all = "camelCase"` — do not rename (`ctx_mcp` → `ctxMcp`).
- No new dependencies. Commit style: `feat(scope):` / `refactor(scope):` / `docs(spec):` as in git log.
- Run backend tests from `src-tauri/`: `cargo test`. Frontend: `npm test` (vitest), `npx tsc --noEmit` for types.

---

### Task 1: Spec amendments (billed-context definition, 8a adapter note)

Planning surfaced two corrections the approved spec needs before code:

1. Billed context must include cache writes. On a session's FIRST call the system
   prompt arrives almost entirely as `cache_creation` tokens (it is being written
   to cache), so "input + cache_read" would estimate the system prompt at ~0.
   `input + cache_read + cache_write` is also consistent with the Cache Hit Rate
   denominator already used by TokenBreakdown.
2. Repurposing `ContextBreakdown.tsx` to real props breaks the unmounted 8a
   variant's compile. The v2 precedent (ModelsList) added a mock adapter
   (`mockModelBars`) — we do the same with `mockCtxTotals` in `mock.ts`.

**Files:**
- Modify: `docs/superpowers/specs/2026-07-10-context-breakdown-design.md`

- [ ] **Step 1: Edit the spec**

In the **Schema** section, replace every occurrence of the parenthetical
`(`input_tokens + cache_read_tokens`)` / "(`input + cache_read`)" description of
billed context with:

```
billed context (`input + cache_read + cache_write_5m + cache_write_1h` — cache
writes are context being transmitted; on a session's first call the system
prompt arrives as cache_creation tokens, so excluding writes would blind the
system-prompt heuristic)
```

In **Attribution algorithm → Claude**, same substitution for "billed input".
In **Gemini**, no numeric change (Gemini reports no cache writes).

In the **Frontend** section, replace the final bullet
`- `mock.ts` and the unmounted 8a variant are untouched.` with:

```
- `mock.ts` gains a tiny `mockCtxTotals()` adapter (mock fractions reshaped
  into the real `CtxTotals` prop) so the unmounted 8a FocusPanel keeps
  compiling against the repurposed ContextBreakdown — the same pattern v2
  used for ModelsList (`mockModelBars`).
```

- [ ] **Step 2: Commit**

```bash
git add docs/superpowers/specs/2026-07-10-context-breakdown-design.md
git commit -m "docs(spec): ctx breakdown — billed context includes cache writes; 8a mock adapter"
```

---

### Task 2: Schema v3, CtxTokens, insert paths

**Files:**
- Modify: `src-tauri/src/types.rs`
- Modify: `src-tauri/src/db.rs`
- Modify (compile fixes only): `src-tauri/src/adapters/claude.rs`, `codex.rs`, `gemini.rs`, `hermes.rs`, `src-tauri/src/queries.rs` (test helper)

**Interfaces:**
- Produces: `types::CtxTokens { messages, system, reasoning, toolcalls, agents, mcp, skills: Option<i64> }` (derive `Debug, Clone, Copy, Default, PartialEq`); `UsageEvent.ctx: CtxTokens`; events-table columns `ctx_messages, ctx_system, ctx_reasoning, ctx_toolcalls, ctx_agents, ctx_mcp, ctx_skills` (all nullable INTEGER); tables `ctx_resources(source, kind, name, day)` and `session_ctx(session_id, msg_est, tool_est, mcp_est, skill_est, reas_est, sys_est, initialized, tainted)`; `PRAGMA user_version = 3`.
- Consumed by: every later task.

- [ ] **Step 1: Write the failing migration tests**

In `src-tauri/src/db.rs` tests module, add:

```rust
#[test]
fn v2_db_migrates_to_v3_preserving_events() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    // Build a genuine v2 database by hand.
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute_batch(SCHEMA_V2).unwrap();
        conn.execute(
            "INSERT INTO events (dedup_key, source, timestamp, model, project, api_calls, \
             input_tokens, output_tokens, cache_read_tokens, cache_write_5m_tokens, \
             cache_write_1h_tokens, source_file, session_id, reasoning_tokens) \
             VALUES ('claude:old:1','claude',1,'m',NULL,1,10,20,0,0,0,'f','s',NULL)",
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
    assert_eq!(v, 3);
    // Old row intact, ctx columns NULL.
    let (input, cm): (i64, Option<i64>) = conn
        .query_row(
            "SELECT input_tokens, ctx_messages FROM events WHERE dedup_key='claude:old:1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(input, 10);
    assert_eq!(cm, None);
    // Scan state cleared so the next scan re-parses every log (ctx backfill).
    let files: i64 = conn
        .query_row("SELECT COUNT(*) FROM scanned_files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(files, 0);
    // New tables exist.
    for table in ["ctx_resources", "session_ctx"] {
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "table {table} missing");
    }
}

#[test]
fn insert_backfills_ctx_on_existing_rows() {
    let (_dir, mut conn) = temp_db();
    insert_events(&mut conn, &[sample_event("claude:a:1", "f1.jsonl")]).unwrap();
    let mut new = sample_event("claude:a:1", "f1.jsonl");
    new.ctx.messages = Some(90);
    new.ctx.system = Some(15);
    new.ctx.reasoning = Some(10);
    let inserted = insert_events(&mut conn, &[new]).unwrap();
    assert_eq!(inserted, 0);
    let (cm, cs, cr): (Option<i64>, Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT ctx_messages, ctx_system, ctx_reasoning FROM events WHERE dedup_key='claude:a:1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((cm, cs, cr), (Some(90), Some(15), Some(10)));
}

#[test]
fn keep_max_ctx_follows_winner_and_backfills_on_tie() {
    let (_dir, mut conn) = temp_db();
    let mut first = sample_event("claude:k:1", "f1.jsonl");
    first.ctx.messages = Some(10);
    insert_events_keep_max_output(&mut conn, &[first]).unwrap();
    // Bigger output wins the row: its ctx values replace.
    let mut bigger = sample_event("claude:k:1", "f1.jsonl");
    bigger.output_tokens = 999;
    bigger.ctx.messages = Some(50);
    bigger.ctx.toolcalls = Some(5);
    insert_events_keep_max_output(&mut conn, &[bigger]).unwrap();
    let (cm, ct): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT ctx_messages, ctx_toolcalls FROM events WHERE dedup_key='claude:k:1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((cm, ct), (Some(50), Some(5)));
    // Tie (equal output): NULLs backfill, existing values stay.
    let mut tie = sample_event("claude:k:1", "f1.jsonl");
    tie.output_tokens = 999;
    tie.ctx.messages = Some(1); // must NOT overwrite 50
    tie.ctx.skills = Some(2);   // fills a NULL
    insert_events_keep_max_output(&mut conn, &[tie]).unwrap();
    let (cm2, cs2): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT ctx_messages, ctx_skills FROM events WHERE dedup_key='claude:k:1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((cm2, cs2), (Some(50), Some(2)));
}
```

- [ ] **Step 2: Run to verify failure**

Run (from `src-tauri/`): `cargo test db::tests`
Expected: FAIL — `SCHEMA_V2` used before extraction compiles? No — first failure is `no field ctx on type UsageEvent` (compile error). That counts: the interface doesn't exist yet.

- [ ] **Step 3: Implement types + schema + inserts**

`src-tauri/src/types.rs` — add after `UsageEvent` (and add the field):

```rust
/// Attributed share of an event's billed context (input + cache_read +
/// cache_write). NULL = the source cannot attribute that category.
/// messages/system/reasoning partition billed exactly; toolcalls/agents/
/// mcp/skills are overlapping subsets of messages.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CtxTokens {
    pub messages: Option<i64>,
    pub system: Option<i64>,
    pub reasoning: Option<i64>,
    pub toolcalls: Option<i64>,
    pub agents: Option<i64>,
    pub mcp: Option<i64>,
    pub skills: Option<i64>,
}
```

and in `UsageEvent` (after `reasoning_tokens`):

```rust
    pub ctx: CtxTokens,
```

`src-tauri/src/db.rs`:

Add after `SCHEMA_V2`:

```rust
// v3: context attribution. ctx_* columns hold each event's attributed share
// of billed context (see types::CtxTokens). ctx_resources powers the panel's
// meta line (distinct names per local day). session_ctx persists Claude's
// running composition across byte-offset resumes. Clearing scanned_files
// forces a full re-scan so existing rows backfill ctx via the conflict
// clauses below. Events are never deleted (Ledger rule).
// No BEGIN/COMMIT here: migrate() runs the batches inside its own
// BEGIN IMMEDIATE transaction.
const SCHEMA_V3: &str = "\
ALTER TABLE events ADD COLUMN ctx_messages INTEGER;
ALTER TABLE events ADD COLUMN ctx_system INTEGER;
ALTER TABLE events ADD COLUMN ctx_reasoning INTEGER;
ALTER TABLE events ADD COLUMN ctx_toolcalls INTEGER;
ALTER TABLE events ADD COLUMN ctx_agents INTEGER;
ALTER TABLE events ADD COLUMN ctx_mcp INTEGER;
ALTER TABLE events ADD COLUMN ctx_skills INTEGER;
CREATE TABLE IF NOT EXISTS ctx_resources (
  source TEXT NOT NULL,
  kind TEXT NOT NULL,
  name TEXT NOT NULL,
  day TEXT NOT NULL,
  PRIMARY KEY (source, kind, name, day)
);
CREATE TABLE IF NOT EXISTS session_ctx (
  session_id TEXT PRIMARY KEY,
  msg_est INTEGER NOT NULL DEFAULT 0,
  tool_est INTEGER NOT NULL DEFAULT 0,
  mcp_est INTEGER NOT NULL DEFAULT 0,
  skill_est INTEGER NOT NULL DEFAULT 0,
  reas_est INTEGER NOT NULL DEFAULT 0,
  sys_est INTEGER NOT NULL DEFAULT 0,
  initialized INTEGER NOT NULL DEFAULT 0,
  tainted INTEGER NOT NULL DEFAULT 0
);
DELETE FROM scanned_files;
DELETE FROM session_ctx;
PRAGMA user_version = 3;";
```

In `migrate()` add after the `version < 2` block:

```rust
        if version < 3 {
            conn.execute_batch(SCHEMA_V3)?;
        }
```

Replace `INSERT_SQL` and `REPLACE_SQL` (ctx columns backfill like session_id —
token counts stay immutable; update the comment above INSERT_SQL to say
"session_id/reasoning/ctx"):

```rust
const INSERT_SQL: &str = "INSERT INTO events \
(dedup_key, source, timestamp, model, project, api_calls, input_tokens, output_tokens, \
cache_read_tokens, cache_write_5m_tokens, cache_write_1h_tokens, source_file, session_id, reasoning_tokens, \
ctx_messages, ctx_system, ctx_reasoning, ctx_toolcalls, ctx_agents, ctx_mcp, ctx_skills) \
VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21) \
ON CONFLICT(dedup_key) DO UPDATE SET \
  session_id = COALESCE(excluded.session_id, events.session_id), \
  reasoning_tokens = COALESCE(excluded.reasoning_tokens, events.reasoning_tokens), \
  ctx_messages  = COALESCE(excluded.ctx_messages,  events.ctx_messages), \
  ctx_system    = COALESCE(excluded.ctx_system,    events.ctx_system), \
  ctx_reasoning = COALESCE(excluded.ctx_reasoning, events.ctx_reasoning), \
  ctx_toolcalls = COALESCE(excluded.ctx_toolcalls, events.ctx_toolcalls), \
  ctx_agents    = COALESCE(excluded.ctx_agents,    events.ctx_agents), \
  ctx_mcp       = COALESCE(excluded.ctx_mcp,       events.ctx_mcp), \
  ctx_skills    = COALESCE(excluded.ctx_skills,    events.ctx_skills)";

const REPLACE_SQL: &str = "INSERT OR REPLACE INTO events \
(dedup_key, source, timestamp, model, project, api_calls, input_tokens, output_tokens, \
cache_read_tokens, cache_write_5m_tokens, cache_write_1h_tokens, source_file, session_id, reasoning_tokens, \
ctx_messages, ctx_system, ctx_reasoning, ctx_toolcalls, ctx_agents, ctx_mcp, ctx_skills) \
VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)";
```

Every `stmt.execute(params![...])` on events (in `insert_events`,
`insert_events_keep_max_output`, `upsert_events`, `replace_file_events`)
gains the seven ctx params after `e.reasoning_tokens`:

```rust
                e.session_id, e.reasoning_tokens,
                e.ctx.messages, e.ctx.system, e.ctx.reasoning,
                e.ctx.toolcalls, e.ctx.agents, e.ctx.mcp, e.ctx.skills
```

In `insert_events_keep_max_output`'s SQL: extend the column list and VALUES to
the same 21 columns, and add per-ctx-column clauses to the `DO UPDATE SET`
(winner takes the new values; tie only fills NULLs — same rationale as the
session_id tie rule in the existing comment):

```rust
               ctx_messages  = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_messages  ELSE COALESCE(events.ctx_messages,  excluded.ctx_messages)  END, \
               ctx_system    = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_system    ELSE COALESCE(events.ctx_system,    excluded.ctx_system)    END, \
               ctx_reasoning = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_reasoning ELSE COALESCE(events.ctx_reasoning, excluded.ctx_reasoning) END, \
               ctx_toolcalls = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_toolcalls ELSE COALESCE(events.ctx_toolcalls, excluded.ctx_toolcalls) END, \
               ctx_agents    = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_agents    ELSE COALESCE(events.ctx_agents,    excluded.ctx_agents)    END, \
               ctx_mcp       = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_mcp       ELSE COALESCE(events.ctx_mcp,       excluded.ctx_mcp)       END, \
               ctx_skills    = CASE WHEN excluded.output_tokens > events.output_tokens THEN excluded.ctx_skills    ELSE COALESCE(events.ctx_skills,    excluded.ctx_skills)    END, \
```

(placed with the other CASE columns, before the final `output_tokens` clause).

Compile fixes — add `ctx: CtxTokens::default(),` (import `CtxTokens` where
needed, or use `Default::default()`) to every `UsageEvent { ... }` literal:
- `src-tauri/src/adapters/claude.rs` (`parse_line`)
- `src-tauri/src/adapters/codex.rs` (`events.push(UsageEvent { ... })`)
- `src-tauri/src/adapters/gemini.rs` (`events.push(UsageEvent { ... })`)
- `src-tauri/src/adapters/hermes.rs` (its event literal)
- `src-tauri/src/db.rs` tests `sample_event`
- `src-tauri/src/queries.rs` tests `ev()`

Update the three existing version assertions in db.rs tests
(`fresh_db_has_tables_and_user_version`, `open_is_idempotent`,
`v1_db_migrates_to_v2_preserving_events`) from `assert_eq!(version, 2)` /
`assert_eq!(v, 2)` to `3`, and rename `v1_db_migrates_to_v2_preserving_events`'s
intent comment accordingly (it now proves v1→v3 chains cleanly). In
`concurrent_opens_of_v1_db_both_succeed`, change the final assertion to `3`.

- [ ] **Step 4: Run tests**

Run: `cargo test` (from `src-tauri/`)
Expected: PASS (all existing + 3 new).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src
git commit -m "feat(db): schema v3 — ctx attribution columns, ctx_resources, session_ctx"
```

---

### Task 3: Claude attribution engine (pure module)

**Files:**
- Create: `src-tauri/src/adapters/claude_ctx.rs`
- Modify: `src-tauri/src/adapters/mod.rs` (add `pub mod claude_ctx;` — match the existing style, e.g. `mod claude;` lines; use `pub(crate)` visibility if that's the pattern)

**Interfaces:**
- Produces (consumed by Task 4):
  - `pub fn est(bytes: usize) -> i64` — bytes/4.
  - `pub struct Composition { pub msg: i64, pub tool: i64, pub mcp: i64, pub skill: i64, pub reas: i64, pub sys: i64, pub initialized: bool, pub tainted: bool }` (derive `Debug, Clone, Copy, Default, PartialEq`) with methods `attribute(&self, billed: i64) -> CtxTokens`, `init_system(&mut self, billed: i64)`, `reset_compact(&mut self)`.
  - `pub fn apply_user_line(comp: &mut Composition, v: &Value, tool_names: &HashMap<String, String>)`
  - `pub fn apply_assistant_content(comp: &mut Composition, v: &Value, tool_names: &mut HashMap<String, String>, resources: &mut Vec<(&'static str, String)>)` — resource kinds: `"skill" | "mcp_server" | "agent" | "memory_file"`.
  - `pub fn load_composition(conn: &Connection, session_id: &str) -> rusqlite::Result<Option<Composition>>`
  - `pub fn save_composition(conn: &Connection, session_id: &str, c: &Composition) -> rusqlite::Result<()>`
  - `pub fn record_resources(conn: &Connection, source: &str, rows: &[(&'static str, String, i64)]) -> rusqlite::Result<()>` — (kind, name, epoch_ts); day computed in SQL via `strftime('%Y-%m-%d', ?, 'unixepoch', 'localtime')`.

- [ ] **Step 1: Write the failing tests**

Create `src-tauri/src/adapters/claude_ctx.rs` with the tests first (module body
can be empty stubs that don't compile yet — the test run IS the failure):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn attribute_partitions_billed_exactly() {
        let c = Composition { msg: 750, tool: 200, mcp: 40, skill: 10, reas: 150, sys: 100, initialized: true, tainted: false };
        let ctx = c.attribute(10_000);
        // total known = 750 + 150 + 100 = 1000
        assert_eq!(ctx.system, Some(1_000));
        assert_eq!(ctx.reasoning, Some(1_500));
        assert_eq!(ctx.messages, Some(7_500)); // billed − system − reasoning: partition exact
        assert_eq!(ctx.messages.unwrap() + ctx.system.unwrap() + ctx.reasoning.unwrap(), 10_000);
        // secondaries are subsets of messages
        assert_eq!(ctx.toolcalls, Some(2_000));
        assert_eq!(ctx.mcp, Some(400));
        assert_eq!(ctx.skills, Some(100));
        assert_eq!(ctx.agents, None, "agents set by the caller, not the engine");
    }

    #[test]
    fn attribute_with_no_known_content_or_taint_is_all_null() {
        assert_eq!(Composition::default().attribute(5_000), crate::types::CtxTokens::default());
        let tainted = Composition { msg: 100, tainted: true, ..Default::default() };
        assert_eq!(tainted.attribute(5_000), crate::types::CtxTokens::default());
    }

    #[test]
    fn init_system_runs_once_from_first_call_remainder() {
        let mut c = Composition { msg: 300, ..Default::default() };
        c.init_system(2_000);
        assert_eq!(c.sys, 1_700);
        assert!(c.initialized);
        c.init_system(99_999); // second call: no-op
        assert_eq!(c.sys, 1_700);
    }

    #[test]
    fn user_text_line_adds_messages_and_resets_reasoning() {
        let mut c = Composition { reas: 500, ..Default::default() };
        let line = json!({"type":"user","message":{"role":"user","content":"abcdefgh"}});
        apply_user_line(&mut c, &line, &HashMap::new());
        assert_eq!(c.msg, 2); // 8 bytes / 4
        assert_eq!(c.reas, 0, "genuine user turn strips prior thinking from context");
    }

    #[test]
    fn tool_result_adds_to_messages_toolcalls_and_matched_subset() {
        let mut c = Composition { reas: 7, ..Default::default() };
        let mut names = HashMap::new();
        names.insert("tu1".to_string(), "mcp__pencil__get_screenshot".to_string());
        let line = json!({"type":"user","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"tu1","content":"xxxxxxxxxxxxxxxx"}
        ]}});
        apply_user_line(&mut c, &line, &names);
        assert_eq!(c.msg, 4);
        assert_eq!(c.tool, 4);
        assert_eq!(c.mcp, 4);
        assert_eq!(c.skill, 0);
        assert_eq!(c.reas, 7, "tool_result is not a user turn; thinking persists in-turn");
    }

    #[test]
    fn assistant_blocks_route_to_categories_and_collect_resources() {
        let mut c = Composition::default();
        let mut names = HashMap::new();
        let mut res: Vec<(&'static str, String)> = Vec::new();
        let line = json!({"type":"assistant","message":{"content":[
            {"type":"text","text":"tttttttt"},
            {"type":"thinking","thinking":"rrrrrrrrrrrr"},
            {"type":"tool_use","id":"a","name":"Skill","input":{"skill":"graphify"}},
            {"type":"tool_use","id":"b","name":"mcp__pencil__batch_get","input":{"x":1}},
            {"type":"tool_use","id":"c","name":"Task","input":{"subagent_type":"Explore"}},
            {"type":"tool_use","id":"d","name":"Read","input":{"file_path":"/Users/x/.claude/projects/-p/memory/MEMORY.md"}}
        ]}});
        apply_assistant_content(&mut c, &line, &mut names, &mut res);
        assert_eq!(c.msg, 2 + est_of(&json!({"skill":"graphify"})) + est_of(&json!({"x":1}))
            + est_of(&json!({"subagent_type":"Explore"}))
            + est_of(&json!({"file_path":"/Users/x/.claude/projects/-p/memory/MEMORY.md"})));
        assert_eq!(c.reas, 3); // 12 bytes / 4
        assert!(c.skill > 0 && c.mcp > 0);
        assert_eq!(names.get("b").unwrap(), "mcp__pencil__batch_get");
        assert!(res.contains(&("skill", "graphify".to_string())));
        assert!(res.contains(&("mcp_server", "pencil".to_string())));
        assert!(res.contains(&("agent", "Explore".to_string())));
        assert!(res.iter().any(|(k, n)| *k == "memory_file" && n.ends_with("MEMORY.md")));
    }

    // helper mirroring the engine's estimator for JSON values
    fn est_of(v: &serde_json::Value) -> i64 {
        est(serde_json::to_string(v).unwrap().len())
    }

    #[test]
    fn reset_compact_keeps_system_zeroes_rest() {
        let mut c = Composition { msg: 10, tool: 5, mcp: 2, skill: 1, reas: 4, sys: 100, initialized: true, tainted: false };
        c.reset_compact();
        assert_eq!(c, Composition { sys: 100, initialized: true, ..Default::default() });
    }

    #[test]
    fn composition_roundtrips_through_db() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_db(&dir.path().join("t.db")).unwrap();
        assert!(load_composition(&conn, "s1").unwrap().is_none());
        let c = Composition { msg: 1, tool: 2, mcp: 3, skill: 4, reas: 5, sys: 6, initialized: true, tainted: true };
        save_composition(&conn, "s1", &c).unwrap();
        assert_eq!(load_composition(&conn, "s1").unwrap(), Some(c));
    }

    #[test]
    fn record_resources_dedupes_per_day() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_db(&dir.path().join("t.db")).unwrap();
        let ts = 1_782_907_200i64; // some day
        record_resources(&conn, "claude", &[
            ("skill", "graphify".to_string(), ts),
            ("skill", "graphify".to_string(), ts + 60), // same local day → deduped
            ("mcp_server", "pencil".to_string(), ts),
        ]).unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM ctx_resources", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test claude_ctx`
Expected: compile FAIL (`Composition` etc. not defined).

- [ ] **Step 3: Implement the engine**

Above the tests in `claude_ctx.rs`:

```rust
// Claude context-attribution engine (spec: 2026-07-10-context-breakdown).
// Pure running-composition counters in estimated tokens (bytes/4); the
// adapter feeds every transcript line through here and asks attribute()
// for each API call's CtxTokens. Persistence lives here too because the
// counters must survive Claude's byte-offset resume between scans.
use crate::types::CtxTokens;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::collections::HashMap;

/// tokens ≈ bytes / 4 (labeled "est." end to end).
pub fn est(bytes: usize) -> i64 {
    (bytes / 4) as i64
}

/// Bytes of a content value: strings verbatim, everything else JSON-serialized.
fn content_bytes(v: &Value) -> usize {
    match v.as_str() {
        Some(s) => s.len(),
        None => serde_json::to_string(v).map(|s| s.len()).unwrap_or(0),
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Composition {
    pub msg: i64,   // all conversation content (tool/mcp/skill are subsets)
    pub tool: i64,  // ⊆ msg
    pub mcp: i64,   // ⊆ tool
    pub skill: i64, // ⊆ tool
    pub reas: i64,  // thinking, current turn only (API strips it across turns)
    pub sys: i64,   // system-prompt baseline, estimated at the session's first call
    pub initialized: bool,
    pub tainted: bool, // resumed mid-session with lost state: attribution stays NULL
}

impl Composition {
    /// Split `billed` (input + cache_read + cache_write) by the running
    /// composition. Partition is EXACT: messages takes the rounding remainder.
    pub fn attribute(&self, billed: i64) -> CtxTokens {
        let total = self.msg + self.reas + self.sys;
        if total <= 0 || self.tainted {
            return CtxTokens::default(); // all NULL — nothing known
        }
        let system = billed * self.sys / total;
        let reasoning = billed * self.reas / total;
        let messages = billed - system - reasoning;
        CtxTokens {
            messages: Some(messages),
            system: Some(system),
            reasoning: Some(reasoning),
            toolcalls: Some((billed * self.tool / total).min(messages)),
            agents: None, // sidechain attribution is the caller's call
            mcp: Some((billed * self.mcp / total).min(messages)),
            skills: Some((billed * self.skill / total).min(messages)),
        }
    }

    /// System prompt is absent from transcripts: estimate it once per session
    /// as the first call's billed context minus the content seen so far.
    pub fn init_system(&mut self, billed: i64) {
        if !self.initialized {
            self.sys = (billed - self.msg - self.reas).max(0);
            self.initialized = true;
        }
    }

    /// Compaction rebuilds the window: content counters reset, the system
    /// prompt (and its estimate) survives.
    pub fn reset_compact(&mut self) {
        *self = Composition { sys: self.sys, initialized: self.initialized, ..Default::default() };
    }
}

pub fn apply_user_line(comp: &mut Composition, v: &Value, tool_names: &HashMap<String, String>) {
    let content = &v["message"]["content"];
    if let Some(s) = content.as_str() {
        comp.msg += est(s.len());
        comp.reas = 0; // user turn: prior thinking leaves the context
        return;
    }
    let Some(blocks) = content.as_array() else { return };
    for b in blocks {
        match b["type"].as_str() {
            Some("tool_result") => {
                let n = est(content_bytes(&b["content"]));
                comp.msg += n;
                comp.tool += n;
                let name = b["tool_use_id"].as_str().and_then(|id| tool_names.get(id));
                match name.map(|s| s.as_str()) {
                    Some(s) if s.starts_with("mcp__") => comp.mcp += n,
                    Some("Skill") => comp.skill += n,
                    _ => {}
                }
            }
            Some("text") => {
                comp.msg += est(content_bytes(&b["text"]));
                comp.reas = 0;
            }
            _ => {}
        }
    }
}

pub fn apply_assistant_content(
    comp: &mut Composition,
    v: &Value,
    tool_names: &mut HashMap<String, String>,
    resources: &mut Vec<(&'static str, String)>,
) {
    let Some(blocks) = v["message"]["content"].as_array() else { return };
    for b in blocks {
        match b["type"].as_str() {
            Some("text") => comp.msg += est(content_bytes(&b["text"])),
            Some("thinking") => comp.reas += est(content_bytes(&b["thinking"])),
            Some("tool_use") => {
                let name = b["name"].as_str().unwrap_or("");
                let n = est(content_bytes(&b["input"]));
                comp.msg += n;
                comp.tool += n;
                if let Some(id) = b["id"].as_str() {
                    tool_names.insert(id.to_string(), name.to_string());
                }
                if let Some(rest) = name.strip_prefix("mcp__") {
                    comp.mcp += n;
                    let server = rest.split("__").next().unwrap_or(rest);
                    resources.push(("mcp_server", server.to_string()));
                } else if name == "Skill" {
                    comp.skill += n;
                    if let Some(s) = b["input"]["skill"].as_str() {
                        resources.push(("skill", s.to_string()));
                    }
                } else if name == "Task" || name == "Agent" {
                    let agent = b["input"]["subagent_type"].as_str().unwrap_or("agent");
                    resources.push(("agent", agent.to_string()));
                } else if name == "Read" {
                    if let Some(p) = b["input"]["file_path"].as_str() {
                        if p.contains("/memory/") && p.ends_with("MEMORY.md") {
                            resources.push(("memory_file", p.to_string()));
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

pub fn load_composition(conn: &Connection, session_id: &str) -> rusqlite::Result<Option<Composition>> {
    conn.query_row(
        "SELECT msg_est, tool_est, mcp_est, skill_est, reas_est, sys_est, initialized, tainted \
         FROM session_ctx WHERE session_id = ?1",
        [session_id],
        |r| {
            Ok(Composition {
                msg: r.get(0)?, tool: r.get(1)?, mcp: r.get(2)?, skill: r.get(3)?,
                reas: r.get(4)?, sys: r.get(5)?,
                initialized: r.get::<_, i64>(6)? != 0,
                tainted: r.get::<_, i64>(7)? != 0,
            })
        },
    )
    .optional()
}

pub fn save_composition(conn: &Connection, session_id: &str, c: &Composition) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO session_ctx \
         (session_id, msg_est, tool_est, mcp_est, skill_est, reas_est, sys_est, initialized, tainted) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![session_id, c.msg, c.tool, c.mcp, c.skill, c.reas, c.sys,
                c.initialized as i64, c.tainted as i64],
    )?;
    Ok(())
}

pub fn record_resources(
    conn: &Connection,
    source: &str,
    rows: &[(&'static str, String, i64)],
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(
        "INSERT OR IGNORE INTO ctx_resources (source, kind, name, day) \
         VALUES (?1, ?2, ?3, strftime('%Y-%m-%d', ?4, 'unixepoch', 'localtime'))",
    )?;
    for (kind, name, ts) in rows {
        stmt.execute(params![source, kind, name, ts])?;
    }
    Ok(())
}
```

Add to `src-tauri/src/adapters/mod.rs`: `pub mod claude_ctx;` (match the file's existing `mod` style/visibility for the other adapters).

- [ ] **Step 4: Run tests**

Run: `cargo test claude_ctx`
Expected: PASS (9 tests).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/adapters/claude_ctx.rs src-tauri/src/adapters/mod.rs
git commit -m "feat(claude): context-attribution engine — composition counters, persistence, resources"
```

---

### Task 4: Wire attribution into the Claude adapter

**Files:**
- Modify: `src-tauri/src/adapters/claude.rs` (the `scan_file` loop + tests)

**Interfaces:**
- Consumes: everything Task 3 produces; `UsageEvent.ctx` from Task 2.
- Produces: Claude events carry populated `ctx`; `session_ctx` rows persist per session; `ctx_resources` rows recorded. Behavior contract for tests: sidechain/subagent events get `ctx.agents = Some(billed)`; a resumed file with no session_ctx row and offset > 0 taints the session (ctx all NULL from then on).

- [ ] **Step 1: Write the failing tests**

Add to `claude.rs` tests (uses the same tempdir/scan pattern as existing tests;
each JSON line is a plain string — keep them on one line):

```rust
#[test]
fn attributes_context_categories_across_a_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open_db(&dir.path().join("t.db")).unwrap();
    let root = dir.path().join("projects");
    let proj = root.join("x");
    std::fs::create_dir_all(&proj).unwrap();

    // user text (40 bytes → 10 est) → call 1 (billed 1000: input 100 + cw 900)
    // → assistant thinking (80 bytes → 20 est) + tool_use → tool_result
    // → call 2 (billed 2000: input 500 + cache_read 1500)
    let user1 = r#"{"type":"user","sessionId":"s1","timestamp":"2026-07-01T10:00:00.000Z","message":{"role":"user","content":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}"#;
    let call1 = r#"{"type":"assistant","sessionId":"s1","requestId":"r1","timestamp":"2026-07-01T10:00:01.000Z","cwd":"/p/x","message":{"id":"m1","model":"claude-opus-4-8","usage":{"input_tokens":100,"output_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":900}}}"#;
    let think = r#"{"type":"assistant","sessionId":"s1","requestId":"r1","timestamp":"2026-07-01T10:00:02.000Z","cwd":"/p/x","message":{"id":"m1","model":"claude-opus-4-8","content":[{"type":"thinking","thinking":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}],"usage":{"input_tokens":100,"output_tokens":30,"cache_read_input_tokens":0,"cache_creation_input_tokens":900}}}"#;
    let tooluse = r#"{"type":"assistant","sessionId":"s1","requestId":"r1","timestamp":"2026-07-01T10:00:03.000Z","cwd":"/p/x","message":{"id":"m1","model":"claude-opus-4-8","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls -la"}}],"usage":{"input_tokens":100,"output_tokens":40,"cache_read_input_tokens":0,"cache_creation_input_tokens":900}}}"#;
    let toolres = r#"{"type":"user","sessionId":"s1","timestamp":"2026-07-01T10:00:04.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"cccccccccccccccccccccccccccccccccccccccc"}]}}"#;
    let call2 = r#"{"type":"assistant","sessionId":"s1","requestId":"r2","timestamp":"2026-07-01T10:00:05.000Z","cwd":"/p/x","message":{"id":"m2","model":"claude-opus-4-8","usage":{"input_tokens":500,"output_tokens":10,"cache_read_input_tokens":1500,"cache_creation_input_tokens":0}}}"#;
    let lines = [user1, call1, think, tooluse, toolres, call2].join("\n") + "\n";
    std::fs::write(proj.join("s1.jsonl"), lines).unwrap();

    let res = scan_claude(&mut conn, &root);
    assert_eq!(res.error, None);
    assert_eq!(res.events_inserted, 2);

    // Call 1: composition = msg 10 (user text only) → sys initialized to 990.
    // Partition: total=1000, sys=990, reas=0 → system=990, reasoning=0, messages=10.
    let (m1, s1, r1, t1): (i64, i64, i64, i64) = conn.query_row(
        "SELECT ctx_messages, ctx_system, ctx_reasoning, ctx_toolcalls FROM events WHERE dedup_key='claude:m1:r1'",
        [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
    assert_eq!(s1, 990);
    assert_eq!(r1, 0);
    assert_eq!(m1, 10);
    assert_eq!(t1, 0);
    assert_eq!(m1 + s1 + r1, 1000, "partition exact");

    // Call 2 composition: msg 10 + tool_use input est + tool_result est(10),
    // reas 20 (in-turn thinking persists across tool_result), sys 990.
    // Just assert the invariants — exact split depends on JSON byte lengths.
    let (m2, s2, r2, t2): (i64, i64, i64, i64) = conn.query_row(
        "SELECT ctx_messages, ctx_system, ctx_reasoning, ctx_toolcalls FROM events WHERE dedup_key='claude:m2:r2'",
        [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
    assert_eq!(m2 + s2 + r2, 2000, "partition exact");
    assert!(r2 > 0, "thinking counted within its turn");
    assert!(t2 > 0 && t2 <= m2, "toolcalls subset of messages");
    assert!(s2 > 0 && s2 < 2000);
}

#[test]
fn sidechain_and_subagent_files_attribute_agents() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open_db(&dir.path().join("t.db")).unwrap();
    let root = dir.path().join("projects");
    let agent_dir = root.join("x/sess-a/subagents");
    std::fs::create_dir_all(&agent_dir).unwrap();
    let user = r#"{"type":"user","sessionId":"ag1","timestamp":"2026-07-01T09:00:00.000Z","message":{"role":"user","content":"task prompt here"}}"#;
    let call = r#"{"type":"assistant","sessionId":"ag1","requestId":"ra","timestamp":"2026-07-01T09:00:01.000Z","cwd":"/p/x","message":{"id":"ma","model":"claude-opus-4-8","usage":{"input_tokens":400,"output_tokens":5,"cache_read_input_tokens":600,"cache_creation_input_tokens":0}}}"#;
    std::fs::write(agent_dir.join("agent-1.jsonl"), format!("{user}\n{call}\n")).unwrap();

    let res = scan_claude(&mut conn, &root);
    assert_eq!(res.events_inserted, 1);
    let (agents, msgs): (i64, i64) = conn.query_row(
        "SELECT ctx_agents, ctx_messages FROM events WHERE dedup_key='claude:ma:ra'",
        [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(agents, 1000, "whole billed context attributed to agents");
    assert!(msgs > 0, "primary partition still computed for agent sessions");
}

#[test]
fn resume_with_lost_state_taints_session_to_null() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open_db(&dir.path().join("t.db")).unwrap();
    let root = dir.path().join("projects");
    let proj = root.join("x");
    std::fs::create_dir_all(&proj).unwrap();
    let logp = proj.join("s2.jsonl");
    let user = r#"{"type":"user","sessionId":"s2","timestamp":"2026-07-01T10:00:00.000Z","message":{"role":"user","content":"hello there friend"}}"#;
    let call1 = r#"{"type":"assistant","sessionId":"s2","requestId":"r1","timestamp":"2026-07-01T10:00:01.000Z","cwd":"/p/x","message":{"id":"m1","model":"claude-opus-4-8","usage":{"input_tokens":100,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    std::fs::write(&logp, format!("{user}\n{call1}\n")).unwrap();
    scan_claude(&mut conn, &root);

    // Simulate lost state (e.g. cleared out-of-band) between scans.
    conn.execute("DELETE FROM session_ctx", []).unwrap();

    let call2 = r#"{"type":"assistant","sessionId":"s2","requestId":"r2","timestamp":"2026-07-01T10:05:00.000Z","cwd":"/p/x","message":{"id":"m2","model":"claude-opus-4-8","usage":{"input_tokens":200,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&logp).unwrap();
        writeln!(f, "{call2}").unwrap();
    }
    scan_claude(&mut conn, &root);
    let cm: Option<i64> = conn.query_row(
        "SELECT ctx_messages FROM events WHERE dedup_key='claude:m2:r2'",
        [], |r| r.get(0)).unwrap();
    assert_eq!(cm, None, "resumed without state: NULL, never a guess");
}

#[test]
fn compact_boundary_resets_content_counters() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open_db(&dir.path().join("t.db")).unwrap();
    let root = dir.path().join("projects");
    let proj = root.join("x");
    std::fs::create_dir_all(&proj).unwrap();
    let user1 = r#"{"type":"user","sessionId":"s3","timestamp":"2026-07-01T10:00:00.000Z","message":{"role":"user","content":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}"#;
    let call1 = r#"{"type":"assistant","sessionId":"s3","requestId":"r1","timestamp":"2026-07-01T10:00:01.000Z","cwd":"/p/x","message":{"id":"m1","model":"claude-opus-4-8","usage":{"input_tokens":100,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":900}}}"#;
    let compact = r#"{"type":"system","subtype":"compact_boundary","sessionId":"s3","timestamp":"2026-07-01T11:00:00.000Z"}"#;
    let user2 = r#"{"type":"user","sessionId":"s3","timestamp":"2026-07-01T11:00:01.000Z","message":{"role":"user","content":"bbbb"}}"#;
    let call2 = r#"{"type":"assistant","sessionId":"s3","requestId":"r2","timestamp":"2026-07-01T11:00:02.000Z","cwd":"/p/x","message":{"id":"m2","model":"claude-opus-4-8","usage":{"input_tokens":100,"output_tokens":1,"cache_read_input_tokens":900,"cache_creation_input_tokens":0}}}"#;
    std::fs::write(proj.join("s3.jsonl"), [user1, call1, compact, user2, call2].join("\n") + "\n").unwrap();

    scan_claude(&mut conn, &root);
    // After compaction: composition = msg 1 (4 bytes user2), sys 990 → of 1000
    // billed, system ≈ 990/991·1000, messages the remainder.
    let (m2, s2): (i64, i64) = conn.query_row(
        "SELECT ctx_messages, ctx_system FROM events WHERE dedup_key='claude:m2:r2'",
        [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert!(s2 > 900, "system baseline survives compaction");
    assert!(m2 < 100, "pre-compaction messages no longer in the window");
}

#[test]
fn records_skill_and_mcp_resources() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open_db(&dir.path().join("t.db")).unwrap();
    let root = dir.path().join("projects");
    let proj = root.join("x");
    std::fs::create_dir_all(&proj).unwrap();
    let line = r#"{"type":"assistant","sessionId":"s4","requestId":"r1","timestamp":"2026-07-01T10:00:00.000Z","cwd":"/p/x","message":{"id":"m1","model":"claude-opus-4-8","content":[{"type":"tool_use","id":"t1","name":"Skill","input":{"skill":"graphify"}},{"type":"tool_use","id":"t2","name":"mcp__pencil__batch_get","input":{}}],"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
    std::fs::write(proj.join("s4.jsonl"), format!("{line}\n")).unwrap();
    scan_claude(&mut conn, &root);
    let rows: Vec<(String, String)> = {
        let mut stmt = conn.prepare("SELECT kind, name FROM ctx_resources WHERE source='claude' ORDER BY kind").unwrap();
        let it = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        it.collect::<rusqlite::Result<Vec<_>>>().unwrap()
    };
    assert_eq!(rows, vec![
        ("mcp_server".to_string(), "pencil".to_string()),
        ("skill".to_string(), "graphify".to_string()),
    ]);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test adapters::claude`
Expected: new tests FAIL (ctx columns NULL — attribution not wired).

- [ ] **Step 3: Implement the wiring**

In `claude.rs`:

Add imports:

```rust
use super::claude_ctx::{self, Composition};
use crate::types::CtxTokens;
use std::collections::HashMap;
```

Replace the middle of `scan_file` — from `let mut events = Vec::new();` through
the `insert_events_keep_max_output` call — with:

```rust
    // Context attribution (spec 2026-07-10): feed every line through the
    // running composition; attribute each API call at first sight of its
    // dedup_key (content the call produces is its output, not its input).
    let is_agent_file = path_str.contains("/subagents/");
    let mut events = Vec::new();
    let mut comps: HashMap<String, Composition> = HashMap::new();
    let mut tool_names: HashMap<String, String> = HashMap::new();
    let mut resources: Vec<(&'static str, String, i64)> = Vec::new();
    let mut attr_by_key: HashMap<String, CtxTokens> = HashMap::new();

    for line in buf[..consumed].split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_slice(line) {
            Ok(v) => v,
            Err(_) => {
                result.lines_skipped += 1;
                continue;
            }
        };
        // Session key: per-line sessionId, else the file stem (one session per file).
        let sid = v
            .get("sessionId")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
            });
        if !comps.contains_key(&sid) {
            let loaded = claude_ctx::load_composition(conn, &sid)?;
            let mut c = loaded.unwrap_or_default();
            // Mid-file resume with no persisted state: composition is unknowable —
            // taint the session so attribution stays NULL instead of guessing.
            if loaded.is_none() && start > 0 {
                c.tainted = true;
            }
            comps.insert(sid.clone(), c);
        }
        let comp = comps.get_mut(&sid).expect("inserted above");
        let line_ts = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .and_then(crate::time::iso_to_epoch)
            .unwrap_or(0);

        match v.get("type").and_then(|t| t.as_str()) {
            Some("user") => claude_ctx::apply_user_line(comp, &v, &tool_names),
            Some("system") => {
                if v.get("subtype").and_then(|s| s.as_str()) == Some("compact_boundary") {
                    comp.reset_compact();
                }
            }
            Some("assistant") => {
                if let Some(mut ev) = parse_line_event(&v, &path_str, &encoded_dir) {
                    let billed = ev.input_tokens
                        + ev.cache_read_tokens
                        + ev.cache_write_5m_tokens
                        + ev.cache_write_1h_tokens;
                    comp.init_system(billed);
                    let mut ctx = *attr_by_key
                        .entry(ev.dedup_key.clone())
                        .or_insert_with(|| comp.attribute(billed));
                    let sidechain = is_agent_file
                        || v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true);
                    if sidechain {
                        ctx.agents = Some(billed);
                    }
                    ev.ctx = ctx;
                    events.push(ev);
                }
                // Attribution first, THEN book this line's own content: what a
                // call produces is its output, not its input.
                let mut sink: Vec<(&'static str, String)> = Vec::new();
                claude_ctx::apply_assistant_content(comp, &v, &mut tool_names, &mut sink);
                resources.extend(sink.into_iter().map(|(k, n)| (k, n, line_ts)));
            }
            _ => {}
        }
    }

    let inserted = insert_events_keep_max_output(conn, &events)?;
    result.events_inserted += inserted;
    for (sid, comp) in &comps {
        claude_ctx::save_composition(conn, sid, comp)?;
    }
    claude_ctx::record_resources(conn, "claude", &resources)?;
```

(Remove the unused `let before/let _ = before;` pair — shown only to flag that
the sink drains into `resources` with the line timestamp.)

Rename the existing `parse_line(line: &[u8], ...)` to
`parse_line_event(v: &serde_json::Value, source_file: &str, encoded_dir: &str) -> Option<UsageEvent>`
— identical body except it starts from the already-parsed `Value` (drop the
`serde_json::from_slice` first line and the `Result` wrapper: the old function
only ever returned `Err` on malformed JSON, which the loop above now counts —
so `Ok(Some(ev))` → `Some(ev)` and `Ok(None)` → `None`). Its `UsageEvent`
literal keeps `ctx: CtxTokens::default()`. `lines_skipped` semantics are
unchanged: malformed JSON on any line type counts once, in the loop.

- [ ] **Step 4: Run tests**

Run: `cargo test adapters::claude`
Expected: PASS — all existing tests (they assert usage parsing, dedup,
resume, keep-max: unaffected) plus the 5 new ones.
If `parses_dedups_splits_and_skips` fixture counts change: they must NOT —
the fixture files contain only assistant lines and one malformed line, and
malformed counting still increments once per bad line.

- [ ] **Step 5: Run the full backend suite**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/adapters/claude.rs
git commit -m "feat(claude): wire context attribution into the transcript scan"
```

---

### Task 5: Codex attribution

**Files:**
- Modify: `src-tauri/src/adapters/codex.rs`

**Interfaces:**
- Consumes: `claude_ctx::est` (reuse the estimator; import as `use super::claude_ctx::est;`), `CtxTokens`.
- Produces: codex events with `ctx.messages`, `ctx.reasoning`, `ctx.toolcalls` set (others NULL). Messages counter includes function-call payloads (subset rule); shares normalize over known content, so the partition sums to billed by construction.

- [ ] **Step 1: Write the failing test**

Add to `codex.rs` tests:

```rust
#[test]
fn codex_attributes_context_from_response_items() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("sessions");
    write_rollout(&root, "rollout-2026-05-01-ctx.jsonl", &[
        r#"{"type":"response_item","timestamp":"2026-05-01T09:00:00.000Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}}"#,
        r#"{"type":"response_item","timestamp":"2026-05-01T09:00:01.000Z","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}]}}"#,
        r#"{"type":"response_item","timestamp":"2026-05-01T09:00:02.000Z","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"ls\"]}"}}"#,
        r#"{"type":"response_item","timestamp":"2026-05-01T09:00:03.000Z","payload":{"type":"function_call_output","output":"cccccccccccccccccccccccccccccccccccccccc"}}"#,
        r#"{"type":"event_msg","timestamp":"2026-05-01T09:00:04.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":900,"cached_input_tokens":100,"output_tokens":50,"total_tokens":950}}}}"#,
    ]);
    let mut conn = open_db(&tmp.path().join("t.db")).unwrap();
    let r = scan_codex(&mut conn, &root);
    assert_eq!(r.events_inserted, 1);

    let (cm, cs, cr, ct, ca): (i64, Option<i64>, i64, i64, Option<i64>) = conn
        .query_row(
            "SELECT ctx_messages, ctx_system, ctx_reasoning, ctx_toolcalls, ctx_agents \
             FROM events WHERE source='codex'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    // billed = Δinput(900, incl. cached) → partition exact over msg+reas.
    assert_eq!(cm + cr, 900, "messages + reasoning == billed (system NULL, absorbed)");
    assert!(cr > 0, "reasoning share attributed");
    assert!(ct > 0 && ct <= cm, "toolcalls ⊆ messages");
    assert_eq!(cs, None, "codex cannot attribute a system prompt");
    assert_eq!(ca, None, "codex has no agent concept");
}

#[test]
fn codex_user_message_resets_reasoning() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("sessions");
    write_rollout(&root, "rollout-2026-05-02-rst.jsonl", &[
        r#"{"type":"response_item","timestamp":"2026-05-02T09:00:00.000Z","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}]}}"#,
        r#"{"type":"response_item","timestamp":"2026-05-02T09:00:01.000Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}}"#,
        r#"{"type":"event_msg","timestamp":"2026-05-02T09:00:02.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":500,"cached_input_tokens":0,"output_tokens":10,"total_tokens":510}}}}"#,
    ]);
    let mut conn = open_db(&tmp.path().join("t.db")).unwrap();
    scan_codex(&mut conn, &root);
    let cr: i64 = conn
        .query_row("SELECT ctx_reasoning FROM events WHERE source='codex'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(cr, 0, "user turn strips prior reasoning from context");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test adapters::codex`
Expected: new tests FAIL (ctx columns NULL).

- [ ] **Step 3: Implement**

In `codex.rs` `scan_file`, add counters beside the existing `prev_*` state:

```rust
    // Running composition for context attribution (est. tokens, bytes/4).
    // Toolcall content is a subset of messages (schema subset rule); shares
    // normalize over known content so the unattributable system prompt is
    // absorbed proportionally and the partition sums to billed exactly.
    let mut msg_est: i64 = 0;
    let mut tool_est: i64 = 0;
    let mut reas_est: i64 = 0;
```

Add a `"response_item"` arm to the existing `match typ` (import
`use super::claude_ctx::est;`):

```rust
            "response_item" => {
                let payload = match v.get("payload") {
                    Some(p) => p,
                    None => continue,
                };
                let bytes = serde_json::to_string(payload).map(|s| s.len()).unwrap_or(0);
                match payload.get("type").and_then(|t| t.as_str()) {
                    Some("message") => {
                        msg_est += est(bytes);
                        if payload.get("role").and_then(|r| r.as_str()) == Some("user") {
                            reas_est = 0; // user turn: reasoning leaves the context
                        }
                    }
                    Some("function_call") | Some("function_call_output") => {
                        msg_est += est(bytes); // subset rule: tool ⊆ messages
                        tool_est += est(bytes);
                    }
                    Some("reasoning") => reas_est += est(bytes),
                    _ => {}
                }
            }
```

In the `token_count` arm, after computing `input`/`cache_read`/`output` and
passing the all-zero skip, build the attribution before `events.push`:

```rust
                let billed = input + cache_read; // codex reports no cache writes
                let total = msg_est + reas_est;
                let ctx = if total > 0 && billed > 0 {
                    let reasoning = billed * reas_est / total;
                    let messages = billed - reasoning; // partition exact
                    crate::types::CtxTokens {
                        messages: Some(messages),
                        system: None,
                        reasoning: Some(reasoning),
                        toolcalls: Some((billed * tool_est / total).min(messages)),
                        agents: None,
                        mcp: None,
                        skills: None,
                    }
                } else {
                    crate::types::CtxTokens::default()
                };
```

and set `ctx,` (replacing `ctx: CtxTokens::default(),`) in the `UsageEvent`
literal.

- [ ] **Step 4: Run tests**

Run: `cargo test adapters::codex`
Expected: PASS (existing 4 + new 2). The existing tests use fixtures without
response_items — their events now carry all-NULL ctx, asserted by nothing,
so they stay green.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/adapters/codex.rs
git commit -m "feat(codex): context attribution from rollout response items"
```

---

### Task 6: Gemini attribution + Hermes NULL check

**Files:**
- Modify: `src-tauri/src/adapters/gemini.rs`
- Modify: `src-tauri/src/adapters/hermes.rs` (test only — its events already default to all-NULL ctx)

**Interfaces:**
- Produces: gemini events with `ctx.messages = tokens.input` (raw input includes cached — the whole billed context) and `ctx.toolcalls = tokens.tool` clamped to messages (reported value, not estimated); all other ctx NULL. Hermes events: all ctx NULL.

- [ ] **Step 1: Write the failing tests**

`gemini.rs` — the fixtures already carry `"tool": 0`; add one message with a
nonzero tool count. In `SESSION_ALPHA`, change message `m2`'s tokens line to:

```
          "tokens": { "input": 500, "output": 100, "cached": 0, "thoughts": 0, "tool": 120, "total": 600 } }
```

and add to `test_scan_gemini_extracts_and_maps`:

```rust
        // Context attribution: messages = raw input (incl. cached) = billed;
        // toolcalls = reported tokens.tool; the rest NULL.
        let (cm, ct, cs, cr): (i64, i64, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT ctx_messages, ctx_toolcalls, ctx_system, ctx_reasoning \
                 FROM events WHERE dedup_key = 'gemini:sess-alpha:m1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(cm, 1000, "billed context = raw input incl. cached");
        assert_eq!(ct, 0);
        assert_eq!(cs, None);
        assert_eq!(cr, None, "thoughts are output-side, never re-sent as input");
        let (cm2, ct2): (i64, i64) = conn
            .query_row(
                "SELECT ctx_messages, ctx_toolcalls FROM events WHERE dedup_key = 'gemini:sess-alpha:m2'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(cm2, 500);
        assert_eq!(ct2, 120, "reported tokens.tool, not an estimate");
```

`hermes.rs` — add to its tests (mirror the existing test setup pattern in that
file for opening the fixture/temp DB; the only new assertion matter is):

```rust
    #[test]
    fn hermes_ctx_is_all_null() {
        // ...same setup as the existing scan test in this file...
        let nulls: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE source='hermes' AND (\
                 ctx_messages IS NOT NULL OR ctx_system IS NOT NULL OR \
                 ctx_reasoning IS NOT NULL OR ctx_toolcalls IS NOT NULL OR \
                 ctx_agents IS NOT NULL OR ctx_mcp IS NOT NULL OR ctx_skills IS NOT NULL)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nulls, 0, "hermes logs record no content: everything NULL");
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test adapters::gemini adapters::hermes`
Expected: gemini FAILS (ctx NULL); hermes PASSES already (Default) — that's
fine, it's a regression guard, keep it.

- [ ] **Step 3: Implement gemini**

In `gemini.rs`, extend `Tokens`:

```rust
#[derive(Deserialize)]
struct Tokens {
    input: i64,
    output: i64,
    cached: i64,
    thoughts: i64,
    #[serde(default)]
    tool: i64,
}
```

and in the `events.push(UsageEvent { ... })` literal replace
`ctx: CtxTokens::default(),` with:

```rust
            ctx: crate::types::CtxTokens {
                // Billed context = raw input (cached is a subset; no cache writes).
                messages: Some(tokens.input.max(0)),
                system: None,
                reasoning: None, // thoughts are output-side, never re-sent as input
                toolcalls: Some(tokens.tool.clamp(0, tokens.input.max(0))),
                agents: None,
                mcp: None,
                skills: None,
            },
```

- [ ] **Step 4: Run tests**

Run: `cargo test adapters::gemini adapters::hermes`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/adapters/gemini.rs src-tauri/src/adapters/hermes.rs
git commit -m "feat(gemini): reported tool/context attribution; hermes NULL guard"
```

---

### Task 7: Queries — series ctx sums + ctx_resources counts

**Files:**
- Modify: `src-tauri/src/queries.rs`

**Interfaces:**
- Produces:
  - `SeriesPoint` gains `pub ctx_messages: Option<i64>, pub ctx_system: Option<i64>, pub ctx_reasoning: Option<i64>, pub ctx_toolcalls: Option<i64>, pub ctx_agents: Option<i64>, pub ctx_mcp: Option<i64>, pub ctx_skills: Option<i64>` (serde camelCase → `ctxMessages` … `ctxMcp` …).
  - `pub struct CtxResourceCount { pub source: String, pub kind: String, pub count: i64 }` (Serialize, camelCase).
  - `pub fn ctx_resources(conn: &Connection, f: &Filters) -> rusqlite::Result<Vec<CtxResourceCount>>` — honors `tools` and start/end ts (day-granular); ignores models/project (resources aren't model-scoped).

- [ ] **Step 1: Write the failing tests**

Add to `queries.rs` tests:

```rust
    #[test]
    fn series_sums_ctx_preserving_null() {
        std::env::set_var("TZ", "UTC");
        let dir = tempdir().unwrap();
        let mut conn = db::open_db(&dir.path().join("t.db")).unwrap();
        let mut a = ev_s("a", "claude", DAY1_TS, "claude-opus-4-8", Some("s1"), None);
        a.ctx.messages = Some(900);
        a.ctx.system = Some(80);
        a.ctx.reasoning = Some(20);
        a.ctx.toolcalls = Some(300);
        let mut b = ev_s("b", "claude", DAY1_TS, "claude-opus-4-8", Some("s1"), None);
        b.ctx.messages = Some(100);
        b.ctx.system = Some(10);
        b.ctx.reasoning = Some(0);
        // hermes: all-NULL ctx must stay NULL, not become 0
        let h = ev_s("h", "hermes", DAY1_TS, "hermes-local", Some("hs"), None);
        db::insert_events(&mut conn, &[a, b, h]).unwrap();

        let pts = series(&conn, &Filters::default(), "day").unwrap();
        let c = pts.iter().find(|p| p.source == "claude").unwrap();
        assert_eq!(c.ctx_messages, Some(1000));
        assert_eq!(c.ctx_system, Some(90));
        assert_eq!(c.ctx_reasoning, Some(20));
        assert_eq!(c.ctx_toolcalls, Some(300));
        assert_eq!(c.ctx_agents, None, "no contributing value: NULL, never 0");
        let hm = pts.iter().find(|p| p.source == "hermes").unwrap();
        assert_eq!(hm.ctx_messages, None);
    }

    #[test]
    fn ctx_resources_counts_distinct_in_range() {
        std::env::set_var("TZ", "UTC");
        let dir = tempdir().unwrap();
        let conn = db::open_db(&dir.path().join("t.db")).unwrap();
        crate::adapters::claude_ctx::record_resources(&conn, "claude", &[
            ("skill", "graphify".to_string(), DAY1_TS),
            ("skill", "graphify".to_string(), DAY2_TS), // same name, new day: still 1 distinct
            ("skill", "verify".to_string(), DAY2_TS),
            ("mcp_server", "pencil".to_string(), DAY1_TS),
        ]).unwrap();

        let all = ctx_resources(&conn, &Filters::default()).unwrap();
        let skill = all.iter().find(|r| r.kind == "skill").unwrap();
        assert_eq!(skill.count, 2);
        let mcp = all.iter().find(|r| r.kind == "mcp_server").unwrap();
        assert_eq!(mcp.count, 1);

        // Day-1-only window excludes the day-2 'verify'.
        let f = Filters { start_ts: Some(DAY1_START), end_ts: Some(DAY2_START), ..Filters::default() };
        let d1 = ctx_resources(&conn, &f).unwrap();
        assert_eq!(d1.iter().find(|r| r.kind == "skill").unwrap().count, 1);

        // Tool filter scopes by source.
        let f2 = Filters { tools: vec!["codex".to_string()], ..Filters::default() };
        assert!(ctx_resources(&conn, &f2).unwrap().is_empty());
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test queries`
Expected: compile FAIL (no ctx fields on SeriesPoint / no ctx_resources fn).

- [ ] **Step 3: Implement**

`SeriesPoint` gains the seven fields (after `convs`):

```rust
    pub ctx_messages: Option<i64>,
    pub ctx_system: Option<i64>,
    pub ctx_reasoning: Option<i64>,
    pub ctx_toolcalls: Option<i64>,
    pub ctx_agents: Option<i64>,
    pub ctx_mcp: Option<i64>,
    pub ctx_skills: Option<i64>,
```

In `series()`: extend the first SQL's select list after `SUM(reasoning_tokens)`:

```
, SUM(ctx_messages), SUM(ctx_system), SUM(ctx_reasoning), SUM(ctx_toolcalls), SUM(ctx_agents), SUM(ctx_mcp), SUM(ctx_skills)
```

read them into the row tuple as `r.get::<_, Option<i64>>(10)?` … `(16)?` —
name them `cxm, cxs, cxr, cxt, cxa, cxmc, cxsk` (the obvious `cr` is already
taken by cache_read in this function) — initialize the new `SeriesPoint`
fields to `None`, and merge each like reasoning does (extract a tiny helper
to avoid 8 copies):

```rust
fn add_opt(acc: &mut Option<i64>, v: Option<i64>) {
    if let Some(x) = v {
        *acc = Some(acc.unwrap_or(0) + x);
    }
}
```

```rust
        // ... inside the row loop, after the existing reasoning merge:
        add_opt(&mut p.reasoning_tokens, reasoning); // replaces the existing if-let
        add_opt(&mut p.ctx_messages, cxm);
        add_opt(&mut p.ctx_system, cxs);
        add_opt(&mut p.ctx_reasoning, cxr);
        add_opt(&mut p.ctx_toolcalls, cxt);
        add_opt(&mut p.ctx_agents, cxa);
        add_opt(&mut p.ctx_mcp, cxmc);
        add_opt(&mut p.ctx_skills, cxsk);
```

(Place `fn add_opt` at module level near `series`.)

Add at the end of the file (before tests):

```rust
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CtxResourceCount {
    pub source: String,
    pub kind: String,
    pub count: i64,
}

// Distinct resources (skills / MCP servers / agents / memory files) seen in
// range, per source — the Context Breakdown meta line. Day-granular: the
// ctx_resources table dedups per local day, so ts bounds map to day strings
// (end_ts exclusive → day of end_ts − 1s inclusive).
pub fn ctx_resources(conn: &Connection, f: &Filters) -> rusqlite::Result<Vec<CtxResourceCount>> {
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
    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", clauses.join(" AND "))
    };
    let sql = format!(
        "SELECT source, kind, COUNT(DISTINCT name) FROM ctx_resources {where_sql} \
         GROUP BY source, kind"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter()), |r| {
        Ok(CtxResourceCount { source: r.get(0)?, kind: r.get(1)?, count: r.get(2)? })
    })?;
    rows.collect()
}
```

The `series_groups_by_day_and_source` / other existing tests construct
`SeriesPoint` only via the function under test — no literal updates needed.
The test-side `ev()` helper already carries `ctx: Default::default()` from
Task 2.

- [ ] **Step 4: Run tests**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/queries.rs
git commit -m "feat(queries): series ctx sums + ctx_resources distinct counts"
```

---

### Task 8: IPC command + frontend plumbing

**Files:**
- Modify: `src-tauri/src/lib.rs`
- Modify: `src/types.ts`
- Modify: `src/api.ts`

**Interfaces:**
- Produces: Tauri command `ctx_resources(filters) -> Vec<CtxResourceCount>`; TS `SeriesPoint` ctx fields; TS `CtxResourceCount { source: string; kind: string; count: number }`; `fetchCtxResources(filters: Filters): Promise<CtxResourceCount[]>`.

- [ ] **Step 1: Backend command**

In `lib.rs`: extend the queries import to include `CtxResourceCount`, add:

```rust
#[tauri::command]
fn ctx_resources(
    state: State<'_, AppState>,
    filters: Filters,
) -> Result<Vec<queries::CtxResourceCount>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    queries::ctx_resources(&db, &filters).map_err(|e| e.to_string())
}
```

and add `ctx_resources,` to the `generate_handler![...]` list.

- [ ] **Step 2: Frontend types + api**

`src/types.ts` — extend `SeriesPoint` (after `convs`):

```ts
  ctxMessages: number | null;
  ctxSystem: number | null;
  ctxReasoning: number | null;
  ctxToolcalls: number | null;
  ctxAgents: number | null;
  ctxMcp: number | null;
  ctxSkills: number | null;
```

and add:

```ts
export interface CtxResourceCount {
  source: string;
  kind: string; // 'skill' | 'mcp_server' | 'agent' | 'memory_file'
  count: number;
}
```

`src/api.ts` — import `CtxResourceCount` and add:

```ts
export function fetchCtxResources(filters: Filters): Promise<CtxResourceCount[]> {
  return invoke('ctx_resources', { filters });
}
```

- [ ] **Step 3: Verify compile**

Run: `cargo test` (from src-tauri/) and `npx tsc --noEmit` (repo root).
Expected: cargo PASS; tsc FAILS in `src/overview/data.test.ts` (`pt()` helper
missing the new SeriesPoint fields) — fix it now by adding to the `pt()`
literal in `src/overview/data.test.ts`:

```ts
    ctxMessages: null,
    ctxSystem: null,
    ctxReasoning: null,
    ctxToolcalls: null,
    ctxAgents: null,
    ctxMcp: null,
    ctxSkills: null,
```

Re-run `npx tsc --noEmit` → clean. `npm test` → PASS.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/lib.rs src/types.ts src/api.ts src/overview/data.test.ts
git commit -m "feat(ipc): ctx_resources command + SeriesPoint ctx fields"
```

---

### Task 9: data.ts — ctxTotals + ctxMeta

**Files:**
- Modify: `src/overview/data.ts`
- Test: `src/overview/data.test.ts`

**Interfaces:**
- Produces (consumed by Task 10):

```ts
export interface CtxTotals {
  billed: number;              // input + cacheRead + cacheWrite (context transmitted)
  reused: number;              // cacheRead
  messages: number | null;
  system: number | null;
  reasoning: number | null;
  toolcalls: number | null;
  agents: number | null;
  mcp: number | null;
  skills: number | null;
}
export function ctxTotals(pts: SeriesPoint[], tool: ToolKey): CtxTotals;
export function ctxMeta(res: CtxResourceCount[], tool: ToolKey): string; // '32 skills · 2 MCP servers · 1 agent · 1 memory file'
```

- [ ] **Step 1: Write the failing tests**

Add to `data.test.ts` (extend imports with `ctxTotals, ctxMeta` and
`CtxResourceCount` type):

```ts
describe('ctxTotals', () => {
  it('sums per-tool ctx preserving null (never 0)', () => {
    const pts: SeriesPoint[] = [
      pt({ source: 'claude', inputTokens: 100, cacheReadTokens: 200, cacheWriteTokens: 30,
           ctxMessages: 250, ctxSystem: 60, ctxReasoning: 20, ctxToolcalls: 90 }),
      pt({ source: 'claude', inputTokens: 50, cacheReadTokens: 0, cacheWriteTokens: 0,
           ctxMessages: 40, ctxSystem: 10, ctxReasoning: 0 }),
      pt({ source: 'codex', ctxMessages: 999 }), // other tool: excluded
    ];
    const t = ctxTotals(pts, 'claude');
    expect(t.billed).toBe(380); // (100+200+30) + (50+0+0)
    expect(t.reused).toBe(200);
    expect(t.messages).toBe(290);
    expect(t.system).toBe(70);
    expect(t.reasoning).toBe(20);
    expect(t.toolcalls).toBe(90); // one null contributor does not zero it
    expect(t.agents).toBeNull();  // nothing reported anywhere: null, not 0
  });

  it('all-null source stays all null (hermes)', () => {
    const t = ctxTotals([pt({ source: 'hermes' })], 'hermes');
    expect(t.messages).toBeNull();
    expect(t.billed).toBe(330); // header still real: 100+200+30
  });
});

describe('ctxMeta', () => {
  const res: CtxResourceCount[] = [
    { source: 'claude', kind: 'skill', count: 32 },
    { source: 'claude', kind: 'mcp_server', count: 2 },
    { source: 'claude', kind: 'agent', count: 1 },
    { source: 'claude', kind: 'memory_file', count: 1 },
    { source: 'codex', kind: 'mcp_server', count: 5 },
  ];
  it('renders counts in canonical order with pluralization', () => {
    expect(ctxMeta(res, 'claude')).toBe('32 skills · 2 MCP servers · 1 agent · 1 memory file');
  });
  it('omits zero kinds and scopes to the tool', () => {
    expect(ctxMeta(res, 'codex')).toBe('5 MCP servers');
    expect(ctxMeta(res, 'hermes')).toBe('');
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `npm test`
Expected: FAIL (`ctxTotals` is not exported).

- [ ] **Step 3: Implement in data.ts**

Add `CtxResourceCount` to the type import from `'../types'`, then append:

```ts
// ---- context breakdown panel ----

export interface CtxTotals {
  billed: number; // input + cacheRead + cacheWrite — the context transmitted
  reused: number; // cacheRead
  messages: number | null;
  system: number | null;
  reasoning: number | null;
  toolcalls: number | null;
  agents: number | null;
  mcp: number | null;
  skills: number | null;
}

// Null-preserving sum: a null contributor never zeroes the total, and a
// category nobody reported stays null (renders "—", same rule as reasoning).
function addOpt(a: number | null, b: number | null): number | null {
  return b == null ? a : (a ?? 0) + b;
}

export function ctxTotals(pts: SeriesPoint[], tool: ToolKey): CtxTotals {
  const t: CtxTotals = {
    billed: 0, reused: 0,
    messages: null, system: null, reasoning: null,
    toolcalls: null, agents: null, mcp: null, skills: null,
  };
  for (const p of pts) {
    if (p.source !== tool) continue;
    t.billed += p.inputTokens + p.cacheReadTokens + p.cacheWriteTokens;
    t.reused += p.cacheReadTokens;
    t.messages = addOpt(t.messages, p.ctxMessages);
    t.system = addOpt(t.system, p.ctxSystem);
    t.reasoning = addOpt(t.reasoning, p.ctxReasoning);
    t.toolcalls = addOpt(t.toolcalls, p.ctxToolcalls);
    t.agents = addOpt(t.agents, p.ctxAgents);
    t.mcp = addOpt(t.mcp, p.ctxMcp);
    t.skills = addOpt(t.skills, p.ctxSkills);
  }
  return t;
}

const CTX_KINDS: { kind: string; label: string }[] = [
  { kind: 'skill', label: 'skill' },
  { kind: 'mcp_server', label: 'MCP server' },
  { kind: 'agent', label: 'agent' },
  { kind: 'memory_file', label: 'memory file' },
];

export function ctxMeta(res: CtxResourceCount[], tool: ToolKey): string {
  const bits: string[] = [];
  for (const { kind, label } of CTX_KINDS) {
    const n = res.find((r) => r.source === tool && r.kind === kind)?.count ?? 0;
    if (n > 0) bits.push(`${n} ${label}${n === 1 ? '' : 's'}`);
  }
  return bits.join(' · ');
}
```

- [ ] **Step 4: Run tests**

Run: `npm test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/overview/data.ts src/overview/data.test.ts
git commit -m "feat(overview): ctxTotals + ctxMeta for the context panel"
```

---

### Task 10: ContextBreakdown rewrite + Overview8b mount + 8a adapter

**Files:**
- Modify: `src/overview/ContextBreakdown.tsx` (full rewrite to real props)
- Modify: `src/overview/Overview8b.tsx` (mount above TokenBreakdown; fetch resources)
- Modify: `src/overview/mock.ts` (add `mockCtxTotals` adapter)
- Modify: `src/overview/FocusPanel.tsx` (pass adapter output — keeps unmounted 8a compiling)

**Interfaces:**
- Consumes: `CtxTotals`, `ctxTotals`, `ctxMeta`, `fetchCtxResources`, `CtxResourceCount` from Tasks 8–9.
- Produces: `ContextBreakdown({ tool, ctx, meta }: { tool: ToolMeta; ctx: CtxTotals; meta: string })`.

- [ ] **Step 1: Rewrite ContextBreakdown.tsx**

Replace the whole file:

```tsx
import type { CtxTotals, ToolMeta } from './data';
import { fmtTok, fmtPct } from '../lib/format';

const PRIMARY = [
  { key: 'messages', label: 'Messages' },
  { key: 'system', label: 'System prompt', info: "Estimated from each session's first call" },
  { key: 'reasoning', label: 'Reasoning' },
] as const;

const SECONDARY = [
  { key: 'toolcalls', label: 'Tool calls' },
  { key: 'agents', label: 'Custom agents' },
  { key: 'mcp', label: 'MCP servers' },
  { key: 'skills', label: 'Skills' },
] as const;

// Context-window breakdown for one source, from real scan-time attribution.
// Primary rows partition billed context (input + cache read + cache write);
// secondary rows are overlapping subsets of Messages. null renders "—" —
// this source's logs can't attribute that category (never 0).
export default function ContextBreakdown({
  tool,
  ctx,
  meta,
}: {
  tool: ToolMeta;
  ctx: CtxTotals;
  meta: string;
}) {
  const hit = ctx.billed > 0 ? ctx.reused / ctx.billed : 0;
  const denom = Math.max(1, ctx.billed);
  const primary = PRIMARY.map((p) => ({ ...p, tokens: ctx[p.key] }));
  const primaryMax = Math.max(1, ...primary.map((p) => p.tokens ?? 0));
  const unattributed = primary.every((p) => p.tokens == null);

  return (
    <>
      <div className="tt-ctx-title">
        <span className="dot" style={{ background: tool.color }} />
        {tool.source} Context Breakdown
      </div>
      <div className="tt-ctx-sub">
        Cache hit rate <b>{fmtPct(hit)}</b> · <b>{fmtTok(ctx.reused)}</b> reused /{' '}
        <b>{fmtTok(ctx.billed)}</b> input · est.
      </div>
      {primary.map((p) => (
        <div className="tt-ctx-row" key={p.key}>
          {p.tokens != null && (
            <span
              className="bar"
              style={{ width: (p.tokens / primaryMax) * 100 + '%', background: tool.color }}
            />
          )}
          <span className="name">
            <span className="dot" style={{ background: tool.color }} />
            {p.label}
            {'info' in p && p.info && (
              <span className="aff" title={p.info}>
                ⓘ
              </span>
            )}
          </span>
          <span className="vals">
            {p.tokens == null ? (
              <span className="val">—</span>
            ) : (
              <>
                <span className="val">{fmtTok(p.tokens)}</span>
                <span className="rpct">{fmtPct(p.tokens / denom)}</span>
              </>
            )}
          </span>
        </div>
      ))}
      <div style={{ height: 1, background: 'rgba(255,255,255,.06)', margin: '8px 4px' }} />
      {SECONDARY.map((s) => (
        <div className="tt-ctx-row muted" key={s.key}>
          <span className="name">
            <span className="dot" style={{ background: '#4a5262' }} />
            {s.label}
          </span>
          <span className="vals">
            <span className="val">{ctx[s.key] == null ? '—' : fmtTok(ctx[s.key]!)}</span>
          </span>
        </div>
      ))}
      {unattributed ? (
        <div className="tt-ctx-meta" title="This source's logs don't record message content">
          no content attribution for this source
        </div>
      ) : (
        meta && <div className="tt-ctx-meta">{meta}</div>
      )}
    </>
  );
}
```

- [ ] **Step 2: Mock adapter for the unmounted 8a variant**

In `src/overview/mock.ts`, after `contextBreakdown`, add (v2 precedent:
`mockModelBars`):

```ts
// Adapter: mock fractions reshaped into the real ContextBreakdown props
// (CtxTotals + meta string), so the unmounted 8a FocusPanel keeps compiling.
export function mockCtxTotals(tool: ToolKey) {
  const c = contextBreakdown(tool);
  const grab = (arr: { key: string; tokens: number }[], key: string) =>
    arr.find((x) => x.key === key)?.tokens ?? null;
  return {
    ctx: {
      billed: c.input,
      reused: c.reused,
      messages: grab([...c.primary], 'messages'),
      system: grab([...c.primary], 'system'),
      reasoning: grab([...c.primary], 'reasoning'),
      toolcalls: grab([...c.secondary], 'toolcalls'),
      agents: grab([...c.secondary], 'agents'),
      mcp: grab([...c.secondary], 'mcp'),
      skills: grab([...c.secondary], 'skills'),
    },
    meta: c.meta,
  };
}
```

In `src/overview/FocusPanel.tsx`, add `mockCtxTotals` to the `./mock` import
list and replace `<ContextBreakdown tool={tool} />` with:

```tsx
        <ContextBreakdown tool={tool} {...mockCtxTotals(sel)} />
```

- [ ] **Step 3: Mount in Overview8b**

In `src/overview/Overview8b.tsx`:

- Imports: add `ContextBreakdown from './ContextBreakdown';`, add
  `fetchCtxResources` to the `../api` import, add `CtxResourceCount` to the
  `../types` type import, add `ctxTotals, ctxMeta` to the `./data` import list.
- State (with the other useState calls):

```tsx
  const [ctxRes, setCtxRes] = useState<CtxResourceCount[]>([]);
```

- In the per-range effect's `jobs` array, add:

```tsx
        fetchCtxResources(filters).then((v) => { if (!stale) setCtxRes(v); }),
```

- In the `view` memo's returned object, add:

```tsx
      ctx: ctxTotals(rpts, sel),
```

- In the right column, above the TokenBreakdown block:

```tsx
            <div className="tt-b8-col">
              <div>
                <ContextBreakdown tool={tool} ctx={view.ctx} meta={ctxMeta(ctxRes, sel)} />
              </div>
              <div>
                <TokenBreakdown tool={tool} cats={view.cats} />
              </div>
              <div>
                <ModelsList
```

- [ ] **Step 4: Verify**

Run: `npx tsc --noEmit && npm test`
Expected: both clean. (No new vitest cases here: the panel is a pure render of
Task 9's tested view-model; component snapshot tests aren't part of this
codebase's pattern.)

- [ ] **Step 5: Visual check**

Run the app (`npm run tauri dev`) long enough to confirm: Claude selected →
Context Breakdown above Token Breakdown, primary rows with bars/percentages,
meta line present; Hermes selected → header + all "—" rows + the
no-attribution note. Screenshot or eyeball; then quit.

- [ ] **Step 6: Commit**

```bash
git add src/overview/ContextBreakdown.tsx src/overview/Overview8b.tsx src/overview/mock.ts src/overview/FocusPanel.tsx
git commit -m "feat(overview): mount real Context Breakdown above Token Breakdown"
```

---

### Task 11: End-to-end verification on real logs

**Files:**
- Modify: `src-tauri/src/e2e_real_logs.rs`

- [ ] **Step 1: Add invariant checks**

Append to the `e2e_real_logs` test (before the final ccusage cross-check
section), after the existing summary prints:

```rust
    // Context attribution invariants (spec 2026-07-10-context-breakdown).
    // Partition exact where attributed:
    let bad_partition: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE ctx_messages IS NOT NULL AND \
             ctx_messages + COALESCE(ctx_system, 0) + COALESCE(ctx_reasoning, 0) != \
             input_tokens + cache_read_tokens + cache_write_5m_tokens + cache_write_1h_tokens",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bad_partition, 0, "primary partition must equal billed context exactly");

    // Secondary ⊆ messages:
    let bad_subset: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE \
             COALESCE(ctx_toolcalls, 0) > COALESCE(ctx_messages, 0) OR \
             COALESCE(ctx_mcp, 0) > COALESCE(ctx_messages, 0) OR \
             COALESCE(ctx_skills, 0) > COALESCE(ctx_messages, 0)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bad_subset, 0, "secondary categories are subsets of messages");

    // Hermes: no content, everything NULL:
    let hermes_ctx: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE source='hermes' AND ctx_messages IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(hermes_ctx, 0);

    // Claude attributed the bulk of its events (real transcripts on this machine):
    let (claude_total, claude_attr): (i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COUNT(ctx_messages) FROM events WHERE source='claude'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    println!("\n=== claude ctx coverage: {claude_attr}/{claude_total} events attributed ===");
    assert!(
        claude_attr * 10 >= claude_total * 5,
        "expected ≥50% of claude events attributed (got {claude_attr}/{claude_total})"
    );

    let resources: Vec<(String, String, i64)> = {
        let mut stmt = conn
            .prepare("SELECT source, kind, COUNT(DISTINCT name) FROM ctx_resources GROUP BY source, kind")
            .unwrap();
        let it = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
        it.collect::<rusqlite::Result<Vec<_>>>().unwrap()
    };
    println!("=== ctx resources ===");
    for (s, k, n) in &resources {
        println!("  {s:<8} {k:<12} {n}");
    }
```

- [ ] **Step 2: Run against real logs**

Run: `cargo test --release e2e_real_logs -- --ignored --nocapture`
Expected: PASS; the printed coverage/resource numbers should look plausible
(tens of skills, a few MCP servers). If the ≥50% coverage assertion fails,
investigate before weakening it — likely a line-shape mismatch, not a bad
threshold. (Old pre-retention sessions whose files still exist SHOULD
attribute; only tainted resumes and content-free sessions may not.)

- [ ] **Step 3: Full suites one last time**

Run: `cargo test` and `npm test` and `npx tsc --noEmit`
Expected: all PASS.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/e2e_real_logs.rs
git commit -m "test(e2e): context-attribution invariants on real logs"
```

---

## Post-plan notes

- **First launch after this ships re-scans everything** (v3 clears
  `scanned_files`) — same cost as a fresh install; mention in any release note.
- **Out of scope** (per spec): per-name drill-down rows, old dashboard/8a
  real data, live context inspection.
