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
