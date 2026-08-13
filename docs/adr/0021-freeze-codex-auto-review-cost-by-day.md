---
status: accepted
---

# Freeze Codex Auto Review Cost by day

`codex-auto-review` is an internal route whose public price may appear or change
without the Usage Record naming a different Model. TokenLedger therefore stores
one resolved rate snapshot per local calendar day for that Model. A closed day
keeps its snapshot when later catalog refreshes change today's rate.

An Unpriced earlier day takes the first priced snapshot observed after it. This
implements the explicit assumption that the route used today's Model while
turning the backfill into a fixed historical fact; later prices cannot rewrite
it again. Other Models retain ADR-0009's current resolution behavior.
