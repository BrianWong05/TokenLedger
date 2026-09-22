# Claude Limits may be read from the Claude desktop app's usage history

Both of Claude's Limit channels need an interactive `claude` terminal session.
The Companion (ADR-0019) reads the CLI's stored sign-in, which the Claude
desktop app never writes: it hands its embedded Claude Code a token through
the environment and refreshes that token itself, and Claude Code skips its
credential store when such a token is present. The statusline tap fires only
when a terminal UI renders a status line, which the desktop app's headless
session never does. A person working only in the desktop app therefore saw a
Claude card reading Sign-in unavailable, and the card was right.

The Claude desktop app keeps its own usage history on disk —
`plan-usage-history.json` in its Electron user-data directory (macOS
`~/Library/Application Support/Claude` today; Electron's defaults name
`%APPDATA%\Claude` on Windows and `~/.config/Claude` on Linux, but a Source
Catalog entry lands only behind ADR-0012's validation gate, so those wait
until someone has seen the desktop app write them) — one entry per
poll of the vendor's organisation usage endpoint through the app's own
session, carrying the vendor's utilisation percentage per window under the
app's own short codes, the organisation id, and no reset instant. The scan
reads that file on its ordinary tick, exactly as ADR-0013 already allows for
an already-populated third-party cache: no credential, no network, no
Companion, no timer. It is the third channel for a Claude Limit Reading, beside
the Companion and the statusline tap, and the first that works with the
desktop app alone.

An entry becomes a Limit Reading only when the Ledger already knows the epoch
it belongs to — a stored Reading for the same window whose reset lies after
the entry and within one window length of it — and it takes that reset, so it
joins the stored series under the vendor's window key with `via = desktop` and
no account identity: the file's organisation id is not the account id the
Companion proves, and an unproven identity is recorded as unknown, never
asserted (ADR-0024). Such a Reading feeds the Limit Token Estimate exactly as
far as the evidence rules admit it, which today is not at all. An entry with no
known epoch is not a Reading and is not stored. The newest one is written as
current state beside the Companion's Export Artifact — a Limit State Artifact,
on the same footing as Codex's Usage Reset count in ADR-0019 — and the card
shows it as the current figure with its reset unknown. A reset nobody proved
is drawn as the mark `-`, decided once in the dictionary under
`limits.resetUnknown` and rendered from that one key by every surface, so the
Limits page and the tray panel never answer one window two ways. The mark is a
dash and not a digit: the slot it fills otherwise reads `Resets in 5d 2h`, and
a numeral there is read as a countdown that reached zero rather than as the
absence of one. The stored value stays absent: `-` is what the slot draws for
an unknown reset, never a reset instant the Ledger claims to know.

A file this version cannot read — an unknown `version`, a shape nobody has
mapped — is a malformed instance of a supported shape and reports as the
Source's own warning until a later pass reads it (ADR-0015), exactly as an
export the Companion wrote would. It is not an Unreadable Artifact in
ADR-0017's sense: it holds no Usage Records and marks no total incomplete. An
absent file is the ordinary absence of a Source — the desktop app is not
installed, or has never polled.

On the card the newest observation wins per window, whichever channel
produced it. A desktop figure older than its own window length is not drawn,
because the window has certainly reset since. A fresh desktop figure outranks
a stored epoch that has expired, which the page would otherwise draw as
unused. The card's one freshness line names the channel and the age of the
newest fact. When the Companion reports a dead sign-in while a desktop figure
exists, the bars still draw and the sign-in trouble shrinks to a note beneath
them. The plan the Source last reported stays on the card with them: a
subscription tier is not something the failed check disproves, and a card that
forgot its tier on every dead sign-in would flicker between naming the plan and
denying one. The redeemable Usage Reset count does not stay: it is a live
entitlement whose value the dead sign-in is precisely what fails to confirm, so
it goes to unknown rather than standing on an old answer. The five-hour window
is never projected forward from an earlier reset;
the rule that removed that forecast stands.

Deliberately not done: TokenLedger does not read the desktop app's own OAuth
token, which does answer the usage endpoint but lives under Electron
safeStorage with its key in another application's Keychain item — a credential
ADR-0013 forbids the app to handle. Nor does it run a headless `claude` on a
timer to keep the CLI's sign-in fresh: that would spend the reader's own quota
to measure it, break ADR-0019's third bound, and race Claude Code's single-use
refresh tokens — the race that empties the Keychain item in the first place
(TOKL-35). The file's cadence, roughly every fifteen minutes while the desktop
app is open, and its silence about reset instants are the vendor's, accepted
as they are.
