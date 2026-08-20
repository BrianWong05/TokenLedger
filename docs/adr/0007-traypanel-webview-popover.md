# The Menu Bar Extra is a webview panel, not a native menu

Supersedes ADR-0006. The tray icon toggles a small frameless webview window
(the traypanel) that renders the current Menu Bar Extra mock pixel-faithfully
instead of opening a native NSMenu. The mock is Glance + Models · bars:
period tabs, hero Cost and pace, a stacked Source Cost bar with legend, a
Cost bar chart, Model rows led by their Source mark, stat tiles (Cache Hit
Rate, the costliest Project, last Scan), and icon actions with the keyboard
shortcut printed beside each one that has one. (An earlier mock, design 2b,
was the same decision with a different look — column-aligned Source rows and
text action rows. The webview stayed; the pixels moved.)

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
frontend — the mock's look exactly, the app's own formatters and ports
(Display Currency, "≥" Partial marker, unpriced wording come from the same
code the Overview uses), and testable view logic in vitest instead of Rust
string-building.

The bar title (tokens · Cost) stays native and stays computed in Rust — text
beside the icon is the one thing a webview cannot do.
