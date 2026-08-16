# Resident background capture: launch at login defaults ON

TokenLedger enrolls itself to launch at login by default, starts hidden with
only a tray/menu-bar icon, and scans Sources in the background on a lazy
schedule (on start, then every few hours). A one-time first-run notice
discloses the enrollment and points at the Settings toggle that turns it off.

Default-on autostart is normally user-hostile, and the respectful default
(off) was the real alternative. We chose on because the domain demands it:
Sources prune their logs (Claude Code deletes transcripts after ~30 days), the
Ledger's whole premise is that a Usage Record outlives its source log, and an
app that isn't running cannot capture. With autostart off, a user who doesn't
open the app for a month loses Usage Records irreversibly — a silent data-loss
default. Capture-by-default with disclosed, one-toggle opt-out was judged the
lesser harm.

Consequences: the app is a resident tray app, not a window you open — closing
the window must not kill capture, and quit lives in the tray. Scan cadence can
stay lazy (hours, not minutes) because log retention is measured in weeks.

## Amendment (2026-08-16, TOKL-4)

The lazy cadence's premise was capture alone. The Menu Bar Extra's bar figures
made the resident scan a display surface too, and a number that is hours stale
beside the icon reads as broken. The resident schedule is now two tiers: a
reader-paced Menu Bar Extra refresh timer (Off / 1m / 5m / 15m, default 1m, a
Settings section of its own) drives frequent Scans, while the original
every-few-hours cadence survives underneath as the untouchable capture floor —
"Off" slows the bar back to that floor and never stops recording, so the
data-loss argument above still holds. "Hours, not minutes" now describes only
the floor. The bar additionally repaints at local midnight so Today never
shows yesterday's total. A minutes-pace Scan is affordable because per-file
skip state makes a no-change pass a directory walk plus a stat per file.

The tightened cadence also cost the resident loop its `prices-rebuilt` emit.
Nudging an open Overview was harmless once every four hours; at a minute it
re-reads a window whose reader may have turned that very timer off, and clears
the reprice cache each time. The bar is repainted by the scan itself, and the
Overview keeps its own timer, so the loop now pushes nothing at it.
