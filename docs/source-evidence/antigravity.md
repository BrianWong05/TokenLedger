# Antigravity Source evidence

Status as of 2026-08-08: the Antigravity adapter reads the SQLite Session
databases (`.db`) of both the IDE and the CLI, verified against this
machine's real databases. The encrypted `.pb` Session files are
evidence-blocked: they cannot be read passively, so their tokens never reach
the Ledger. This is a missing-Artifact limitation, not a parser gap.

## Supported shape: `<uuid>.db`

Each Session is one SQLite database under
`~/.gemini/antigravity/conversations/` (IDE) or
`~/.gemini/antigravity-cli/conversations/` (CLI), same schema:

- `gen_metadata(idx, data)` — one row per generation (one API call), protobuf
  blob: `chatModel.#19` model alias, `.#9.#4` per-generation timestamp,
  `.#4` usage (system input #1, fresh input #2, cache-read #5, output #9,
  thinking #10, responseId #11). Field numbers follow tokscale's reverse
  engineering and were re-verified against genuine databases: output #9 +
  thinking #10 = the API's total output.
- `trajectory_metadata_blob` — Session created-at (#2) and workspace
  `file://` URI (#1.#1).

Wire aliases (`gemini-3-flash-a`/`-b`, `gemini-default`) are resolved to real
Model ids at parse time; see `resolve_model` in the adapter.

## Blocked shape: `<uuid>.pb`

Investigated 2026-08-08 against this machine's genuine installation, where
the IDE migrated every Session to encrypted `.pb` (2026-05-20) and the
`conversations/` directory holds 100 `.pb` files against a single `.db`:

- Whole-file ciphertext: byte entropy ≈ 8.0 from offset zero, no common
  header across files, no plaintext protobuf anywhere.
- The historical scheme (AES-128-CTR, 16-byte nonce prefix, key from the
  macOS Keychain item "Antigravity Safe Storage" / "Antigravity Key") does
  not decrypt files written by the current IDE. Third-party decryptors report
  the same against recent Antigravity versions — see
  [arashz/antigravity_decryptor issue #5](https://github.com/arashz/antigravity_decryptor/issues/5)
  and the
  [r/google_antigravity thread on decrypting `.pb` Sessions](https://www.reddit.com/r/google_antigravity/comments/1qtz007/decrypting_pb_conversations/).
- Exhaustive offline attempts — both Keychain items ("Antigravity Safe
  Storage", "Antigravity IDE Safe Storage"), raw and PBKDF2-derived keys,
  CTR/CBC/GCM, nonce and skip offsets — produced no parseable protobuf.
- Plaintext side Artifacts carry no usage: `agyhub_summaries_proto.pb` holds
  titles and timestamps only, `annotations/*.pbtxt` holds view times, and
  `brain/` holds artifact markdown.
- The only known working export path drives Antigravity's own language
  server, which ADR-0013 rules out: TokenLedger reads already-present
  Artifacts and never runs the Source's programs.

The `.pb` shape reopens only if Antigravity publishes the scheme or writes a
passively readable usage Artifact. Until then the adapter skips `.pb` files
silently. That deviates from ADR-0015's rule that an existing unsupported
Artifact emits a Source-specific warning; the deviation is deliberate and
recorded here rather than papered over: `.pb` is not a malformed instance of
a supported shape but a permanently rejected Artifact class — present on
every scan and unparseable offline without violating ADR-0013 — so a repeated
warning would be noise with no remedy.

## Synthetic coverage

The committed tests cover generation decoding with workspace and per-
generation timestamps, wire-alias resolution and era-based `gemini-default`
mapping, responseId deduplication, zero-row skipping, unchanged-rescan and
growth-rescan idempotency, parser-version bump re-parsing, multi-root IDE +
CLI scanning, `.pb` files ignored, and missing roots staying quiet.
