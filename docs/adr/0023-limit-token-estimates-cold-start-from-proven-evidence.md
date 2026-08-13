# Limit token estimates cold-start from proven evidence

Claude and Codex expose a percentage, reset time, and window, but no token
numerator, denominator, or stable raw-token conversion. Their pools may also
include work outside the local Artifacts TokenLedger reads. A Limit Token
Estimate can therefore describe only a local, workload-dependent correlation;
it cannot recover the vendor's true token ceiling or prove that local Usage
Records caused the full movement.

Source and time are not enough to make that correlation honest. The current
Ledger carries no account identity, historical Scan results cannot certify that
an arbitrary old interval was complete, and Claude's model-scoped Limit key is
derived from a display name rather than a proven mapping to raw logged Models.
Treating any of those unknowns as continuity would make a plausible-looking
estimate from records that may belong to a different account, meter, or Model.

So comparable Readings belong to one **Limit Evidence Partition**, identified
by all of:

1. Source;
2. a proven privacy-safe account or subscription identity shared by the
   Reading and the Usage capture context;
3. plan and metering regime;
4. a stable raw vendor Limit identity, or an adapter-defined canonical identity
   with a documented one-to-one mapping;
5. reset epoch; and
6. explicit Model scope, mapped to exact raw logged Model identities when the
   Limit is model-scoped.

An unknown component never acts as a wildcard. A change to any component
starts a new partition. Duration, display labels, slugs, and pricing-name
normalisation are not identity evidence.

A **Limit Evidence Interval** joins two consecutive, distinct, increasing
Readings in one partition. Its Usage Records come from the current canonical
Ledger and satisfy:

```text
previous.observed_at < usage.timestamp <= current.observed_at
```

This previous-exclusive/current-inclusive boundary includes a Codex token delta
emitted in the same snapshot as the later Reading without assigning the earlier
snapshot's delta forward. The stored timestamp remains an observation boundary,
not a claim about request start or finish.

For a Source-wide Limit, every matching Source-and-account Usage Record
participates, including Unattributed Usage. For a model-scoped Limit, only
Records whose raw Model is in the explicit scope participate; a potentially
matching Unattributed Usage Record invalidates the interval rather than being
ignored or assigned to a guessed Model. Known nonmatching Models do not
participate.

An interval is also ineligible across a reset, decrease, saturation, account,
plan, meter, Limit-identity or scope change; when Source completeness is not
durably proven; or when external activity is known or detected. The mere
possibility of unobserved vendor-surface activity does not reject every
interval: the pairing is explicitly candidate correlation, and the estimator
must later require several consistent intervals and withhold an unstable result.
Unknown, unavailable, and incomplete evidence are never interpreted as zero.

Pairing is re-derived from the current canonical Ledger. Late discovery,
record upgrades, or supersession may revise prior intervals, so derived results
retain the contributing Reading and Usage Record identities rather than freezing
an unexplained sum.

These rules deliberately require a cold start. Existing Claude and Codex
history lacks proven account identity and historical completeness, so it cannot
be backfilled into evidence. TokenLedger shows “not enough data” until enough
post-migration observations satisfy the contract. Future Sources participate
through the same contract, and existing Sources may capture identity from data
they already read, but estimating tokens does not justify a new vendor request.
