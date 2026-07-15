# Token Headline Reference Motion Design

## Goal

Match the aggressive rolling-number motion in the supplied screen recording when the Total tokens headline switches between compact and exact formats. Both directions should briefly produce the reference's intentionally scrambled, multi-row reel appearance before resolving into a sharp, centered value.

## Reference characteristics

The supplied recording establishes these visual requirements:

- The complete sequence lasts about 1.3–1.4 seconds.
- Numeric columns begin moving together but settle from left to right.
- Each column travels through multiple digits and exposes partial glyphs above and below the normal baseline.
- The intermediate value is intentionally difficult to read; this is part of the desired visual strength.
- The layout adopts the target width early instead of leaving compact and exact values painted beside one another.
- Decimal points, grouping separators, and unit suffixes remain visually anchored near the baseline while numeric reels move around them.
- The final value lands cleanly without a second width or font-size jump.

## Architecture

`TokenTotalHeadline` remains responsible for display mode, persisted preference, accessible labeling, click handling, Reduce Motion, and the 1,400 ms animation lifecycle.

`RollingTokenTotal` continues to render the transition, but its inputs are converted into a deterministic reel plan before rendering. Each aligned slot records:

- its source and target character;
- whether it is a numeric reel, stable symbol, entering symbol, or exiting value;
- the ordered digit sequence it will traverse;
- its travel distance and settle duration;
- a deterministic phase variation derived from slot position.

The planner contains no runtime randomness. The same source and target strings always produce the same choreography, keeping the motion repeatable and the tests stable. React renders the plan once per toggle; CSS animations perform the motion without frame-by-frame component rerenders.

## Motion choreography

### Horizontal behavior

The outer headline viewport moves to the target string width in 160 ms using a fast ease-out curve. Obsolete exact digits are clipped horizontally as the compact stage forms, while newly required exact slots become available immediately in the reverse direction. This prevents independent whole-number layers from appearing side by side.

The target value remains centered throughout the transition. Odd/even string-length differences retain the existing half-character alignment correction.

### Numeric reels

Every active numeric slot travels upward through at least one complete `0–9` cycle. A repeating per-column pattern adds one extra full cycle to alternating reels, so adjacent columns do not form a uniform horizontal row during the transition.

All reels begin together. Their durations increase by 55 ms from the leftmost active numeric slot to the rightmost, with the final slot settling at 1,400 ms. The easing accelerates quickly, sustains visible travel, and decelerates decisively into the target digit.

Each slot uses a 1.5-character-height vertical reveal window. It clips the reel locally while exposing partial neighboring digits above and below the baseline. The outer viewport permits this controlled vertical exposure while continuing to clip horizontally. An 8% alpha fade at the top and bottom boundaries softens clipped glyph edges. The current mid-reel blur is removed so the reference's discrete digit shapes remain visible.

### Symbols

Commas, decimal points, and `K`/`M`/`B` suffixes occupy fixed target slots near the baseline. Unchanged symbols remain stable. Changed symbols crossfade early in the sequence, rather than waiting until the final frames, so punctuation gives the moving digits a stable visual scaffold.

### Completion

At 1,400 ms the animation tree is replaced by the authoritative target string. The target width, centering, and responsive font size already match the settled state, preventing a completion jump.

## Interaction and accessibility

- The headline remains a semantic button with the exact total and next action in its accessible name.
- The animated grid remains `aria-hidden`; `aria-busy` remains present for the active sequence.
- The selected mode is persisted at click time.
- A repeated click safely retargets the animation from the selected display mode and replaces the prior timer.
- Identical compact and exact strings persist the selected mode without animating.
- `prefers-reduced-motion: reduce` swaps immediately to the target value.
- The Cost line remains below the headline and is not included in the animation.

## Testing and verification

Component tests will verify:

- both compact-to-exact and exact-to-compact transitions last 1,400 ms;
- the outer viewport adopts the target width early and clips obsolete horizontal content;
- numeric reels contain at least one complete digit cycle;
- deterministic phase variation produces non-uniform reel lengths or travel distances;
- settle durations increase from left to right;
- punctuation and suffixes are present in anchored target slots during motion;
- the final string, accessible state, persistence, identical-string behavior, and Reduce Motion remain correct;
- responsive exact totals remain on one line.

The implementation will also be sampled at multiple timestamps with the same representative totals used in the reference recording and compared visually with its compact-to-exact and exact-to-compact sequences.

## Scope

This change affects only the Total tokens headline's format-toggle animation. It does not change token formatting, data loading, Ledger queries, Cost behavior, date ranges, initial-load motion, or other dashboard sections.
