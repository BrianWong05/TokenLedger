// Context-attribution math (spec: 2026-07-10-context-breakdown). Pure
// running-composition counters in estimated tokens (bytes/4); the claude/codex
// adapters feed content in and ask attribute() for each API call's CtxTokens.
// NO rusqlite here — persistence (Claude's byte-offset resume) lives in db.rs.
use crate::types::CtxTokens;
use serde_json::Value;

/// tokens ≈ bytes / 4 (labeled "est." end to end).
pub fn est(bytes: usize) -> i64 {
    (bytes / 4) as i64
}

/// Bytes of a content value: strings verbatim, everything else JSON-serialized.
pub fn content_bytes(v: &Value) -> usize {
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn reset_compact_keeps_system_zeroes_rest() {
        let mut c = Composition { msg: 10, tool: 5, mcp: 2, skill: 1, reas: 4, sys: 100, initialized: true, tainted: false };
        c.reset_compact();
        assert_eq!(c, Composition { sys: 100, initialized: true, ..Default::default() });
    }
}
