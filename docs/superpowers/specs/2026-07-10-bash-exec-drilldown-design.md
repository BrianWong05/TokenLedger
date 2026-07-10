# Bash Exec Drill-Down — command-level facets (schema v5)

Date: 2026-07-10
Status: approved (brainstormed with user)
Reference: TokenTracker's exec ledger (`src/lib/categorizer-utils.js`:
`inferExecCommandKind`, `getExecutableName`, `sanitizeCommandSignature`).
Extends the unmerged `feature/context-drilldown` branch.

## Problem

The drill-down panel stops at the tool level: Execution → Bash shows one
number. TokenTracker drills one level further — per command kind,
executable, and command signature. This project adds that level for the
Bash leaf.

## Decisions (user-selected)

1. **Facets: By type / Executable / Command** — the three the logs
   support exactly. Rejected: Duration (not recorded in transcripts),
   Output (marginal), By-exit (TokenTracker itself hardcodes
   `unknown:unknown`; our optional `is_error` variant deferred).
2. **Approach A**: single raw-signature table; facets derived by GROUP
   BY at query/display time. Rejected: three parallel facet-row tables
   (3× writes, zero extra information); frontend-only classification
   (ctx_tools stores no command strings).
3. **Schema v5**, not a mutation of the unmerged v4 — the dev DB may
   already be at user_version 4 via tauri-dev auto-rebuilds. v5 costs
   the same one-time re-scan.
4. **Claude only in v1.** Codex `shell` commands are classifiable the
   same way later; out of scope now.

## Schema (v5)

- `ctx_exec(source TEXT, source_file TEXT, day TEXT, kind TEXT,
  exe TEXT, cmd TEXT, est_tokens INTEGER, calls INTEGER,
  PRIMARY KEY (source_file, day, kind, exe, cmd))` — one row per
  (file, local day, classified command), additive upsert.
- Migration v5: CREATE TABLE + clear `scanned_files` + `session_ctx`
  (one-time full re-scan populates history; self-healing per the
  full-reparse rule). `PRAGMA user_version = 5`.
- **Idempotency contract (same as ctx_tools):** any parse from byte 0
  clears the file's ctx_exec rows first; byte-offset resumes add
  increments over fresh bytes only.

## Classification (new pure module `src-tauri/src/adapters/exec_class.rs`)

Rust ports of TokenTracker's three helpers, applied to the full command
string at scan time:

- `exec_kind(cmd) -> &'static str` — regex table, evaluated in
  TokenTracker's order (build, test, typecheck, dependency, package,
  dev_server, syntax_check, node_eval, node_cli, git_status, git_remote,
  git_local, http, process, terminal, browser_control, file_mutation,
  shell_inspect, then `compound` when the command contains `;`/`&`/`|`,
  else `unknown`). Port the regexes verbatim (adapted to Rust regex or
  hand-rolled `starts_with`/contains checks — no new dependencies, so
  prefer hand-rolled matching; the plan pins each rule with a test).
- `exec_words(cmd) -> Vec<String>` — shell-ish word splitting honoring
  double/single quotes; `unwrap_shell(words)` strips `bash|sh|zsh|fish
  -lc` wrappers and `env|command|xcrun` prefixes (recursive).
- `exec_exe(cmd) -> String` — basename of the first unwrapped word,
  else "unknown".
- `exec_cmd(cmd) -> String` — signature: `exe` + first non-flag,
  non-VAR=value argument (e.g. "git add", "npx vitest"), else `exe`.

## Scan (Claude adapter)

- On a `Bash` `tool_use`: classify `input.command` (string; missing or
  non-string → skip exec accumulation, the generic ctx_tools weight
  still books), accumulate `(kind, exe, cmd, est(input bytes), 1, ts)`.
- On its paired `tool_result` (via the existing `tool_use_id` map):
  add the result's est to the SAME classified command, calls +0. A
  result whose tool_use is unknown (resume boundary) books nothing to
  ctx_exec (its weight still lands in ctx_tools under "unknown" — the
  known resume imprecision).
- Clear-per-file on `start == 0` alongside the ctx_tools clear; write
  once after the events insert (`add_ctx_exec_rows`, mirroring
  `add_ctx_tool_rows`).

## Query & IPC

- `ctx_exec(filters)` → `[{ source, kind, exe, cmd, est_tokens, calls }]`
  summed over in-range days (same tool + day-bounds convention as
  ctx_tools; ignores model/project). serde camelCase → `estTokens`.
- `fetchCtxExec(filters)` in api.ts; `CtxExecRow` in types.ts.

## Frontend

- data.ts: `execFacets(rows, bashTotal)` — groups rows three ways
  (kind / exe / cmd), allocates `bashTotal` across each facet's keys by
  est weights (`allocateByWeight` — each facet's rows sum exactly to
  the Bash total), sorts desc, returns
  `{ byType, byExecutable, byCommand }` of `{ key, tokens, calls }[]`.
- ContextBreakdown: the Bash leaf row becomes expandable when exec rows
  exist. Expanded: a facet-tab strip (`By type · Executable · Command`,
  local state, default By type) + a compact table (TYPE | CALLS |
  TOTAL), top 20 rows + "+N more" summary line when truncated. Reuses
  existing row/table styling; at most two small CSS additions (tab
  strip, table grid).
- Overview8b: one more fetch in the per-range effect (same stale
  guard); pass `execRows={ctxExecRows.filter(r => r.source === sel)}`.
- mock.ts adapter gains a matching `execRows: []` so 8a compiles.

## Edge cases

- Non-Bash sources: no rows → Bash leaf not expandable (Codex/Gemini/
  Hermes unaffected).
- A command classified differently across occurrences (e.g. `git add`
  alone vs inside a compound) produces distinct rows; GROUP BY cmd in
  the Command facet merges them, kind facet keeps them separate —
  correct by construction.
- Empty command string → kind "unknown", exe/cmd "unknown".
- Facet tables render only allocated tokens (weights never shown raw),
  consistent with the tool tree.

## Testing

- exec_class unit tests: every kind rule pinned with a concrete command
  (incl. "npm run build"→build, "npx vitest"→unknown — the ported table
  has no npx rule, it ranks via the Executable/Command facets instead,
  "git status"→git_status, "git push"→git_remote,
  "rm x && git add ."→git_local — the git/http rules match anywhere in
  the command, before file_mutation/compound, "cd a && npx tsc"→compound,
  `bash -lc "git add ."` unwrap,
  quoted-arg word splitting, VAR=value skip in signatures).
- Scan test: Bash tool_use + result accumulate one classified row;
  unchanged re-scan stable; forced full re-parse replaces (the
  ctx_tools idempotency triple, applied to ctx_exec).
- Query test: day-range + tool scoping.
- data.test.ts: facet grouping, allocation exactness per facet, sort,
  truncation.
- e2e: assert claude ctx_exec rows exist on real logs; print top 8 by
  kind and by executable.

## Out of scope

- Codex `shell` classification (same machinery, later).
- Duration / Output / By-exit facets.
- Per-command-row drill-into-output.
