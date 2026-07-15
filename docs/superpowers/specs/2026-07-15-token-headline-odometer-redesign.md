# Token Headline Odometer Redesign

## Problem

The current mode-change animation has two visual defects:

- Each digit moves through only one glyph-height, so the motion reads as a light slide rather than a rolling odometer.
- Exact-to-compact transitions paint independently centered outgoing and incoming layers. The wide exact layer remains visible outside the compact target width and overlaps the new value.

## Approved design

Replace the independent layers with one clipped, centered character grid. The grid has one slot per character in the wider of the source and target strings. Source and target characters are centered into the same slots, so a slot can contain one digit reel, a fading symbol, or an empty value; whole-number layers never paint over each other.

The outer viewport animates from the source string width to the target string width with `overflow: hidden`. Each numeric slot is clipped independently. A digit reel performs at least one complete upward `0–9` cycle before reaching its target; disappearing digits perform one complete upward cycle and fade at the end. Separators, decimal points, and `K`/`M`/`B` suffixes crossfade inside their own slots.

## Motion

- Total sequence: 1,400 ms.
- Neighboring numeric slots settle about 55 ms apart, left to right.
- All reels start together.
- Motion direction for format-only changes is upward.
- The viewport width morph and digit reels share the same 1,400 ms envelope.
- `prefers-reduced-motion: reduce` remains an immediate swap.
- Identical compact and exact strings still persist the selected mode without animation.

## Responsive and accessible behavior

The headline remains a semantic button with its existing accessible name, tooltip, persisted preference, focus/hover affordances, tabular numerals, centered block layout, and single-line responsive sizing. The animated grid is `aria-hidden`; the button's accessible name exposes the authoritative exact total and next action.

## Testing

Use the existing `TokenTotalHeadline` component DOM seam. Add a deterministic exact-to-compact regression that proves the active transition has one clipped viewport rather than multiple unbounded whole-number layers. Update controlled-clock assertions to remain busy through 1,399 ms and settle at 1,400 ms. Keep coverage for compact-to-exact, exact-to-compact, identical strings, persistence, layout, and Reduce Motion.

No backend, Ledger, query, Cost, or domain-model behavior changes.
