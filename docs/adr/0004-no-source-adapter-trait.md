# No Source-adapter trait: scan wiring stays explicit

TokenLedger's seven Source adapters are wired into the scan as seven explicit
function calls (`scan.rs::run_scan`), not through a shared trait — a shape that
looks like it is begging for one. Architecture reviews evaluated a `Source`
trait twice (July 2026) and rejected it both times for the same reason: the
apparent uniformity is one line deep. The adapters deliberately differ on every
axis a trait would have to abstract — inputs (a single directory for most,
two paths for Gemini, a path slice for Antigravity, a live SQLite handle for
Hermes), skip strategy (Claude resumes by byte offset, Codex/Gemini/Grok/
Antigravity skip unchanged files, Hermes rescans every time), and write
strategy (keep-max upsert for Claude's growing snapshots, dedup-insert for
Codex, whole-session upsert for Hermes, replace-by-file for the rest — each
encoding that Source's idempotency semantics). A trait would need a config
blob and per-Source knobs to cover all three axes, relocating cheap wiring
without concentrating any complexity: it fails the deletion test.

What the evaluations *did* yield is recorded in code, not a trait: Claude and
Codex parse through pure `parse_file` cores like Grok and Antigravity, and all
whole-file adapters share `mod.rs`'s `unchanged()` skip-check. The explicit
call list in `run_scan` is the cost we accept: adding a Source means editing
`scan.rs` by hand.

## Consequences

Revisit only if a concrete trigger fires: the post-split persistence shells
become genuinely identical across adapters, or adding a Source starts
requiring edits beyond `scan.rs` and the new adapter file. Absent that, a
future review proposing "introduce a Source trait" is re-litigating this
decision, not finding new friction.
