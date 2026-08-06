# Behavior-Preserving Source Catalog Implementation Plan

> For agentic workers: REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox syntax and must be updated as work completes.

**Goal:** Introduce one shared declarative Source Catalog consumed by the Rust scanner and TypeScript Overview while preserving all existing scanner, persistence, query, pricing, and rendering behavior for the seven supported Sources.

**Architecture:** Store catalog facts in src/source-catalog.json. Rust loads that document with include_str! and serde_json behind a OnceLock. TypeScript imports the same JSON through src/overview/meta.ts. The Rust scanner remains an explicit dispatch over Source-specific functions, and the catalog does not become an adapter trait, plugin runtime, or generic scanner configuration.

**Testing strategy:** Follow the public behavior seams from the TDD skill. First prove backend scan-to-Ledger behavior and idempotency with the catalog-driven discovery path. Then prove the frontend catalog-to-Overview reshaping path, including unknown Ledger Source keys and the null-versus-zero capability distinction. Run focused tests after each slice, then the complete Rust and TypeScript suites plus typechecking, formatting, and production build.

## Task 1: Add the shared catalog and drive Rust discovery from it

- [ ] Inspect the existing scanner, invariant tests, and source metadata contracts immediately before editing. Confirm current source order, root paths, explicit scanner functions, Pi environment overrides, and the seven-source scan-to-Ledger acceptance seam.
- [ ] Add src/source-catalog.json with one entry for each existing permanent lowercase key, in current scan order:
  - claude: Claude / Claude Code / #d97757 / .claude/projects
  - codex: Codex / Codex / #6e50f2 / .codex/sessions
  - gemini: Gemini / Gemini CLI / #3186ff / .gemini/tmp and .gemini/projects.json
  - hermes: Hermes / Hermes / #f472b6 / .hermes/state.db
  - grok: Grok / Grok Build / #c3c8d2 / .grok/sessions
  - antigravity: Antigravity / Google Antigravity / #22d3ee / .gemini/antigravity/conversations and .gemini/antigravity-cli/conversations
  - pi: pi / pi / #a3a3a3 / .pi/agent/sessions, plus the existing PI_CODING_AGENT_SESSION_DIR and PI_CODING_AGENT_DIR override descriptors
- [ ] Give every entry explicit fields for key, label, source, aliases, color, icon identity, artifact descriptors, capabilities, platforms, and external prerequisite. Keep all current model, project, session, and token-category capability facts. Represent context capability only for Claude, Codex, and pi, matching current behavior. Use null for currently unspecified prerequisites and all supported platforms for current roots. Keep the catalog factual; do not encode scanner/parser behavior in it.
- [ ] Add a Rust source_catalog module that deserializes the shared JSON once, exposes the catalog and lookup helpers, and gives artifact descriptors typed enough for root construction. Keep source identity and metadata owned by the catalog, while retaining typed SourceRoots fields and explicit environment-variable semantics.
- [ ] Refactor SourceRoots construction to consume catalog artifact paths and append the existing visible Pi session and agent overrides without changing path filtering or root ordering. Do not add a shared Source adapter trait.
- [ ] Refactor run_scan discovery to iterate catalog order but retain an explicit match from each known key to its existing Source-specific scanner function. Preserve per-source error isolation, quiet missing roots, status ordering, SourceStatus shape, persistence, and idempotency.
- [ ] Add or update backend tests proving the catalog contains exactly the seven expected keys and roots, including the Claude Code source name and Pi root descriptors. Preserve and strengthen scan-to-Ledger behavior parity tests: missing roots remain empty, malformed one-source input remains isolated, the other sources continue scanning, and a repeated scan remains idempotent.
- [ ] Run focused checks:
  ~~~
  cargo test --manifest-path src-tauri/Cargo.toml catalog_describes_the_existing_sources_and_artifact_roots
  cargo test --manifest-path src-tauri/Cargo.toml pi_roots_include_standard_and_visible_session_and_agent_overrides
  cargo test --manifest-path src-tauri/Cargo.toml run_scan_isolates_sources
  cargo test --manifest-path src-tauri/Cargo.toml hermetic_seven_source_partition_invariants
  cargo check --manifest-path src-tauri/Cargo.toml
  cargo fmt --all -- --check
  ~~~
- [ ] Review the diff for behavior changes and commit only the backend/catalog slice:
  ~~~
  git add src/source-catalog.json src-tauri/src/source_catalog.rs src-tauri/src/lib.rs src-tauri/src/scan.rs src-tauri/src/invariants.rs
  git diff --cached --check
  git commit -m "feat: drive source discovery from shared catalog"
  ~~~

## Task 2: Drive frontend Overview metadata and reshaping from the catalog

- [ ] Inspect the existing Overview metadata, icons, data reshaping, store selectors, tray panel model, cost/pricing helpers, and their tests immediately before editing. Preserve existing selected-source defaults, known-source ordering, visual labels, colors, and icon assets.
- [ ] Replace the closed ToolKey union and hand-maintained authoritative metadata list with catalog-derived metadata. Keep ToolKey as a string-compatible type, expose catalog order, and include aliases, icon identity, and capabilities in ToolMeta.
- [ ] Implement sourceMeta(key: string) so known catalog keys return catalog metadata and an unknown Ledger key returns neutral metadata using that key as its label/source, a stable fallback color, a generic icon identity, empty aliases, and no capabilities. This keeps future or historical Ledger keys visible instead of silently dropping them.
- [ ] Make emptyByTool and all reshaping maps dynamic records. Preserve catalog order for known keys and append unknown keys in first-seen order. Remove source-key guards that discard points when p.source is not in the old union. Ensure buckets, totals, series, small multiples, model-owner labels, visible-source selection, selected-source metadata, cost breakdowns, pricing derivation, and tray panel rendering all use the catalog/fallback path.
- [ ] Keep the icon registry keyed by catalog icon identity and provide a generic fallback icon. Do not create a second authoritative list of source display names in a component.
- [ ] Add frontend tests before implementation for the public catalog-to-Overview seam:
  - An unknown future-source point remains in bucket byTool, bucket total, and smallMultiples with its key and fallback label.
  - A context capability with null remains unavailable and renders as an em dash, while a numeric zero remains numeric zero.
  - Catalog metadata and generic fallback metadata flow through the tray panel model.
- [ ] Run focused checks:
  ~~~
  npx vitest run src/overview/meta.test.ts src/overview/data.test.ts src/traypanel/panelModel.test.ts
  npm run build
  ~~~
- [ ] Review the diff for dropped keys, changed display metadata, and null/zero regressions, then commit only the frontend slice:
  ~~~
  git add src/overview/meta.ts src/overview/icons.ts src/overview/data.ts src/overview/overviewStore.ts src/overview/Overview.tsx src/overview/AggTrend.tsx src/overview/HeatmapModal.tsx src/overview/costBreakdown.ts src/pricing/pricing.derive.ts src/traypanel/OverrideEditor.tsx src/traypanel/panelModel.ts src/overview/meta.test.ts src/overview/data.test.ts src/traypanel/panelModel.test.ts
  git diff --cached --check
  git commit -m "feat: drive overview source metadata from catalog"
  ~~~

## Task 3: Run whole-branch verification and review against issue #66

- [ ] Confirm the working tree contains only the intended implementation commits and the pre-existing user-owned edits. Do not stage or modify CONTEXT.md, the existing ADR edits, the untracked source-expansion document, or the untracked ADRs.
- [ ] Run fresh verification from the implementation tip:
  ~~~
  cargo test --manifest-path src-tauri/Cargo.toml
  npm test
  cargo fmt --all -- --check
  cargo check --manifest-path src-tauri/Cargo.toml
  npm run build
  git diff --check 635bdd36757a02334aebd95b43ff84e30cf22afb..HEAD
  ~~~
- [ ] Run the code-review skill against base commit 635bdd36757a02334aebd95b43ff84e30cf22afb, using issue #66, docs/agents/domain.md, ADR 0004, ADRs 0012-0015, and the implementation diff. Review Standards and Spec separately. Address any actionable findings with a follow-up fixer task and rerun the affected tests.
- [ ] Recheck the issue acceptance criteria: one shared catalog, seven permanent keys, backend and frontend consumers, unknown Source preservation, null versus zero capability display, explicit adapters, quiet missing roots, source isolation, durable records, unchanged contracts, and documentation consistency.
- [ ] Report the implementation commits, verification commands, review result, and preserved pre-existing edits. Leave the current branch as-is; do not push or open a pull request unless separately requested.
