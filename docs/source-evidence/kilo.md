# Kilo Source evidence

Status as of 2026-08-07: the Kilo adapter and synthetic validation are
implemented; the genuine private-Artifact gate remains pending because no Kilo
`kilo.db` was available in the validation workspace. This file deliberately
does not claim a private-artifact pass.

## Upstream corroboration

The supported shape is the current CLI session database of
[Kilo Code](https://github.com/Kilo-Org/kilocode):

- `kilo.db` (platform roots: `~/Library/Application Support/kilo/kilo.db` on
  macOS, `~/.local/share/kilo/kilo.db` on Linux, `%LOCALAPPDATA%\kilo\kilo.db`
  on Windows, overridable with `KILO_DB`) holds `session` rows that aggregate
  usage (input, output, reasoning, cache-read, cache-write token totals) and
  `message` rows keyed to a session.
- Kilo's database aggregates usage on the Session row rather than proving a
  trustworthy timestamp for each Request, so one Usage Record is created per
  usage-bearing Session.
- Legacy editor migrations whose rows contain only zero tokens naturally
  produce no Usage Records.

## Private validation

Run the ignored validation only against a locally selected genuine artifact:

```sh
TOKENLEDGER_VALIDATION_SOURCE=kilo \
TOKENLEDGER_VALIDATION_ARTIFACT=/private/path/to/kilo.db \
cargo test --manifest-path src-tauri/Cargo.toml source_artifact_validation -- --ignored --nocapture
```

The report emits aggregate counts, a schema fingerprint, and pass/fail only.
It does not print the artifact path or content, and no real artifact belongs in
the repository.

## Synthetic coverage

The committed tests cover the supported `session`/`message` schema, token
category totals, session-level timestamps, zero-token legacy migrations
producing nothing, malformed and unsupported data, privacy-marker absence from
the Ledger, platform roots, first-scan backfill, unchanged rescans,
disappearance, source isolation, and cross-Source partition invariants.

## Fidelity limitations

Kilo's supported Artifact exposes Session totals without trustworthy per-Request
timing, so one Usage Record is created per usage-bearing Session at the
Session's updated timestamp. Unknown or unproven Models become Unattributed
Usage rather than being guessed from the last Model in a Session.
