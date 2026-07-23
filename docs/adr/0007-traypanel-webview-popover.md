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
(positioning under the icon, dismiss-on-focus-loss) rather than inherited;
a second webview rides along; macOSPrivateApi is enabled for the transparent
rounded window. What it buys: the panel is ordinary frontend — the 2b look
exactly, the app's own formatters and ports (Display Currency, "≥" Partial
marker, unpriced wording come from the same code the Overview uses), and
testable view logic in vitest instead of Rust string-building.

The bar title (tokens · Cost) stays native and stays computed in Rust — text
beside the icon is the one thing a webview cannot do.
