## Install

**macOS** (Apple Silicon) — download the `.dmg`, open it, drag TokenLedger to Applications.
It is not notarized yet, so the first launch shows "TokenLedger is damaged and can't be
opened": **right-click the app → Open → Open**. That path is Gatekeeper's deliberate
escape hatch, and it is only needed once. Notarization is planned.

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

## Claude 5-hour reset times (optional)

TokenLedger reads Claude's usage percentages from the Claude desktop app without any
setup. That file carries no reset instant, so the 5-hour row shows `-` where the other
rows count down. A live check can supply the reset, but the vendor rate-limits it.

The `claude-statusline-tap` shipped beside the app is the way around that. Claude Code
already knows its own reset times and hands them to whatever renders its status line,
so the tap reads them out of that pipe. It asks the vendor nothing and presents no
credential, which is why it keeps working when the live check is being refused.

Point Claude Code's status line at it in `~/.claude/settings.json`:

```json
"statusLine": { "type": "command", "command": "<path to the tap>" }
```

- **macOS** — `/Applications/TokenLedger.app/Contents/MacOS/claude-statusline-tap`
- **Windows** — `claude-statusline-tap.exe` beside `tokenledger.exe` in the install directory

Already have a status line? Pass your existing command as arguments and the tap runs it
untouched: `"<path to the tap> bunx -y ccstatusline@latest"`. With no arguments it draws
nothing, which is what you want if you had no status line to begin with.

Reset times arrive the next time a `claude` session renders its status line — a terminal
session, since the Claude desktop app's Code tab does not draw one.

> **Linux**: the tap ships inside the AppImage, whose contents are mounted at a temporary
> path only while the app runs. There is no stable path to point `statusLine` at, so this
> is macOS and Windows only for now.
