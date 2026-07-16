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
