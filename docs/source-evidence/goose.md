# Goose Source evidence

Status as of 2026-08-07: the Goose adapter and synthetic validation are
implemented; the genuine private-Artifact gate remains pending because no Goose
`sessions.db` or legacy session `.jsonl` file was available in the validation
workspace. This file deliberately does not claim a private-artifact pass.

## Upstream corroboration

The supported shapes are the current and pre-1.10 storage of
[Goose](https://github.com/block/goose), Block's local coding agent:

- The modern storage is a SQLite `sessions.db` (schema version 15) whose
  `usage_ledger` rows carry per-request timestamps and per-row Model values.
  Goose writes inclusive input tokens, so the adapter removes its cache
  buckets from Input to store exclusive categories.
- The legacy storage is session `.jsonl` records written before 1.10; they
  expose only a session aggregate and are read from their first line only.
- Platform roots mirror Goose's data directory: `~/Library/Application
  Support/Block/goose/data/sessions` on macOS, `~/.local/share/goose/sessions`
  on Linux, `%APPDATA%\Block\goose\data\sessions` on Windows, with
  `$GOOSE_PATH_ROOT/data/sessions` as the documented override.

## Private validation

Run the ignored validation only against a locally selected genuine artifact:

```sh
TOKENLEDGER_VALIDATION_SOURCE=goose \
TOKENLEDGER_VALIDATION_ARTIFACT=/private/path/to/sessions.db \
cargo test --manifest-path src-tauri/Cargo.toml source_artifact_validation -- --ignored --nocapture
```

The report emits aggregate counts, a schema fingerprint, and pass/fail only.
It does not print the artifact path or content, and no real artifact belongs in
the repository.

## Synthetic coverage

The committed tests cover `usage_ledger` schema-version handling, inclusive
input normalization across cache buckets, per-row Model and timestamp parsing,
legacy session-aggregate records with a null Model, malformed and unsupported
files, privacy-marker absence from the Ledger, platform roots, first-scan
backfill, unchanged rescans, disappearance, source isolation, and cross-Source
partition invariants.

## Fidelity limitations

Modern `usage_ledger` rows become one Usage Record each. Legacy `.jsonl`
session records expose only a session aggregate, so they become one Usage
Record at the session timestamp with a null Model. Goose's cache-write bucket
has no TTL, so it is booked as the 5-minute Cache write category; Goose's
logged Cost is ignored and repriced from TokenLedger's rates.
