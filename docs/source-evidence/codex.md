# Codex Source evidence

Status as of 2026-08-13: Codex rollout parsing was already supported. Issue
[#131](https://github.com/BrianWong05/TokenLedger/issues/131) verified a genuine
relocated Codex home and this change expands discovery without changing the
parser, Ledger schema, or Source-native event keys.

## Upstream corroboration

The supported Artifact is produced by
[OpenAI Codex](https://github.com/openai/codex):

- Codex's [`find_codex_home`](https://github.com/openai/codex/blob/eb752e43d9b7bd7dc5965ea20642bcf7f1a492d8/codex-rs/utils/home-dir/src/lib.rs)
  resolves non-empty `$CODEX_HOME`, otherwise falling back to `~/.codex`.
- The production
  [`RolloutRecorder`](https://github.com/openai/codex/blob/eb752e43d9b7bd7dc5965ea20642bcf7f1a492d8/codex-rs/rollout/src/recorder.rs)
  constructs new rollout paths beneath
  `<codex_home>/sessions/YYYY/MM/DD/rollout-*.jsonl` and writes JSONL records.

TokenLedger applies an additive discovery policy: it scans
`~/.codex/sessions` first, then a visible nonblank `$CODEX_HOME/sessions`.
This preserves the established default-root `source_file` when both roots
refer to the same physical rollout.

## Private validation

The maintainer-side report in issue #131 compared two genuine Codex homes:
the relocated home contained 207 rollout files and the default home 177. Of
those, 175 were shared hard links with the same device, inode, and basename;
the relocated home contributed 32 unique rollouts and the default home two.
No private path or rollout content is committed.

## Synthetic coverage

The committed tests cover default-first root resolution, blank and equivalent
overrides, a configured top-level symlink, overlapping hard-linked rollouts,
first-scan backfill, unchanged rescans, malformed-line isolation, disappearance
of the configured root, durable Usage and Context totals, and suppression of
Limit Readings from non-default roots.

## Fidelity limitations

Environment discovery only works when the TokenLedger process can see
`CODEX_HOME`; desktop launches often do not inherit shell-only variables.
Relocated roots contribute Usage and Context, but not Limit Readings, until a
separate account-to-limits policy is agreed.
