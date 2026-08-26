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
