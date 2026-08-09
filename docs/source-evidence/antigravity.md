# Antigravity Source evidence

Status as of 2026-08-09: the Antigravity adapter reads the SQLite Session
databases (`.db`) of both the IDE and the CLI, verified against real
databases from one genuine macOS installation (detailed below). The encrypted
`.pb` Session files still cannot be read passively, so on their own they remain
Unreadable Artifacts (ADR-0017) and mark affected totals with "≥". They are no
longer a dead end, though: the `antigravity-export` companion decrypts them
through Antigravity's own running language server and writes an Artifact the
scan can read (ADR-0018, "Reading the `.pb` Sessions" below).

Field numbers throughout are no longer reverse-engineered guesses. The language
server ships its own protobuf descriptors, and `ChatModelMetadata` /
`ModelUsageStats` were read straight out of them.

## Supported shape: `<uuid>.db`

Each Session is one SQLite database under
`~/.gemini/antigravity/conversations/` (IDE) or
`~/.gemini/antigravity-cli/conversations/` (CLI), same schema:

- `gen_metadata(idx, data)` — one row per generation (one API call), protobuf
  blob: `chatModel.#19` model alias, `.#9.#4` per-generation timestamp, `.#4`
  usage (`ModelUsageStats`). Its fields, read from the server's own descriptors:
  **#1 is the Model enum, not a token count**; #2 input; #3 total output; #4
  cache-write; #5 cache-read; #9 thinking; #10 response text; #11 responseId.
  Cross-checked on real databases — #3 == #9 + #10 on every row carrying all
  three (4,645/4,645), and #1 is constant per Session (e.g. 1132), which is what
  gives it away as an identifier. Reading #1 as "system prompt tokens" inflated
  every generation's input by ~1,100 until 2026-08-09; #9 and #10 were also
  labelled the wrong way round, so `reasoning_tokens` reported the response side.
- `trajectory_metadata_blob` — Session created-at (#2) and workspace
  `file://` URI (#1.#1).

Wire aliases (`gemini-3-flash-a`/`-b`, `gemini-default`) are resolved to real
Model ids at parse time; see `resolve_model` in the adapter.

## Blocked shape: `<uuid>.pb`

Investigated 2026-08-08 and re-checked 2026-08-09 against one genuine macOS
installation, where the IDE migrated Sessions to encrypted `.pb` (2026-05-20)
and the `conversations/` directory holds 100 `.pb` files alongside 14 `.db`.
The two sets are disjoint — no Session id appears as both — so the `.pb` files
are 100 distinct Sessions, not encrypted copies of readable ones. The
`.db`/`.pb` mix varies by machine and migration date, so other installs may
hold more readable `.db` Sessions:

- Whole-file ciphertext: byte entropy ≈ 8.0 from offset zero, no common
  header across files, no plaintext protobuf anywhere.
- Byte-size mod 16 is uniformly distributed across the 100 files, which rules
  out CBC/ECB block padding: whatever the scheme is, it is a stream cipher.
- The historical scheme (AES-128-CTR, 16-byte nonce prefix, key from the
  macOS Keychain item "Antigravity Safe Storage" / "Antigravity Key") does
  not decrypt files written by the current IDE, and the reason is now positive
  rather than inferred: the strings "Safe Storage" and "Antigravity Key" do not
  occur *anywhere* in either shipped language server binary (119 MB and 114 MB,
  full symbol tables searched). The only keyring use left is `zalando/go_keyring`
  for OAuth tokens ("Restored saved token from keyring"). The old scheme is gone,
  not merely rekeyed, which is why every third-party decryptor now fails.
  Third-party decryptors report the same against recent Antigravity versions — see
  [arashz/antigravity_decryptor issue #5](https://github.com/arashz/antigravity_decryptor/issues/5)
  and the
  [r/google_antigravity thread on decrypting `.pb` Sessions](https://www.reddit.com/r/google_antigravity/comments/1qtz007/decrypting_pb_conversations/).
- Exhaustive offline attempts — both Keychain items ("Antigravity Safe
  Storage", "Antigravity IDE Safe Storage"), raw and PBKDF2-derived keys,
  CTR/CBC/GCM, nonce and skip offsets — produced no parseable protobuf. Keys
  derived from `installation_id` (the only key-shaped file on disk) fail too:
  55 derivation/mode combinations, no hits.
- The encryption is real, not a parsing failure. `user_settings.pb`, a `.pb` in
  the same tree, is plaintext protobuf (entropy 3.998, header `0801300348…`)
  while every conversation `.pb` sits at 7.999.
- Plaintext side Artifacts carry no usage: `agyhub_summaries_proto.pb` holds
  titles and timestamps only, `annotations/*.pbtxt` holds view times, and
  `brain/` holds artifact markdown.
- The only thing that decrypts them is Antigravity's own language server, which
  ADR-0013 rules out *for the scan*: TokenLedger reads already-present Artifacts
  and never talks to a Source. See "Reading the `.pb` Sessions" below for the
  companion that does it out of process.

A `.pb` with no export is still an Unreadable Artifact (ADR-0017): never warned,
but counted. The no-warning half stands on the reasoning first recorded here:
`.pb` is not a malformed instance of a supported shape but an Artifact class the
scan must reject — present on every scan and unparseable offline — so a repeated
warning would be noise with no remedy. That argument defeats the warning, which
requests an action that does not exist; it never reached the numbers. A
completeness marker asks nothing, and without one a Source with a hundred
unreadable Sessions and a Source read in full look alike — so the adapter
reports the `.pb` count and latest mtime, and every token total whose window
that content could fall in carries the "≥" marker.

## Reading the `.pb` Sessions

`antigravity-export` (`src-tauri/src/bin/antigravity-export.rs`, ADR-0018) is a
companion binary, run by a person, never by the scan. It asks the
*already-running* language server for what it can already read and writes
`<session>.tokenledger.json` beside each `.pb`; the adapter then reads those
files passively, and a Session with an export stops counting as unreadable.

It ships as a Tauri sidecar (`bundle.externalBin`, built by
`scripts/build-sidecar.mjs` via `npm run sidecar`) and is offered as a Decrypt
button beside the "≥" reason in the Overview, which rescans once it finishes.
Run it from a terminal with `cargo run --bin antigravity-export`.

- Transport: gRPC over HTTP/2 on the server's plaintext loopback port,
  authenticated with the `x-codeium-csrf-token` header whose value is the
  `--csrf_token` argument of the running process. The TLS and LSP ports reject
  the call, so a successful decode is what identifies the right port.
- RPCs: `GetCascadeTrajectoryGeneratorMetadata` (request field 1 = session id)
  returns the generations; `GetConversationMetadata` supplies the workspace.
- Everything is discovered at run time — process list for the token, `lsof` /
  `ss` / `netstat` for that process's ports. The port is assigned per launch and
  differs between runs on the same machine, so nothing may be pinned.
- Verified 2026-08-09 on this installation: 100/100 Sessions exported, 0
  failures, 4,466 generations — 59.1M input, 266.4M cache-read, 3.2M output.

Caveat worth keeping: a naive "find every message shaped like usage" search
over the response double-counts badly, because `ChatModelMetadata` itself has a
varint at #3 and a string at #11 and so matches the shape, contributing its
Model enum as though it were output tokens. Navigate the fields explicitly.

## Synthetic coverage

The committed tests cover generation decoding with workspace and per-
generation timestamps, wire-alias resolution and era-based `gemini-default`
mapping, responseId deduplication, zero-row skipping, unchanged-rescan and
growth-rescan idempotency, parser-version bump re-parsing, multi-root IDE +
CLI scanning, the Model enum staying out of input and #3 winning over its
parts, export ingestion (events, project, alias resolution, placeholder model
ids, unknown-schema warning) with an exported `.pb` no longer counted as
unreadable and an export sharing a responseId with a database collapsing to one
event, `.pb` files counted as Unreadable Artifacts (count and latest
mtime, no warning), and missing roots staying quiet.
