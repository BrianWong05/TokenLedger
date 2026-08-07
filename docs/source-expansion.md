# Source expansion

This record captures the agreed rollout for expanding TokenLedger beyond its
original seven Sources. It is a backlog and acceptance contract, not a claim
that every named Source is already supported.

## Boundaries

- A Source is one independently operated AI coding tool. Alternate interfaces,
  versions, storage formats, and Model backends do not create Sources.
- A Source may have multiple Source Artifact paths. Each Artifact format is
  evidence-gated independently.
- Oh My Pi is a distinct Source that may share pi's parser. Antigravity IDE and
  CLI remain one Antigravity Source. Kimi CLI and Kimi Code remain one Kimi
  Source.
- TokenLedger reads only already-present local Artifacts. It never performs
  account login, cookie or API-key handling, or private remote synchronisation.
- A candidate must expose a timestamp and non-zero token usage. Missing Model,
  Project, Session, or Context attribution does not block truthful tokens; it
  remains unavailable rather than being inferred.
- Warp, Crush, and Sakana-specific handling are not in the implementation
  backlog. Codex usage is not filtered merely because its logged backend is
  Sakana.

## Acceptance gate

A Source is supported only after it has all of the following:

1. A genuine private Source Artifact corroborated by an upstream schema or
   writer, or by several independent samples.
2. A minimal committed fixture containing synthetic content and expected Usage
   Records. Complete user-produced Artifacts never enter the repository.
3. Automatic discovery, version-aware parsing, Source-native deduplication,
   idempotent rescans, disappearance handling, and privacy checks.
4. Cross-Source invariant coverage, frontend identity and filtering, and
   documented fidelity limitations.

Validation may be performed through private maintainer inspection or by a
trusted contributor running an ignored real-artifact parity test locally and
returning only normalised counts, a schema/version fingerprint, and pass/fail
output.

## Foundation and existing Sources

The first change is a behaviour-preserving Source Catalog shared by Rust and
TypeScript. It owns stable lowercase identity, mutable display metadata,
aliases, Artifact roots, Capabilities, and external prerequisites; scan and
persistence behaviour remains explicit per Source.

After the catalog, add these proven roots to existing Sources:

- Hermes: `$HERMES_HOME/state.db` and profile databases, falling back to
  `~/.hermes`.
- Gemini: `$GEMINI_CLI_HOME/.gemini/tmp`, falling back to `~/.gemini/tmp`.
- Grok: `$GROK_HOME/sessions`, falling back to `~/.grok/sessions`, once the
  Artifact fixture confirms the supplied convention.

`~/.claude/transcripts` remains evidence-gated because other programs can write
Claude-shaped records there; it must not be attributed to Claude until origin
and collision rules are proven. `~/.omp/agent/sessions` belongs to the distinct
Oh My Pi Source rather than becoming another pi root.

## Preferred first tranche

1. Goose
2. OpenCode
3. Cline
4. Kilo
5. Zed

This ordering is a preference, not a reason to wait: whichever candidate first
crosses the private-artifact gate may be implemented first. Each new Source
lands independently. OpenCode, Kilo, and Zed use Session-level Usage Records
at the Session's updated timestamp when their supported Artifact exposes no
trustworthy finer timing.

## Ready after a private Artifact

- Qwen
- Reasonix
- Senpi
- Gajae-Code
- Jcode
- MiMo Code
- OpenCodeReview
- OpenClaw
- GitHub Copilot CLI
- Kimchi
- Oh My Pi
- WorkBuddy — genuine private Artifact verified 2026-08-07
  (`~/.workbuddy/projects/**/*.jsonl`); parser design agreed in ADR-0016
- CodeBuddy — genuine private Artifact verified 2026-08-07
  (`~/.codebuddy/projects/**/*.jsonl`); shares WorkBuddy's parser per ADR-0016

## Needs semantic design and a private Artifact

- Mux — read active and archived event history; never turn cumulative
  `session-usage.json` totals into dated Usage Records.
- Kimi — Kimi Code is stronger than the legacy format; unreliable historical
  Model correlation becomes Unattributed Usage.
- Roo Code — never assign the last Session Model to every Request.
- Cursor — prove collision-safe identity for cache rows.
- Trae — cache-backed and dependent on an externally performed sync.
- Antigravity IDE cache — part of the existing Antigravity Source and dependent
  on an externally performed sync.

## Evidence-blocked

- Amp
- Factory Droid
- Junie
- Kiro
- ZCode

These candidates lack enough evidence to design a truthful production parser.
They remain pending rather than rejected.

## Exclusions

- Warp — aggregate requests and billed spend, but no non-zero token usage.
- Crush — the supplied project registry does not prove Usage Records.
- Sakana Fugu — no dedicated Source or special parser.
- Codebuff's current format — credits and context state are not consumed token
  usage.
- Kilo's legacy VS Code task format — the official migration produces zero
  tokens. This does not exclude Kilo's current CLI Artifact.
- Mux `session-usage.json` as an event source — it is a mutable cumulative
  snapshot. This does not exclude Mux event history.
- The earlier supplied WorkBuddy project path — it was not proven to represent
  the desktop Source's primary history at the time. Superseded 2026-08-07 by a
  verified live Artifact at `~/.workbuddy/projects/**/*.jsonl`; WorkBuddy is
  no longer excluded (ADR-0016).

## Scan and presentation contract

- First discovery backfills every available Usage Record; later scans use the
  safe incremental strategy for that Artifact.
- Missing paths are silently empty. Existing malformed or unsupported
  Artifacts produce a Source-specific warning, preserve the Ledger, and cannot
  stop other Sources from scanning.
- Modern and legacy Artifacts may both be scanned; deduplication uses stable
  Source-native identity and the modern representation wins conflicts.
- Overview filters show Sources represented in the Ledger, including historical
  Sources whose Artifacts have disappeared.
- Unsupported or unavailable Capabilities display as `—`; `0` is reserved for
  a measured zero.
