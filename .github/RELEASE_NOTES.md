## Install

**macOS** (Apple Silicon) — download the `.dmg`, open it, drag TokenLedger to Applications.

**Windows** — download the `-setup.exe`. It is not code-signed yet, so SmartScreen
will show "Windows protected your PC": click **More info → Run anyway**. Signing is
planned; until then that click is the price of admission.

> TokenLedger reads the logs under your Windows home directory. Coding tools running
> **inside WSL** write to the Linux home instead, and those are not scanned yet — a
> WSL-only setup will show an empty Ledger.

**Linux** — download the `.AppImage`, `chmod +x` it, and run it.

> The tray needs `libayatana-appindicator3-1`, which is not bundled (Debian/Ubuntu:
> `sudo apt install libayatana-appindicator3-1`). On stock GNOME, tray icons also
> need the AppIndicator extension. Without a tray you can still open the window, but
> the app's resident presence is how you get back to it.

Every download here updates itself from then on — one install, then the app keeps
itself current.
