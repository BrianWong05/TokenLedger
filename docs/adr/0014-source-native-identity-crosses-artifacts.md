# Source-native identity crosses Source Artifacts

When modern, legacy, migrated, or cached Source Artifacts overlap, TokenLedger
scans every artifact needed for complete coverage but treats the same
Source-native unit of work as one Usage Record. Deduplication uses a stable
identity from the Source rather than the artifact path, and the modern
representation wins when two artifacts describe the same work. This makes
identity extraction harder per adapter, but prevents migration, copying, or a
path change from manufacturing usage while preserving unmigrated history.
