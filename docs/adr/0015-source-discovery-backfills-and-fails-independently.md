# Source discovery backfills history and fails independently

When TokenLedger first discovers a Source Artifact, it ingests every available
Usage Record rather than imposing a historical cutoff; later scans use that
Source's safe incremental or unchanged-artifact strategy. A missing path is a
normal empty Source, while an existing unsupported or malformed artifact emits
a Source-specific warning, leaves every previously ingested Usage Record in the
Ledger, and cannot stop other Sources from scanning. We accept a potentially
expensive first scan to avoid permanent, arbitrary gaps in the Ledger.
