# Behavior-Preserving Source Catalog Design

## Goal

Introduce one declarative Source Catalog for the seven existing Sources so
Rust discovery and TypeScript Overview presentation share permanent identity,
display metadata, aliases, Artifact roots, Source Capabilities, platform
conditions, and external prerequisites without changing scan or Ledger
behavior.

## Scope

This change is the catalog foundation only. It keeps the current seven Sources
and their current roots. Hermes profile discovery, Gemini home overrides, and
Grok visible-home discovery remain follow-up work described in
`docs/source-expansion.md`.

The catalog contains facts only. It does not contain parser functions, skip
strategies, persistence policies, or a generic adapter configuration blob.
Existing Source-specific scanner functions, token normalization, persistence,
Cost calculation, and query contracts remain explicit and unchanged.

## Approaches considered

1. **Shared JSON imported by both runtimes (selected).** A repository-local JSON
   document is imported by TypeScript and compiled into Rust with `include_str!`.
   This gives both consumers the same declarative source without adding a
   runtime command or a generation step.
2. **Rust-owned catalog exposed through Tauri.** This guarantees one runtime
   payload, but makes synchronous Overview metadata depend on asynchronous app
   state and adds a second testing port for a static fact set.
3. **Generated Rust and TypeScript files from YAML.** This can offer richer
   authoring syntax, but adds a generator, build ordering, and generated-file
   drift to a change that needs no transformation.

The shared JSON approach has the smallest change surface and preserves the
existing frontend's synchronous pure reshaping functions.

## Catalog shape

The shared document contains an ordered `sources` array. Each entry has:

- a permanent lowercase `key`;
- mutable display `label` and full `source` name;
- `aliases` for accepted names and future display migration;
- `color` and an `icon` identity;
- declarative `artifacts`, each with an identity, root descriptor, platform
  condition, and external prerequisite;
- static `capabilities`, where an unavailable capability is distinct from a
  measured zero.

The seven entries retain the existing keys and display values:
`claude`, `codex`, `gemini`, `hermes`, `grok`, `antigravity`, and `pi`.

Rust deserializes the document once through a small catalog module. The
existing `SourceRoots` constructor resolves the catalog's current root
descriptors against the home directory and existing visible pi environment
overrides. `run_scan` still dispatches each unlike scanner explicitly; the
catalog supplies source order, identity, and discovery facts rather than
behavior.

TypeScript imports the same JSON through a source-catalog module. `TOOLS`,
source metadata, icon identity, and the empty per-Source maps derive from the
catalog. The public reshaping types use string Source keys rather than a
seven-member union. Known catalog entries retain their metadata; a Ledger key
not yet present in the catalog is retained with a neutral fallback label and
color instead of being silently discarded.

## Data flow

```text
source-catalog.json
        ├── Rust catalog ──> SourceRoots ──> explicit scanner calls ──> Ledger
        └── TypeScript catalog ──> Overview metadata/maps/ordering
                                      ↑
                         Series and Breakdown query rows
```

Backend scan statuses continue to report one status per existing Source, and
the scan-to-Ledger seam continues to prove the current seven-Source corpus,
Source isolation, durable history, token partitioning, and idempotent rescans.

Frontend data reshapers include every Source key found in query rows. Catalog
metadata determines labels, colors, icons, ordering, and aliases; unknown keys
remain visible through the fallback metadata path. Existing nullable backend
capability values continue to render as `—`; an observed numeric zero remains
`0`.

## Error handling and invariants

- A missing Artifact root remains a quiet empty scan.
- An existing malformed Source Artifact remains isolated through the current
  `run_one` boundary.
- Source keys are unique, lowercase, and stable; display metadata may change
  without rewriting Ledger history.
- No database schema change is needed.
- No Source content, prompts, responses, or complete real Source Artifacts are
  added to the repository.
- The catalog does not authorize network access, authentication, cookies,
  API-key handling, or synchronization.
- The explicit-adapter decision in ADR-0004 remains in force.

## Testing

The backend acceptance seam will derive its expected Source set from the
catalog while retaining the existing synthetic seven-Source corpus and
assertions for ingestion, failure isolation, token invariants, privacy,
idempotency, and durable history. Catalog parsing and root resolution will
also be covered by public construction behavior.

The frontend catalog-to-Overview seam will prove that catalog metadata reaches
source labels, colors, icons, ordering, and filters; that absent Sources do
not appear; that historical Sources remain visible; that an unlisted Ledger
key is not dropped; and that nullable capability values remain `—` while
measured zero remains `0`.

## Non-goals

- Adding new Source adapters.
- Expanding Hermes, Gemini, or Grok discovery roots.
- Introducing a Source-adapter trait or plugin runtime.
- Replacing Source-specific parsing, skip, or persistence behavior.
- Adding arbitrary user-configurable Artifact paths.
