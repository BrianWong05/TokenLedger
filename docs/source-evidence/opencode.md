# OpenCode Source evidence

Status as of 2026-08-07: the OpenCode adapter and synthetic validation are
implemented; the genuine private-Artifact gate remains pending because no
OpenCode `opencode.db` or legacy JSON storage was available in the validation
workspace. This file deliberately does not claim a private-artifact pass.

## Upstream corroboration

The supported shapes are the current and legacy storage of
[OpenCode](https://github.com/sst/opencode), the OpenCode CLI:

- The current storage is a SQLite database (default `~/.local/share/opencode/
  opencode.db`, overridable with `OPENCODE_DB`, and `opencode-<channel>.db`
  channel variants) whose session and message rows carry usage totals.
- The legacy storage is JSON under `~/.local/share/opencode/storage`
  (overridable with `OPENCODE_DATA_DIR`, plus the `XDG_DATA_HOME` branch).
- OpenCode's supported Artifact records usage per message row with the Model
  identity and the message creation time, but no trustworthy per-Request
  timing, so usage is booked one Record per proven Model per Session (plus
  one Record for messages whose Model is unproven); a Session whose usage
  proves a single Model stays one Record.

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
storage parsing, per-Model session splitting (a Session that used several
Models books one Record per Model, and Records of whichever booking shape
the Session no longer has are superseded in either direction; a Model is
only attributed when proven), session-level timestamps,
malformed and unsupported data, deduplication
of equivalent roots, privacy-marker absence from the Ledger, platform roots,
first-scan backfill, unchanged rescans, disappearance, source isolation, and
cross-Source partition invariants.

## Fidelity limitations

OpenCode's supported Artifact records usage per message row with the Model
identity and the message creation time, but no trustworthy per-Request
timing. A Session whose usage-bearing Requests prove a single Model is
booked as one Usage Record at the Session's updated timestamp. A Session
that used several Models splits into one Usage Record per Model, each
booked at that group's latest message creation time; whenever a Session's
booking shape changes, the Records of the shape it no longer has are
superseded. Requests whose Model is unproven are booked as Unattributed
Usage for their group only, rather than being guessed from another Model
in the Session.
