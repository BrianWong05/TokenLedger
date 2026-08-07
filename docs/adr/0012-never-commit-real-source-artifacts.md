# Never commit real Source Artifacts

Every Source adapter must be validated against at least one genuine Source
Artifact, corroborated by an upstream schema or implementation or by multiple
independent samples. No complete user-produced artifact enters the repository,
even after sanitisation: it is inspected privately, then represented by a
minimal committed fixture with synthetic content and only the structures and
relationships needed to exercise ingestion. This gives up some fixture fidelity
because prompts, responses, paths, identifiers, and deleted SQLite values are
difficult to prove unrecoverable and Git history is permanent.

Evidence and support are version-specific: an adapter recognises only artifact
variants backed by evidence and reports an unfamiliar version rather than
silently reinterpreting it. Every supported variant has a minimal synthetic
fixture and expected Usage Records covering parsing, malformed or live-tail
data, idempotent rescans, overlapping roots, artifact disappearance, and the
rule that no content reaches the Ledger.

An adapter is called supported only when it also has automatic discovery,
stable deduplication, cross-Source invariant coverage, frontend identity and
filtering, documented fidelity limitations, and validation against the private
genuine artifact. A parser by itself is not Source support.

Validation may be direct private inspection or a trusted contributor running an
ignored real-artifact parity test locally. The contributor returns only
normalised counts, a schema/version fingerprint, and pass/fail output; the real
Source Artifact and its content do not need to leave their machine.

No production adapter or Source Catalog entry lands before that validation
gate. A locally isolated prototype may precede it, and the preferred backlog
order yields to the order in which valid artifacts actually become available.
