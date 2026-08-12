// Cross-Source partition invariants, extracted from e2e_real_logs so they run
// on every plain `cargo test` against a hermetic sixteen-Source fixture — not
// only under the #[ignore] real-log e2e. The four assert_* helpers hold the
// exact SQL + messages the e2e used to inline; both callers share them.
//
// The whole module is #[cfg(test)]-gated at the lib.rs mod declaration, so the
// pub(crate) helpers exist only under test (the sole callers — e2e_real_logs
// and the hermetic test below — are themselves test-only).
use rusqlite::Connection;
use serde_json::json;
use std::path::Path;

/// Primary partition is exact where attributed: messages + system + reasoning
/// == billed context (input + cache_read + cache_write).
pub(crate) fn assert_partition_exact(conn: &Connection) {
    let bad_partition: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE ctx_messages IS NOT NULL AND \
             ctx_messages + COALESCE(ctx_system, 0) + COALESCE(ctx_reasoning, 0) != \
             input_tokens + cache_read_tokens + cache_write_5m_tokens + cache_write_1h_tokens",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        bad_partition, 0,
        "primary partition must equal billed context exactly"
    );
}

/// Secondary categories (toolcalls / mcp / skills) are subsets of messages.
pub(crate) fn assert_secondary_subset(conn: &Connection) {
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
    assert_eq!(
        bad_subset, 0,
        "secondary categories are subsets of messages"
    );
}

/// Hermes records no content: every ctx category stays NULL.
pub(crate) fn assert_hermes_ctx_null(conn: &Connection) {
    let hermes_ctx: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE source='hermes' AND ctx_messages IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(hermes_ctx, 0);
}

/// Exact-bucket partition per source: history + new_input + system + response
/// + reasoning == total usage for that source.
pub(crate) fn assert_bucket_partition_exact(conn: &Connection) {
    let buckets = crate::queries::ctx_buckets(conn, &crate::queries::Filters::default()).unwrap();
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
        let sum =
            b.history + b.new_input + b.system.unwrap_or(0) + b.response + b.reasoning.unwrap_or(0);
        assert_eq!(sum, total, "bucket partition exact for {}", b.source);
    }
}

// ---------------------------------------------------------------------------
// Hermetic sixteen-Source fixture + the default-run test that proves the four
// invariants on synthetic logs covering every Source's format. Fixtures are
// tiny, inline, and mined from each adapter's own #[cfg(test)] module.
// ---------------------------------------------------------------------------

use crate::db::open_db;
use crate::scan::{run_scan, SourceRoots};

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

// claude: attribution-rich session — user text, an assistant line carrying
// non-empty thinking (→ reasoning share) + a Bash tool_use (→ ctx_tools/ctx_exec)
// with cache-creation billed (→ cache writes), a matching tool_result, then a
// second billed call whose ctx lands nonzero system AND reasoning.
fn build_claude(base: &Path) {
    let user1 = r#"{"type":"user","sessionId":"s1","timestamp":"2026-07-01T10:00:00.000Z","message":{"role":"user","content":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}"#;
    // ~460 bytes of real thinking text (plain ASCII, JSON-safe) → nonzero reas est.
    let think = "Reasoning through the request carefully to reach a correct answer. ".repeat(7);
    let m1 = format!(
        r#"{{"type":"assistant","sessionId":"s1","requestId":"r1","timestamp":"2026-07-01T10:00:01.000Z","cwd":"/Users/dev/projects/alpha","message":{{"id":"m1","model":"z-ai/glm-5.2","content":[{{"type":"thinking","thinking":"{think}","signature":"sig"}},{{"type":"tool_use","id":"t1","name":"Bash","input":{{"command":"ls -la"}}}}],"usage":{{"input_tokens":100,"output_tokens":30,"cache_read_input_tokens":0,"cache_creation_input_tokens":900}}}}}}"#
    );
    let toolres = r#"{"type":"user","sessionId":"s1","timestamp":"2026-07-01T10:00:02.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"cccccccccccccccccccccccccccccccccccccccc"}]}}"#;
    let m2 = r#"{"type":"assistant","sessionId":"s1","requestId":"r2","timestamp":"2026-07-01T10:00:03.000Z","cwd":"/Users/dev/projects/alpha","message":{"id":"m2","model":"z-ai/glm-5.2","usage":{"input_tokens":500,"output_tokens":10,"cache_read_input_tokens":1500,"cache_creation_input_tokens":0}}}"#;
    write(
        &base.join("claude/proj1/s1.jsonl"),
        &format!("{user1}\n{m1}\n{toolres}\n{m2}\n"),
    );
}

// codex: session_meta + turn_context + response_items (message/reasoning/
// function_call/function_call_output) then TWO cumulative token_count lines
// with growing reasoning_output_tokens and cached_input_tokens.
fn build_codex(base: &Path) {
    let lines = [
        r#"{"type":"session_meta","timestamp":"2026-05-01T09:00:00.000Z","payload":{"id":"sess-cx","cwd":"/Users/dev/projects/alpha"}}"#,
        r#"{"type":"turn_context","timestamp":"2026-05-01T09:00:00.500Z","payload":{"model":"gpt-5.4"}}"#,
        r#"{"type":"response_item","timestamp":"2026-05-01T09:00:01.000Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}}"#,
        r#"{"type":"response_item","timestamp":"2026-05-01T09:00:01.500Z","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}]}}"#,
        r#"{"type":"response_item","timestamp":"2026-05-01T09:00:02.000Z","payload":{"type":"function_call","call_id":"c1","name":"shell","arguments":"{\"command\":[\"ls\"]}"}}"#,
        r#"{"type":"response_item","timestamp":"2026-05-01T09:00:02.500Z","payload":{"type":"function_call_output","call_id":"c1","output":"cccccccccccccccccccccccccccccccccccccccc"}}"#,
        r#"{"type":"event_msg","timestamp":"2026-05-01T09:00:03.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":900,"cached_input_tokens":100,"output_tokens":50,"reasoning_output_tokens":20,"total_tokens":950}}}}"#,
        r#"{"type":"event_msg","timestamp":"2026-05-01T09:00:04.000Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1500,"cached_input_tokens":300,"output_tokens":120,"reasoning_output_tokens":60,"total_tokens":1620}}}}"#,
    ];
    write(
        &base.join("codex/rollout-2026-05-01-ctx.jsonl"),
        &(lines.join("\n") + "\n"),
    );
}

// gemini: tmp_root/<hash>/chats/session-*.json plus projects.json. cached < input
// so the exclusive-input subtraction runs; a tokens.tool field feeds toolcalls.
fn build_gemini(base: &Path) {
    write(
        &base.join("gemini/projects.json"),
        r#"{"projects":{"/Users/dev/projects/alpha":"alpha"}}"#,
    );
    let session = r#"{
      "sessionId": "sess-gem",
      "messages": [
        { "id": "g1", "timestamp": "2026-05-01T10:00:00.000Z", "type": "gemini",
          "model": "gemini-2.5-flash",
          "tokens": { "input": 1000, "output": 200, "cached": 300, "thoughts": 50, "tool": 120, "total": 1250 } }
      ]
    }"#;
    write(&base.join("gemini/tmp/alpha/chats/session-1.json"), session);
}

// hermes: a SQLite DB in the schema the adapter reads; one session row spanning
// multiple api calls (api_call_count 30), with reasoning + cache_write + cwd.
// Minimal builder copied from the hermes adapter's own test module.
fn build_hermes(base: &Path) {
    let path = base.join("hermes/state.db");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let src = Connection::open(&path).unwrap();
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
    src.execute(
        "INSERT INTO sessions VALUES
         ('s1','qwen3.6-35b',1780287300.21103,64728,5088,1394761,100,50,30,'/Users/dev/projects/alpha')",
        [],
    )
    .unwrap();
}

// grok: sessions_root/<workspace>/<session>/updates.jsonl with a cumulative
// context counter growing across one turn (user_message_chunk → agent chunks).
fn build_grok(base: &Path) {
    let updates = [
        r#"{"timestamp":100,"method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"x"}}}}"#,
        r#"{"timestamp":101,"method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"x"}},"_meta":{"totalTokens":2500,"eventId":"e"}}}"#,
        r#"{"timestamp":102,"method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"x"}},"_meta":{"totalTokens":4000,"eventId":"e"}}}"#,
    ];
    write(
        &base.join("grok/%2FUsers%2Fdev%2Falpha/sess-1/updates.jsonl"),
        &(updates.join("\n") + "\n"),
    );
    write(
        &base.join("grok/%2FUsers%2Fdev%2Falpha/sess-1/summary.json"),
        r#"{"info":{"id":"sess-1","cwd":"/Users/dev/projects/alpha"},"current_model_id":"grok-4.5","updated_at":"2026-07-10T20:49:57Z"}"#,
    );
}

// antigravity: one conversation SQLite DB holding a single protobuf-encoded
// gen_metadata blob (system + fresh input + cache_read + output + thinking).
// The proto encoders + build_db are copied verbatim from the antigravity
// adapter's own test module (they are private test-only helpers there).
fn build_antigravity(base: &Path) {
    let dir = base.join("antigravity");
    std::fs::create_dir_all(&dir).unwrap();
    let gen = gen_blob(
        "gemini-3-flash-a",
        1_780_300_000,
        1132,
        500,
        20_000,
        300,
        150,
        "resp-1",
    );
    ag_build_db(&dir.join("conv-1.db"), &[gen]);
}

fn varint(mut v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
    out
}

fn f_varint(no: u64, v: u64) -> Vec<u8> {
    let mut out = varint(no << 3);
    out.extend(varint(v));
    out
}

fn f_len(no: u64, payload: &[u8]) -> Vec<u8> {
    let mut out = varint((no << 3) | 2);
    out.extend(varint(payload.len() as u64));
    out.extend_from_slice(payload);
    out
}

#[allow(clippy::too_many_arguments)]
fn gen_blob(
    model: &str,
    ts_secs: i64,
    sys: u64,
    input: u64,
    cache_read: u64,
    output: u64,
    thinking: u64,
    response_id: &str,
) -> Vec<u8> {
    let mut usage = Vec::new();
    usage.extend(f_varint(1, sys));
    usage.extend(f_varint(2, input));
    usage.extend(f_varint(5, cache_read));
    usage.extend(f_varint(9, output));
    usage.extend(f_varint(10, thinking));
    usage.extend(f_len(11, response_id.as_bytes()));

    let ts = f_varint(1, ts_secs as u64);
    let gen_info = f_len(4, &ts);

    let mut chat_model = Vec::new();
    chat_model.extend(f_len(4, &usage));
    chat_model.extend(f_len(9, &gen_info));
    chat_model.extend(f_len(19, model.as_bytes()));

    f_len(1, &chat_model)
}

fn build_pi(base: &Path) {
    let dir = base.join("pi/--Users-dev-projects-pi-demo--");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("session.jsonl"),
        include_str!("adapters/fixtures/pi/basic-session.jsonl"),
    )
    .unwrap();
}

fn build_goose(base: &Path) {
    let dir = base.join("goose");
    std::fs::create_dir_all(&dir).unwrap();
    let db = Connection::open(dir.join("sessions.db")).unwrap();
    db.execute_batch(
        "CREATE TABLE schema_version (version INTEGER NOT NULL);
         INSERT INTO schema_version VALUES (15);
         CREATE TABLE sessions (
             id TEXT PRIMARY KEY,
             working_dir TEXT,
             created_at TEXT,
             updated_at TEXT,
             input_tokens INTEGER,
             output_tokens INTEGER,
             cache_read_tokens INTEGER,
             cache_write_tokens INTEGER,
             accumulated_input_tokens INTEGER,
             accumulated_output_tokens INTEGER,
             accumulated_cache_read_tokens INTEGER,
             accumulated_cache_write_tokens INTEGER
         );
         CREATE TABLE usage_ledger (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             session_id TEXT NOT NULL,
             created_timestamp INTEGER NOT NULL,
             model TEXT,
             input_tokens INTEGER,
             output_tokens INTEGER,
             total_tokens INTEGER,
             cache_read_tokens INTEGER,
             cache_write_tokens INTEGER,
             cost REAL,
             cost_source TEXT,
             is_compaction INTEGER DEFAULT 0
         );
         INSERT INTO sessions
           (id, working_dir, created_at, updated_at)
           VALUES ('goose-s1', '/Users/dev/projects/goose',
                   '2026-07-01T10:00:00Z', '2026-07-01T10:00:00Z');
         INSERT INTO usage_ledger
           (session_id, created_timestamp, model, input_tokens, output_tokens,
            total_tokens, cache_read_tokens, cache_write_tokens)
           VALUES ('goose-s1', 1780308000, 'goose-model', 100, 20, 130, 10, 5);",
    )
    .unwrap();
}

fn build_opencode(base: &Path) {
    let path = base.join("opencode/opencode.db");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let db = Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TABLE session (
            id TEXT PRIMARY KEY,
            directory TEXT NOT NULL,
            time_created INTEGER NOT NULL,
            time_updated INTEGER NOT NULL
        );
        CREATE TABLE message (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            time_created INTEGER NOT NULL,
            data TEXT NOT NULL
        );
        INSERT INTO session VALUES
          ('opencode-s1', '/Users/dev/projects/opencode', 1780308000000, 1780308001000);",
    )
    .unwrap();
    db.execute(
        "INSERT INTO message VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            "opencode-m1",
            "opencode-s1",
            1780308000000i64,
            r#"{"role":"assistant","modelID":"opencode-model","tokens":{"input":40,"output":8,"reasoning":3,"cache":{"read":20,"write":2}}}"#,
        ],
    )
    .unwrap();
}

fn build_kilo(base: &Path) {
    let path = base.join("kilo/kilo.db");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let db = Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TABLE session (
            id TEXT PRIMARY KEY,
            project_id TEXT,
            workspace_id TEXT,
            parent_id TEXT,
            slug TEXT,
            directory TEXT,
            path TEXT,
            title TEXT,
            version TEXT,
            cost REAL,
            time_created INTEGER,
            time_updated INTEGER,
            model TEXT,
            tokens_input INTEGER,
            tokens_output INTEGER,
            tokens_reasoning INTEGER,
            tokens_cache_read INTEGER,
            tokens_cache_write INTEGER
        );
        CREATE TABLE message (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            time_created INTEGER NOT NULL,
            data TEXT NOT NULL
        );",
    )
    .unwrap();
    db.execute(
        "INSERT INTO session (
            id, directory, time_created, time_updated, model,
            tokens_input, tokens_output, tokens_reasoning,
            tokens_cache_read, tokens_cache_write
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            "kilo-s1",
            "/Users/dev/projects/kilo",
            1_780_308_000_000i64,
            1_780_308_001_000i64,
            r#"{"id":"kilo-model"}"#,
            40,
            8,
            2,
            20,
            2,
        ],
    )
    .unwrap();
}

fn build_zed(base: &Path) {
    let path = base.join("zed/threads/threads.db");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let db = Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TABLE threads (
            id TEXT PRIMARY KEY,
            summary TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            data_type TEXT NOT NULL,
            data BLOB NOT NULL,
            parent_id TEXT,
            folder_paths TEXT,
            folder_paths_order TEXT,
            created_at TEXT
        );",
    )
    .unwrap();
    let thread = json!({
        "version": "0.3.0",
        "messages": [{"content": "ZED_PRIVATE_PROMPT_MARKER"}],
        "cumulative_token_usage": {
            "input_tokens": 40,
            "output_tokens": 8,
            "cache_creation_input_tokens": 2,
            "cache_read_input_tokens": 20
        },
        "request_token_usage": {"request-1": {"input_tokens": 40}},
        "model": {"provider": "zed.dev", "model": "zed-model"}
    });
    let compressed = zstd::encode_all(thread.to_string().as_bytes(), 0).unwrap();
    db.execute(
        "INSERT INTO threads (
            id, summary, updated_at, data_type, data, folder_paths, folder_paths_order
         ) VALUES ('zed-s1', 'private summary', '2026-07-01T10:00:00Z',
                   'zstd', ?1, '/Users/dev/projects/zed', '0')",
        [compressed],
    )
    .unwrap();
}

fn build_cline(base: &Path) {
    write(
        &base.join("cline/editor/tasks/shared-session/ui_messages.json"),
        r#"[{"type":"say","say":"api_req_started","ts":1780308000000,"text":"{\"request\":\"CLINE_PRIVATE_PROMPT_MARKER\",\"tokensIn\":11,\"tokensOut\":5,\"cacheReads\":3,\"cacheWrites\":2}"}]"#,
    );
    write(
        &base.join("cline/editor/tasks/shared-session/api_conversation_history.json"),
        r#"[{"content":"<model>cline-model</model><cwd>/Users/dev/projects/cline</cwd>"}]"#,
    );
    write(
        &base.join("cline/cli/sessions/shared-session.json"),
        r#"{"id":"shared-session","cwd":"/Users/dev/projects/cline","model":"cline-model","messages":[{"type":"say","say":"api_req_started","ts":1780308000000,"text":"{\"tokensIn\":11,\"tokensOut\":5,\"cacheReads\":3,\"cacheWrites\":2}"},{"type":"say","say":"api_req_started","ts":1780308001000,"text":"{\"tokensIn\":7,\"tokensOut\":3,\"cacheReads\":1,\"cacheWrites\":0}"}]}"#,
    );
}

// workbuddy: a parent Session with a cache-heavy function_call (inputTokens
// includes the cache read, reported OpenAI-style), a message line carrying
// Anthropic-style usage, non-usage line types, plus a subagent transcript that
// joins the parent Session — proving the additive-no-double-count rule.
fn build_workbuddy(base: &Path) {
    let parent = base.join("workbuddy/Users-dev-projects-alpha/parent-sess.jsonl");
    write(
        &parent,
        concat!(
            // function_call with cache-inclusive inputTokens (OpenAI-style).
            r#"{"type":"function_call","id":"wb-fc-1","sessionId":"parent-sess","timestamp":1786091399000,"cwd":"/Users/dev/projects/alpha","providerData":{"model":"deepseek-v4-flash","usage":{"requests":1,"inputTokens":32221,"outputTokens":198,"totalTokens":32419},"rawUsage":{"prompt_tokens":32221,"completion_tokens":198,"prompt_cache_hit_tokens":32000,"prompt_cache_miss_tokens":221,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"prompt_cache_write_tokens":0,"completion_thinking_tokens":104,"credit":0.93}}}"#,
            "\n",
            // message with Anthropic-style usage.
            r#"{"type":"message","id":"wb-msg-1","sessionId":"parent-sess","timestamp":1786091400000,"cwd":"/Users/dev/projects/alpha","message":{"usage":{"input_tokens":37247,"output_tokens":454,"total_tokens":37701,"cache_read_input_tokens":36224}}}"#,
            "\n",
            // Non-usage line types: never Records.
            r#"{"type":"reasoning","id":"wb-rea-1","timestamp":1786091399500,"providerData":{"model":"deepseek-v4-flash"}}"#,
            "\n",
            r#"{"type":"function_call_result","id":"wb-fcr-1","timestamp":1786091399600,"output":{"type":"text","text":"ok"}}"#,
            "\n",
            r#"{"type":"ai-title","id":"wb-t-1","timestamp":1786091399700}"#,
            "\n",
            // Zero-token summary: not a Record.
            r#"{"type":"summary","id":"wb-sm-0","timestamp":1786091399800,"providerData":{"usage":{"requests":1,"inputTokens":0,"outputTokens":0,"totalTokens":0}}}"#,
            "\n",
        ),
    );
    // Subagent transcript: joins the parent Session (path-derived).
    write(
        &base.join("workbuddy/Users-dev-projects-alpha/parent-sess/subagents/agent-1.jsonl"),
        concat!(
            r#"{"type":"function_call","id":"wb-sub-1","sessionId":"sub-sess","timestamp":1786091410000,"cwd":"/Users/dev/projects/alpha","providerData":{"model":"deepseek-v4-flash","usage":{"requests":1,"inputTokens":1000,"outputTokens":50,"totalTokens":1050},"rawUsage":{"prompt_tokens":1000,"completion_tokens":50,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":1000}}}"#,
            "\n",
        ),
    );
}

fn ag_build_db(path: &Path, gens: &[Vec<u8>]) {
    let db = Connection::open(path).unwrap();
    db.execute_batch(
        "CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB, size INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE trajectory_metadata_blob (id TEXT DEFAULT \"main\", data BLOB, PRIMARY KEY (id));",
    )
    .unwrap();
    for (i, g) in gens.iter().enumerate() {
        db.execute(
            "INSERT INTO gen_metadata (idx, data) VALUES (?1, ?2)",
            rusqlite::params![i as i64, g],
        )
        .unwrap();
    }
}

// codebuddy: the same transcript shape as WorkBuddy, plus the `summary` line
// type first seen in CodeBuddy transcripts — a zero-token summary is not a
// Record, a non-zero one is. Shares the parser (ADR-0016).
fn build_codebuddy(base: &Path) {
    write(
        &base.join("codebuddy/Users-dev-projects-alpha/cb-sess.jsonl"),
        concat!(
            // Anthropic-style usage on a message line (cache read populated).
            r#"{"type":"message","id":"cb-msg-1","sessionId":"cb-sess","timestamp":1786092914695,"cwd":"/Users/dev/projects/alpha","message":{"usage":{"input_tokens":25190,"output_tokens":10,"total_tokens":25200,"cache_read_input_tokens":512}}}"#,
            "\n",
            // Non-zero summary: a Record (model hy3).
            r#"{"type":"summary","id":"cb-sm-1","timestamp":1786092915000,"cwd":"/Users/dev/projects/alpha","providerData":{"model":"hy3","usage":{"requests":1,"inputTokens":100,"outputTokens":5,"totalTokens":105}}}"#,
            "\n",
            // Zero-token summary: not a Record.
            r#"{"type":"summary","id":"cb-sm-0","timestamp":1786092916000,"providerData":{"usage":{"requests":1,"inputTokens":0,"outputTokens":0,"totalTokens":0}}}"#,
            "\n",
            // Non-usage line type: never a Record.
            r#"{"type":"file-history-snapshot","id":"cb-fh-1","timestamp":1786092917000,"snapshot":{}}"#,
            "\n",
        ),
    );
}

// qoder: a SQLite chat_message table where each usage-bearing assistant row is
// one Record. prompt_tokens includes cached_tokens, so Input = prompt − cached
// (ADR-0001). model_info carries the model_key. No cache-write, no reasoning,
// no Context tiers (catalog: context false).
fn build_qoder(base: &Path) {
    let path = base.join("qoder/local.db");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let db = Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TABLE chat_message (
            id VARCHAR(64) PRIMARY KEY,
            session_id VARCHAR(64),
            request_id VARCHAR(64),
            role VARCHAR(64),
            content text,
            summary text,
            summary_modified INTEGER,
            summary_trigger INTEGER DEFAULT 0,
            tool_result text,
            token_info text,
            model_info text,
            extra text DEFAULT '',
            gmt_create INTEGER
        );",
    )
    .unwrap();
    // prompt 25038 = 420 fresh + 24618 cached.
    db.execute(
        "INSERT INTO chat_message (id, session_id, role, content, token_info, model_info, gmt_create)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            "qoder-m1",
            "task-qoder.session.execution",
            "assistant",
            "QODER_PRIVATE_PROMPT_MARKER",
            r#"{"prompt_tokens":25038,"completion_tokens":470,"cached_tokens":24618,"max_input_tokens":1000000}"#,
            r#"{"model_key":"qmodel_38max"}"#,
            1_786_112_276_027i64,
        ],
    )
    .unwrap();
    drop(db);

    // The IDE also ships as the plain-Qoder edition with an identically shaped
    // database; both databases coexist and merge into the one Source.
    let edition_path = base.join("qoder-edition/local.db");
    std::fs::create_dir_all(edition_path.parent().unwrap()).unwrap();
    let edition_db = Connection::open(&edition_path).unwrap();
    edition_db
        .execute_batch(
            "CREATE TABLE chat_message (
                id VARCHAR(64) PRIMARY KEY,
                session_id VARCHAR(64),
                request_id VARCHAR(64),
                role VARCHAR(64),
                content text,
                summary text,
                summary_modified INTEGER,
                summary_trigger INTEGER DEFAULT 0,
                tool_result text,
                token_info text,
                model_info text,
                extra text DEFAULT '',
                gmt_create INTEGER
            );",
        )
        .unwrap();
    // prompt 300 = 100 fresh + 200 cached.
    edition_db
        .execute(
            "INSERT INTO chat_message (id, session_id, role, content, token_info, model_info, gmt_create)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                "qoder-edition-m1",
                "task-edition.session.execution",
                "assistant",
                "QODER_PRIVATE_PROMPT_MARKER",
                r#"{"prompt_tokens":300,"completion_tokens":20,"cached_tokens":200,"max_input_tokens":1000000}"#,
                r#"{"model_key":"qmodel_38max"}"#,
                1_786_112_276_028i64,
            ],
        )
        .unwrap();
    drop(edition_db);

    // CLI transcript family: one Claude-Code-shaped assistant line with the
    // ephemeral cache-write split; content carries a private marker.
    let cli_path = base.join("qoder-cli/projects/-Users-dev-projects-alpha/qcli-sess.jsonl");
    std::fs::create_dir_all(cli_path.parent().unwrap()).unwrap();
    std::fs::write(
        &cli_path,
        concat!(
            r#"{"type":"assistant","uuid":"qu-1","timestamp":"2026-08-07T16:53:21.465Z","#,
            r#""message":{"id":"chatcmpl-qcli-1","model":"qmodel_38max","role":"assistant","#,
            r#""content":[{"type":"text","text":"QODER_CLI_PRIVATE_PROMPT_MARKER"}],"#,
            r#""usage":{"input_tokens":120,"output_tokens":30,"cache_read_input_tokens":500,"#,
            r#""cache_creation_input_tokens":7,"cache_creation":{"ephemeral_5m_input_tokens":4,"#,
            r#""ephemeral_1h_input_tokens":3}}}},"#,
            r#""cwd":"/Users/dev/projects/alpha","sessionId":"qcli-sess"}"#,
            "\n",
        ),
    )
    .unwrap();
}
fn build_omp(base: &Path) {
    write(
        &base.join("omp/session-omp.jsonl"),
        concat!(
            r#"{"type":"session","version":3,"id":"session-omp","timestamp":"2026-07-01T12:00:00.000Z","cwd":"/Users/dev/projects/alpha"}"#,
            "\n",
            r#"{"type":"message","id":"u1","parentId":null,"timestamp":"2026-07-01T12:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":"hello"}],"timestamp":1782907201000}}"#,
            "\n",
            r#"{"type":"message","id":"a1","parentId":"u1","timestamp":"2026-07-01T12:00:02.000Z","message":{"role":"assistant","content":[{"type":"text","text":"hi"}],"provider":"anthropic","model":"claude-3-5-sonnet","usage":{"input":100,"output":50,"cacheRead":10,"cacheWrite":5},"stopReason":"stop","timestamp":1782907202000}}"#,
            "\n",
        ),
    );
}

#[test]
fn hermetic_sixteen_source_partition_invariants() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();

    build_claude(base);
    build_codex(base);
    build_gemini(base);
    build_hermes(base);
    build_grok(base);
    build_antigravity(base);
    build_goose(base);
    build_opencode(base);
    build_kilo(base);
    build_zed(base);
    build_cline(base);
    build_pi(base);
    build_workbuddy(base);
    build_codebuddy(base);
    build_qoder(base);
    build_omp(base);

    let roots = SourceRoots {
        claude: base.join("claude"),
        codex: base.join("codex"),
        gemini_tmp: base.join("gemini/tmp"),
        gemini_projects_json: base.join("gemini/projects.json"),
        hermes_db: base.join("hermes/state.db"),
        grok_sessions: base.join("grok"),
        grok_logs: base.join("grok-logs"),
        antigravity_conversations: base.join("antigravity"),
        antigravity_ide_conversations: base.join("antigravity-ide"),
        // No CLI fixture: a missing root is scanned quietly (zero events, no error).
        antigravity_cli_conversations: base.join("antigravity-cli"),
        goose_sessions: vec![base.join("goose")],
        pi_sessions: vec![base.join("pi")],
        omp_sessions: vec![base.join("omp")],
        opencode_data: base.join("opencode"),
        opencode_legacy: base.join("opencode/storage"),
        opencode_db: None,
        kilo_db: base.join("kilo/kilo.db"),
        zed_databases: vec![base.join("zed/threads/threads.db")],
        cline: vec![base.join("cline")],
        workbuddy: base.join("workbuddy"),
        codebuddy: base.join("codebuddy"),
        qoder_databases: vec![base.join("qoder/local.db"), base.join("qoder-edition/local.db")],
        qoder_cli_projects: vec![base.join("qoder-cli/projects")],
        limit_exports: base.join("limits"),
    };

    let mut conn = open_db(&base.join("ledger.db")).unwrap();
    let status = run_scan(&mut conn, &roots);

    // --- Non-vacuity guards: the invariants below must have real data to bite. ---

    // Every one of the sixteen Sources ingested events and reported no error.
    for src in [
        "claude",
        "codex",
        "gemini",
        "hermes",
        "grok",
        "antigravity",
        "goose",
        "opencode",
        "kilo",
        "zed",
        "cline",
        "pi",
        "omp",
        "workbuddy",
        "codebuddy",
        "qoder",
    ] {
        let s = status
            .sources
            .iter()
            .find(|s| s.source == src)
            .unwrap_or_else(|| panic!("missing source {src}"));
        assert!(
            s.events_inserted > 0,
            "{src}: expected events, got 0 (error={:?})",
            s.error
        );
        assert!(s.error.is_none(), "{src}: unexpected error {:?}", s.error);
    }

    // Claude attributed at least one event (partition invariant is not vacuous).
    let claude_attr: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE source='claude' AND ctx_messages IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(claude_attr > 0, "claude produced no attributed events");

    // A claude event lands nonzero system AND reasoning — the harder ctx paths
    // (system estimate + proxied-thinking reasoning share) actually fired.
    let claude_nz: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE source='claude' AND ctx_system > 0 AND ctx_reasoning > 0",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        claude_nz > 0,
        "expected a claude event with nonzero system AND reasoning"
    );

    // Claude drill-down tables populated (Bash tool_use + its result).
    let tools: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM ctx_tools WHERE source='claude'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(tools > 0, "claude ctx_tools empty");
    let exec: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM ctx_exec WHERE source='claude'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(exec > 0, "claude ctx_exec empty");

    // Every Source with billed tokens surfaces in ctx_buckets (all sixteen here).
    let buckets = crate::queries::ctx_buckets(&conn, &crate::queries::Filters::default()).unwrap();
    assert!(
        buckets.len() >= 15,
        "expected >=15 sources in ctx_buckets, got {}",
        buckets.len()
    );

    // pi is non-vacuous: it attributes context along its ancestor tree, populates
    // the tool drill-down, AND records Unattributed usage (its usage-bearing tool
    // result) with no Model — so the invariants below actually bite on pi too.
    let pi_attr: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE source='pi' AND ctx_messages IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(pi_attr > 0, "pi produced no attributed events");
    let pi_tools: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM ctx_tools WHERE source='pi'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(pi_tools > 0, "pi ctx_tools empty");
    let pi_unattributed: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE source='pi' AND model IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(pi_unattributed > 0, "pi recorded no Unattributed usage");

    // --- The universal invariants, now proven non-vacuous. ---
    assert_partition_exact(&conn);
    assert_secondary_subset(&conn);
    assert_hermes_ctx_null(&conn);
    assert_bucket_partition_exact(&conn);

    // A second scan of the same corpus inserts nothing new and leaves every
    // ingestion is idempotent across all sixteen.
    let totals_before = source_totals(&conn);
    let rescan = run_scan(&mut conn, &roots);
    for s in &rescan.sources {
        assert_eq!(
            s.events_inserted, 0,
            "{}: second scan re-inserted",
            s.source
        );
    }
    assert_eq!(
        totals_before,
        source_totals(&conn),
        "second-scan totals drifted"
    );
}

// (source, total tokens, requests) per Source — a stable-totals fingerprint.
fn source_totals(conn: &Connection) -> Vec<(String, i64, i64)> {
    let mut stmt = conn
        .prepare(
            "SELECT source, \
               SUM(input_tokens + output_tokens + cache_read_tokens + \
                   cache_write_5m_tokens + cache_write_1h_tokens), \
               SUM(api_calls) \
             FROM events GROUP BY source ORDER BY source",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap();
    rows.collect::<rusqlite::Result<Vec<_>>>().unwrap()
}
