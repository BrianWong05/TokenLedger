// Claude transcript line-parsers (spec: 2026-07-10-context-breakdown). Feed
// each transcript line into a running Composition. The pure attribution math
// lives in adapters::ctx and is re-exported so existing `claude_ctx::est`,
// `claude_ctx::content_bytes`, and `claude_ctx::Composition` call sites stay
// unchanged. DB persistence lives in db.rs.
pub use super::ctx::{content_bytes, est, Composition};
use super::find_segment;
use serde_json::Value;
use std::collections::HashMap;

// A skill's instructions are not returned by the Skill tool — its result is
// only "Launching skill: …". They arrive as an injected user message opening
// with this line, so the body is invisible to the tool-result path below.
const SKILL_BODY_PREFIX: &str = "Base directory for this skill:";

/// The skill a body injection belongs to, named exactly as it is invoked, so a
/// slash-command injection (which has no Skill tool call to name it) and a
/// tool invocation agree. Plugin skills live at
/// `…/plugins/cache/<marketplace>/<plugin>/<version>/skills/**/<skill>` and are
/// invoked `<plugin>:<skill>`; anything else (`~/.claude/skills/<skill>`) is
/// invoked bare. The trailing `**` matters: some plugins group skills into
/// category directories.
pub fn skill_body_name(text: &str) -> Option<String> {
    let first = text.strip_prefix(SKILL_BODY_PREFIX)?.lines().next()?;
    let path = first.trim().trim_end_matches('/');
    let skill = path.rsplit('/').next().filter(|s| !s.is_empty())?;
    match find_segment(path, "/plugins/cache/") {
        Some(i) => {
            // marketplace, then the plugin whose name namespaces the skill.
            let mut segs = path[i + "/plugins/cache/".len()..].split('/');
            let plugin = segs.nth(1).filter(|s| !s.is_empty())?;
            Some(format!("{plugin}:{skill}"))
        }
        None => Some(skill.to_string()),
    }
}

pub fn apply_user_line(
    comp: &mut Composition,
    v: &Value,
    tool_names: &HashMap<String, String>,
    tool_sizes: &mut Vec<(String, i64, i64)>,
    skill_sizes: &mut Vec<(String, i64, i64)>,
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
                let n = est(content_bytes(&b["text"]));
                comp.msg += n;
                comp.reas = 0;
                // Every injection is a fresh copy in the context, so repeats
                // sum rather than dedupe. Counted under skills as well as
                // messages — the secondary categories are shares of msg.
                if let Some(name) = b["text"].as_str().and_then(skill_body_name) {
                    comp.skill += n;
                    skill_sizes.push((name, n, 1));
                }
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
                        if find_segment(p, "/memory/").is_some() && p.ends_with("MEMORY.md") {
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

    // Body text long enough that est() is non-zero, prefixed with the path line.
    fn body(path: &str) -> String {
        format!("{SKILL_BODY_PREFIX} {path}\n\n# Skill\n{}", "x".repeat(400))
    }

    #[test]
    fn skill_body_name_reconstructs_the_invoked_name() {
        // Plugin skill: invoked `superpowers:brainstorming`.
        assert_eq!(
            skill_body_name(&body("/Users/b/.claude/plugins/cache/claude-plugins-official/superpowers/6.1.1/skills/brainstorming")),
            Some("superpowers:brainstorming".to_string()),
        );
        // Plugin that groups skills into category directories.
        assert_eq!(
            skill_body_name(&body("/Users/b/.claude/plugins/cache/claude-plugins-official/mattpocock-skills/1.2.0/skills/engineering/code-review")),
            Some("mattpocock-skills:code-review".to_string()),
        );
        // Local skill: invoked bare, so a same-named plugin skill stays distinct.
        assert_eq!(
            skill_body_name(&body("/Users/b/.claude/skills/grilling")),
            Some("grilling".to_string()),
        );
        assert_eq!(skill_body_name("just a normal user message"), None);
    }

    #[test]
    fn skill_body_counts_under_skills_and_messages() {
        let mut c = Composition::default();
        let mut skills: Vec<(String, i64, i64)> = Vec::new();
        let text = body("/Users/b/.claude/skills/grilling");
        let n = est(text.len());
        let line = json!({"type":"user","message":{"role":"user","content":[
            {"type":"text","text": text}
        ]}});
        apply_user_line(&mut c, &line, &HashMap::new(), &mut Vec::new(), &mut skills);

        assert_eq!(c.skill, n, "the loaded instructions are the skill's real cost");
        assert_eq!(c.msg, n, "still conversation content: skills stay a share of messages");
        assert_eq!(c.tool, 0, "an injected body is not a tool call");
        assert_eq!(skills, vec![("grilling".to_string(), n, 1)]);
    }

    #[test]
    fn repeat_injections_sum_rather_than_dedupe() {
        // Each invocation re-injects the whole body, so the context pays twice.
        let mut c = Composition::default();
        let mut skills: Vec<(String, i64, i64)> = Vec::new();
        let text = body("/Users/b/.claude/skills/implement");
        let line = json!({"type":"user","message":{"role":"user","content":[
            {"type":"text","text": text}
        ]}});
        apply_user_line(&mut c, &line, &HashMap::new(), &mut Vec::new(), &mut skills);
        apply_user_line(&mut c, &line, &HashMap::new(), &mut Vec::new(), &mut skills);

        assert_eq!(skills.len(), 2, "one row per injection; the query sums them");
        assert_eq!(c.skill, est(text.len()) * 2);
    }

    #[test]
    fn user_text_line_adds_messages_and_resets_reasoning() {
        let mut c = Composition { reas: 500, ..Default::default() };
        let line = json!({"type":"user","message":{"role":"user","content":"abcdefgh"}});
        apply_user_line(&mut c, &line, &HashMap::new(), &mut Vec::new(), &mut Vec::new());
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
        apply_user_line(&mut c, &line, &names, &mut Vec::new(), &mut Vec::new());
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

    // The file_path comes from the log, so it is spelt the way the machine that
    // wrote it spells paths — a Windows transcript says `\memory\`.
    #[test]
    fn a_memory_file_is_recognised_in_either_flavour_of_path() {
        let read = |p: &str| {
            let mut res: Vec<(&'static str, String)> = Vec::new();
            let line = json!({"type":"assistant","message":{"content":[
                {"type":"tool_use","id":"a","name":"Read","input":{"file_path": p}}
            ]}});
            apply_assistant_content(
                &mut Composition::default(),
                &line,
                &mut HashMap::new(),
                &mut res,
                &mut Vec::new(),
            );
            res.iter().any(|(k, _)| *k == "memory_file")
        };
        assert!(read("/Users/x/.claude/projects/-p/memory/MEMORY.md"));
        assert!(read(r"C:\Users\x\.claude\projects\-p\memory\MEMORY.md"));
        // A MEMORY.md that is not in a memory directory is still not one.
        assert!(!read("/Users/x/notes/MEMORY.md"));
        assert!(!read(r"C:\Users\x\notes\MEMORY.md"));
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
        apply_user_line(&mut c, &result, &names, &mut sizes2, &mut Vec::new());
        assert_eq!(sizes2, vec![("Bash".to_string(), 4, 0)], "result attributed via id map, calls 0");
    }

    #[test]
    fn unmatched_tool_result_reports_unknown() {
        let mut c = Composition::default();
        let mut sizes: Vec<(String, i64, i64)> = Vec::new();
        let line = json!({"type":"user","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"missing","content":"yyyyyyyy"}
        ]}});
        apply_user_line(&mut c, &line, &HashMap::new(), &mut sizes, &mut Vec::new());
        assert_eq!(sizes, vec![("unknown".to_string(), 2, 0)]);
    }
}
