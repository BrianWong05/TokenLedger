// Claude transcript line-parsers (spec: 2026-07-10-context-breakdown). Feed
// each transcript line into a running Composition. The pure attribution math
// lives in adapters::ctx and is re-exported so existing `claude_ctx::est`,
// `claude_ctx::content_bytes`, and `claude_ctx::Composition` call sites stay
// unchanged. DB persistence lives in db.rs.
pub use super::ctx::{content_bytes, est, Composition};
use serde_json::Value;
use std::collections::HashMap;

pub fn apply_user_line(
    comp: &mut Composition,
    v: &Value,
    tool_names: &HashMap<String, String>,
    tool_sizes: &mut Vec<(String, i64, i64)>,
) {
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
                let name = b["tool_use_id"]
                    .as_str()
                    .and_then(|id| tool_names.get(id))
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());
                tool_sizes.push((name.clone(), n, 0));
                if name.starts_with("mcp__") {
                    comp.mcp += n;
                } else if name == "Skill" {
                    comp.skill += n;
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
    tool_sizes: &mut Vec<(String, i64, i64)>,
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
                tool_sizes.push((name.to_string(), n, 1));
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn user_text_line_adds_messages_and_resets_reasoning() {
        let mut c = Composition { reas: 500, ..Default::default() };
        let line = json!({"type":"user","message":{"role":"user","content":"abcdefgh"}});
        apply_user_line(&mut c, &line, &HashMap::new(), &mut Vec::new());
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
        apply_user_line(&mut c, &line, &names, &mut Vec::new());
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
        apply_assistant_content(&mut c, &line, &mut names, &mut res, &mut Vec::new());
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
}
