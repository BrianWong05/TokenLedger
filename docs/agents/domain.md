# Domain docs

This is a single-context repository. Engineering skills must use its domain
documentation when exploring, specifying, or changing behavior.

## Before exploring

- Read the root `CONTEXT.md` for the ubiquitous language.
- Read ADRs under `docs/adr/` that touch the area being changed.
- If either location is absent, proceed silently; domain-modeling workflows
  create documentation lazily when a real term or decision is resolved.

## Use the glossary vocabulary

Use canonical domain terms in issues, specifications, tests, and code. Do not
replace a glossary term with a synonym that `CONTEXT.md` explicitly avoids.
For example, use Source, Model, Cost, Partial Cost, Unpriced, and
Cache-Estimated with their documented meanings.

If a required concept is missing from the glossary, reconsider whether new
language is needed and raise genuine gaps through the domain-modeling workflow.

## Respect architectural decisions

Read relevant ADRs before specifying implementation decisions. If proposed
work contradicts an accepted ADR, surface that conflict explicitly rather than
silently overriding the decision.
