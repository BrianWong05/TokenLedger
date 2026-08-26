# The Menu Bar Extra is a webview panel, not a native menu

Supersedes ADR-0006. The tray icon toggles a small frameless webview window
(the traypanel) that renders design 2b pixel-faithfully — styled Today
header, pace delta, column-aligned per-Source rows, action rows — instead of
opening a native NSMenu.

ADR-0006 chose the native menu to keep platform behavior and a small
surface, accepting that the mock's styling was unreproducible. That trade
failed sign-off twice: disabled stat rows were unreadably grey, and the
enabled-but-inert fix still left the flattened text looking nothing like the
design the user had picked. The user asked for the mock. A native menu
cannot render it — full stop — so fidelity forced the panel.

What the panel costs, accepted knowingly: menu behavior is reimplemented
(positioning under the icon, dismiss-on-focus-loss) rather than inherited; a
second webview exists while the panel is open; macOSPrivateApi is enabled for
the transparent rounded window. The panel webview is created lazily on click
and destroyed on dismissal, trading a small reopen cost for no resident
renderer while the panel is closed. What it buys: the panel is ordinary
frontend — the 2b look exactly, the app's own formatters and ports (Display
Currency, "≥" Partial marker, unpriced wording come from the same code the
Overview uses), and testable view logic in vitest instead of Rust
string-building.

The bar title (tokens · Cost) stays native and stays computed in Rust — text
beside the icon is the one thing a webview cannot do.

## Amendment (2026-08-21)

The 2b look this ADR cites was replaced by the "Option A + Models · bars"
redesign (the menu-bar-panel-redesign canvas): the period tabs double as the
header, Rescan is a top-right refresh icon, the per-Source rows became a
stacked share bar with a legend, Cost per bucket draws as columns, Model rows
carry their Source's mark, the stats sit in tiles, and the actions are icon
buttons. The costliest-Project read-out was dropped with the redesign, so the
panel no longer fetches the Project breakdown, and the window widened 300 →
320 (material radius 8 → 14) with it. Everything this ADR decided still
stands: the surface remains an ordinary-frontend webview on the same ports,
formatters, and lazy create/destroy lifecycle — only the design it renders
pixel-faithfully changed.

## Amendment (2026-08-25)

The panel is capped, and its colours no longer assume a dark desktop.

A window that hugs its content has no ceiling of its own: with eight Sources,
twenty-one Models and three Sources reporting Limits the card measured ~950
logical px, and an anchored panel taller than the monitor's work area has
nothing left to clamp against (`panel_position`'s `clamp_or_leave`), so the
bottom simply hung off a 14" screen. Four shapes were prototyped — cap the
lists, cap the window and scroll it, collapse the detail behind disclosures,
and swap the detail through one tabbed slot. Capping the lists won: the panel
is a glance, the app already holds the full story, and the other three all keep
a full-height panel or hide something a glance wants. So the legend names three
Sources and the Models list three rows, each counting what it hid; the Limits
section became one line per Source carrying the window nearest its wall and
disclosing that Source's others; and the section rhythm tightened 16 → 12px.
Measured on the shape that provoked this: 643px. What it costs, accepted: the
collapsed line no longer names a second window or its reset countdown, and the
Rust-side clamp on `resize_panel` is still owed — three Sources expanded add
~200px, and nothing yet stops that from leaving the screen again.

The colours moved for a different reason. The material behind the card
transmits whatever is behind the *window*, so "Dark mode" was never a promise
that the card comes out dark: measured on a real panel over a white document it
composited to rgb(65,67,70), nothing like the #1e1e24 the type was calibrated
on, leaving the quiet captions at 1.7:1. The Dark veil is therefore no longer
thin (0.20/0.28/0.36 → 0.55/0.62/0.70, still lighter at every stop than the
Light-mode calibration, which starts from a near-white material) and the quiet
type tiers became tokens and came up: the captions, axis, token figures and
countdowns #6e6e76 → #85858e, and the tier above them — the Rescan glyph and
the tile captions — #8a8a93 → the #a3a3ab this file already used elsewhere,
rather than a third grey a few units away from it. The
honest limit: no veil short of opaque clears 4.5:1 on a white backdrop, because
the card's top gradient stop is about as light as the material itself — the
glass is what caps it. Going opaque was offered and declined; the panel keeps
the glass and the type does what work it can.

## Amendment (2026-08-27)

The Limits card is back; the one-line form the amendment above introduced is
reverted at the user's request.

That form gave each Source one line — the window nearest its wall, with the
Source's other windows behind a disclosure — and it was the largest single
saving in the capping work, ~250px of cards down to ~100px of lines. The user
asked for the card back after living with it. What the card restores: two
meters per Source, the Session and the Weekly lane side by side, each with its
reset countdown, which is the thing the line could not show without a click.
What it costs, knowingly: the panel measures 720px rather than 643px on the
shape that provoked the capping, and a Source that recorded neither named lane
— credits, or a raw duration — collapses to a bare name again, its figures one
click away. A test pins that last behaviour so it stays a decision rather than
a surprise.

Two things from the reverted design were kept deliberately. The type and veil
recalibration is untouched: it answered a different report and nothing about it
depended on the line. And each meter still picks the window nearest its wall
within its lane, rather than the first one the query hands over: Antigravity
records a Session per pool (`gemini:w300` and `3p:w300`) ordered by key, so the
`find` this ADR's previous shape used let the alphabet decide which pool the
Session meter spoke for. Restoring the card did not mean restoring that.

Everything the caps decided stands: three Sources in the legend, three Model
rows, the 12px section rhythm, and the Rust-side clamp on `resize_panel` still
owed — more so now, since the taller card leaves less headroom before an
expanded Source runs off the screen.
