# The panel exists only where the platform delivers icon clicks

TokenLedger ships on Windows and Linux (Windows first). The Menu Bar Extra
stays one concept with one name, but its presentation follows what each
platform's status area can actually do: macOS keeps the icon, the Today
title beside it, and the click-toggled panel (ADR-0007); Windows keeps the
icon and panel but moves Today's figures into the icon's native tooltip,
because its notification area has no text-beside-icon; Linux gets no panel
at all — its tray (libappindicator/ayatana) delivers neither left-clicks
nor an icon rect, so a click-toggled, icon-anchored popover is impossible —
and instead carries a native menu: a disabled Today row, Open TokenLedger,
Scan now, Quit, with the panel's read-out left to the Overview.

This deliberately resurrects on Linux the native menu that ADR-0007 killed.
ADR-0007's fidelity argument still stands where a panel is possible; on
Linux the alternative was not "menu vs. panel" but "menu vs. a panel
floated at some arbitrary screen position from a menu item" — a window that
behaves like neither a popover nor a document window. Native idiom won over
mimicry. The rejected alternatives worth remembering: rendering Today's
figures into the tray icon bitmap (illegible at 16px, a rendering pipeline
to maintain) and dropping the tray on Linux entirely (kills the resident
background capture of ADR-0005 — a hidden resident app with no affordance
to find it again).

Consequence: the panel (traypanel window, its positioning and
dismiss-on-focus-loss) compiles everywhere but is only wired up on macOS
and Windows; the Linux menu is a second, small presentation path that must
carry the same Cost rules (Partial "≥", Unpriced never $0, Display
Currency) as every other surface.
