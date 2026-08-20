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
