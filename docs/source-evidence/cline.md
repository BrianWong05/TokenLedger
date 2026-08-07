# Cline Source evidence

Status as of 2026-08-07: the Cline adapter and synthetic validation are
implemented; the genuine private-Artifact gate remains pending because no Cline
task folder or CLI session snapshot was available in the validation workspace.
This file deliberately does not claim a private-artifact pass.

## Upstream corroboration

The supported shapes are the passive local task and session artifacts of
[Cline](https://github.com/cline/cline), the IDE and CLI coding agent:

- The editor writes request-level usage into `ui_messages.json` (legacy
  `claude_messages.json`) inside VS Code and editor-server
  `globalStorage/saoudrizwan.claude-dev/tasks` folders on each platform.
- The CLI writes JSON session snapshots under `~/.cline/data/sessions`
  (overridable with `CLINE_DATA_DIR`, then `CLINE_SANDBOX_DATA_DIR`). The CLI
  `sessions.db` index is not treated as usage data.
- Only normalized counters and explicit metadata are retained; prompts,
  responses, tool arguments, and other conversation content never cross into
  the Ledger.

## Private validation

Run the ignored validation only against a locally selected genuine artifact:

```sh
TOKENLEDGER_VALIDATION_SOURCE=cline \
TOKENLEDGER_VALIDATION_ARTIFACT=/private/path/to/tasks-folder \
cargo test --manifest-path src-tauri/Cargo.toml source_artifact_validation -- --ignored --nocapture
```

The report emits aggregate counts, a schema fingerprint, and pass/fail only.
It does not print the artifact path or content, and no real artifact belongs in
the repository.

## Synthetic coverage

The committed tests cover request-level usage parsing from `ui_messages.json`
and legacy `claude_messages.json`, CLI JSON session records, cross-surface
deduplication (an editor task also present in CLI storage is one Usage Record),
malformed and unsupported files, cache-write normalization without a TTL,
privacy-marker absence from the Ledger, platform roots, first-scan backfill,
unchanged rescans, disappearance, source isolation, and cross-Source partition
invariants.

## Fidelity limitations

Cline's cache-write counter has no TTL, so it is normalized to the 5-minute
Cache write category. Missing Model and relative Project values remain
unavailable rather than inferred. Task prompts, responses, tool arguments, and
other conversation content are never stored.
