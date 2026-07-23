# Menu bar stats render as a native menu, not a webview panel

The Menu Bar Extra's stats — the Today header and the per-Source rows of
design 2b ("Menu Bar - Options", the ★ pick) — render as disabled items in a
real native menu. The styled mock is not reproduced: no 19px spend figure, no
colored pace delta, no column-aligned rows.

The real alternative was a webview popover panel anchored to the tray icon
(design 1b territory), which would render 2b pixel-perfect. We chose the
native menu because the design's own intent was "menu-shaped" (its stat rows
are explicitly non-interactive — "no fake hover" — which maps exactly to
disabled native items), and because a popover is a fake menu: positioning,
dismiss-on-blur, and keyboard behavior all have to be reimplemented, and a
second webview rides along just to draw text. The tradeoff is pixel fidelity
for native behavior and a far smaller surface.

Consequences: the header flattens to text rows ("Today: $12.84 · +12.4% vs
yesterday"), the pace delta lives inside the cost row's text so its absence
never restructures the menu, and per-Source rows are icon + single-line text.
Menu content updates in place (set_text) after every scan; the menu is
rebuilt only when Source membership changes. A future request for the mock's
visual styling means revisiting this ADR toward the popover — not styling the
native menu, which cannot do it.
