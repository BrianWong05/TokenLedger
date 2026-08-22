# OpenCode Source evidence

Status as of 2026-08-23: the OpenCode adapter and synthetic validation are
implemented, and the genuine private-Artifact gate has now run. A real
`~/.local/share/opencode/opencode.db` — 29 Sessions, 345 Messages, 253
usage-bearing Requests — passes: 253 Records, 253 Requests, 11,805,696
tokens, no Unattributed usage.

The gate had been pending since 2026-08-07 for want of an Artifact, and this
file's fidelity claim was written without one. It was wrong; TOKL-24 replaced
it. Read the section below as the checked version.

## Upstream corroboration

The supported shapes are the current and legacy storage of
[OpenCode](https://github.com/sst/opencode), the OpenCode CLI:

- The current storage is a SQLite database (default `~/.local/share/opencode/
  opencode.db`, overridable with `OPENCODE_DB`, and `opencode-<channel>.db`
  channel variants) whose session and message rows carry usage totals.
- The legacy storage is JSON under `~/.local/share/opencode/storage`
  (overridable with `OPENCODE_DATA_DIR`, plus the `XDG_DATA_HOME` branch).
- OpenCode's supported Artifact records usage per message row with the Model
  identity and the message's own creation time, so usage is booked one Record
  per Request at that Request's timestamp.

## Private validation

Run the ignored validation only against a locally selected genuine artifact:

```sh
TOKENLEDGER_VALIDATION_SOURCE=opencode \
TOKENLEDGER_VALIDATION_ARTIFACT=/private/path/to/opencode.db \
cargo test --manifest-path src-tauri/Cargo.toml source_artifact_validation -- --ignored --nocapture
```

The report emits aggregate counts, a schema fingerprint, and pass/fail only.
It does not print the artifact path or content, and no real artifact belongs in
the repository.

## Synthetic coverage

The committed tests cover SQLite session/message usage totals, legacy JSON
storage parsing, per-Request booking (each usage-bearing Message books one
Record at its own timestamp, with the Session's used only as a fallback; a
Model is only attributed when proven), supersession of the pre-TOKL-24
per-Session and per-Model aggregate rows and of Records whose Request the
Artifact no longer holds, malformed and unsupported data, deduplication
of equivalent roots, privacy-marker absence from the Ledger, platform roots,
first-scan backfill, unchanged rescans, disappearance, source isolation, and
cross-Source partition invariants.

## Fidelity limitations

Every usage-bearing assistant Message carries its own `time.created`, and
the database mirrors it into `message.time_created` — present, and equal to
the column, on 253 of 253 Requests in the validated Artifact. So each
Request books its own Usage Record at its own timestamp, with the Model that
produced it. Requests whose Model is unproven book their own Unattributed
Record rather than borrowing a Model from elsewhere in the Session.

A Message with no timestamp of its own falls back to its Session's — the
real Artifact holds none, but the legacy JSON shape is not guaranteed to
carry one. `time.completed` is read by nothing: booking at `created` matches
how every other per-Request Source is stamped.
