# Zed Source evidence

Status as of 2026-08-07: the Zed adapter and synthetic validation are
implemented; the genuine private-Artifact gate remains pending because no Zed
`threads.db` was available in the validation workspace. This file deliberately
does not claim a private-artifact pass.

## Upstream corroboration

The supported shape is based on Zed's upstream writer and provider identity:

- [`crates/paths/src/paths.rs`](https://github.com/zed-industries/zed/blob/main/crates/paths/src/paths.rs)
  places native data under `Library/Application Support/Zed` on macOS,
  `.local/share/zed` on Linux, and `AppData/Local/Zed` on Windows, with the
  XDG and Flatpak data-home branches used by the adapter.
- [`crates/agent/src/db.rs`](https://github.com/zed-industries/zed/blob/main/crates/agent/src/db.rs)
  writes `threads/threads.db`, a `threads` table with compressed thread data,
  `updated_at`, cumulative token usage, request usage, model metadata, and
  serialized folder paths.
- [`crates/language_model_core/src/provider.rs`](https://github.com/zed-industries/zed/blob/main/crates/language_model_core/src/provider.rs)
  defines the hosted provider ID as `zed.dev`.
- [`crates/acp_thread/src/acp_thread.rs`](https://github.com/zed-industries/zed/blob/main/crates/acp_thread/src/acp_thread.rs)
  keeps ACP session usage in a separate representation. The adapter therefore
  persists only thread rows whose serialized provider is exactly `zed.dev`.

## Private validation

Run the ignored validation only against a locally selected genuine artifact:

```sh
TOKENLEDGER_VALIDATION_SOURCE=zed \
TOKENLEDGER_VALIDATION_ARTIFACT=/private/path/to/threads.db \
cargo test --manifest-path src-tauri/Cargo.toml source_artifact_validation -- --ignored --nocapture
```

The report emits aggregate counts, a schema fingerprint, and pass/fail only.
It does not print the artifact path or content, and no real artifact belongs in
the repository.

## Synthetic coverage

The committed tests cover hosted versus external ACP attribution, unknown
Models as Unattributed Usage, session-level timestamps, cache-token categories,
project path extraction, unsupported versions and schemas, malformed rows,
privacy-marker absence from the Ledger, platform roots, first-scan backfill,
unchanged rescans, disappearance, source isolation, and cross-Source
partition invariants.

The fidelity limit is intentional: Zed's current writer proves session-level
`updated_at`, not trustworthy per-Request timestamps, so one Usage Record is
created per usage-bearing Session.
