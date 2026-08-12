# Live Limits are fetched by a Companion, never by the app

Codex writes Limit state into the logs the scan already reads, while Claude's
lives only with its vendor. A current gauge for either Source means presenting
the Source CLI's stored OAuth token to its vendor. ADR-0013 forbids the app to
handle credentials or fetch private usage remotely — its only remote calls
today are unauthenticated public catalog fetches — so the fetch moves out of
the app entirely, into a Companion: run because a person asked, it reads the
credential document, asks the vendor, and writes an Export Artifact carrying
live Limit state. Window observations are ingested as Limit Readings; current
source-level state such as Codex's Usage Reset count is read from that same
Artifact without becoming Reading history.

This deliberately reaches further than ADR-0018's precedent, and says so
rather than pretending otherwise: `antigravity-export` asks an
already-running local process over localhost with a token discovered at run
time, while this Companion presents a *stored credential* to a *remote
server*. It is done anyway, on this route and no other, because the property
ADR-0018 prizes survives only here — the always-running process provably
never touches a vendor credential, checkable by grep rather than promised by
code review. Fetching in-process would spend ADR-0013's core prohibition to
save a process spawn.

Four bounds, all load-bearing:

1. It reads the credential document and never writes or refreshes it — never
   spends the refresh token. (tokscale rewrote `~/.claude/.credentials.json`,
   dropped fields, and left Claude Code reporting "Not logged in".)
2. It fetches Limit state only, never usage.
3. It runs only because a person asked — page open or manual refresh, with a
   floor between calls — never on a timer.
4. A 401/403 renders the card unavailable and points at the Source's own CLI;
   the Companion never tries to repair a session it does not own.

Bound 1 is narrowed — not repealed — for Google-family credentials by
ADR-0020: their access tokens die in about an hour, and the refresh-token
exchange provably cannot corrupt the Source's own session. Everything else
here stands.
