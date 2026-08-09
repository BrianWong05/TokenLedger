# Source acquisition is local and passive

**Amended by ADR-0018**: this rule binds the *scan*, which still only reads
what is already on disk. A Companion — a separate executable, run because a
person asked — may talk to a Source to write an Export Artifact the scan can
then read. Nothing here loosens for the scan itself.

TokenLedger only reads Source Artifacts already present on the machine. An
already-populated third-party cache is acceptable, but TokenLedger never runs
the synchronising program, signs into the Source, handles account cookies or
API keys, or fetches private usage remotely. Cache-backed Source support is
therefore conditional and a missing cache behaves like a missing installation;
we accept that limitation to preserve the application's local privacy and
simple scan boundary.
