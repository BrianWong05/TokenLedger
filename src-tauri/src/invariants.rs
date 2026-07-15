// Cross-Source partition invariants, extracted from e2e_real_logs so they run
// on every plain `cargo test` against a hermetic six-Source fixture — not only
// under the #[ignore] real-log e2e. The four assert_* helpers hold the exact
// SQL + messages the e2e used to inline; both callers share them.
//
// The whole module is #[cfg(test)]-gated at the lib.rs mod declaration, so the
// pub(crate) helpers exist only under test (the sole callers — e2e_real_logs
// and the hermetic test below — are themselves test-only).
use rusqlite::Connection;
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
    assert_eq!(bad_partition, 0, "primary partition must equal billed context exactly");
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
    assert_eq!(bad_subset, 0, "secondary categories are subsets of messages");
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
    let buckets =
        crate::queries::ctx_buckets(conn, &crate::queries::Filters::default()).unwrap();
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
    }
}

// ---------------------------------------------------------------------------
// Hermetic six-Source fixture + the default-run test that proves the four
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
    let gen = gen_blob("gemini-3-flash-a", 1_780_300_000, 1132, 500, 20_000, 300, 150, "resp-1");
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

#[test]
fn hermetic_six_source_partition_invariants() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();

    build_claude(base);
    build_codex(base);
    build_gemini(base);
    build_hermes(base);
    build_grok(base);
    build_antigravity(base);

    let roots = SourceRoots {
        claude: base.join("claude"),
        codex: base.join("codex"),
        gemini_tmp: base.join("gemini/tmp"),
        gemini_projects_json: base.join("gemini/projects.json"),
        hermes_db: base.join("hermes/state.db"),
        grok_sessions: base.join("grok"),
        antigravity_conversations: base.join("antigravity"),
        // No CLI fixture: a missing root is scanned quietly (zero events, no error).
        antigravity_cli_conversations: base.join("antigravity-cli"),
    };

    let mut conn = open_db(&base.join("ledger.db")).unwrap();
    let status = run_scan(&mut conn, &roots);

    // --- Non-vacuity guards: the invariants below must have real data to bite. ---

    // Every one of the six Sources ingested events and reported no error.
    for src in ["claude", "codex", "gemini", "hermes", "grok", "antigravity"] {
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
    assert!(claude_nz > 0, "expected a claude event with nonzero system AND reasoning");

    // Claude drill-down tables populated (Bash tool_use + its result).
    let tools: i64 = conn
        .query_row("SELECT COUNT(*) FROM ctx_tools WHERE source='claude'", [], |r| r.get(0))
        .unwrap();
    assert!(tools > 0, "claude ctx_tools empty");
    let exec: i64 = conn
        .query_row("SELECT COUNT(*) FROM ctx_exec WHERE source='claude'", [], |r| r.get(0))
        .unwrap();
    assert!(exec > 0, "claude ctx_exec empty");

    // Every Source with billed tokens surfaces in ctx_buckets (all six here).
    let buckets =
        crate::queries::ctx_buckets(&conn, &crate::queries::Filters::default()).unwrap();
    assert!(
        buckets.len() >= 6,
        "expected >=6 sources in ctx_buckets, got {}",
        buckets.len()
    );

    // --- The universal invariants, now proven non-vacuous. ---
    assert_partition_exact(&conn);
    assert_secondary_subset(&conn);
    assert_hermes_ctx_null(&conn);
    assert_bucket_partition_exact(&conn);
}
