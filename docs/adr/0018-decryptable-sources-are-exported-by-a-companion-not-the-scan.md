# Decryptable Sources are exported by a companion, never by the scan

Some Sources encrypt their Artifacts with a key only the Source holds, so the
content is unreadable offline but perfectly readable to a program the user is
already running. Antigravity's `.pb` Sessions are the case in hand: entropy
8.0 from byte zero, no key on disk, and the only thing that can decrypt them is
Antigravity's own language server. ADR-0013 keeps acquisition passive, and
ADR-0017 makes such Artifacts mark totals as floors rather than warn, so the
Ledger on its own can only ever report a `≥` for them.

Rather than relax ADR-0013, the decrypting step moves out of the Ledger
entirely. A companion binary — `antigravity-export` — is run by a person, asks
the already-running Source for the Sessions it can already read, and writes
`<session>.tokenledger.json` beside each `.pb`. The scan then treats those
files as ordinary Artifacts: it reads them, and nothing in the scan path ever
opens a socket. That narrows ADR-0013 rather than leaving it alone — its rule
binds the scan, and the companion is not the scan — so ADR-0013 and ADR-0017
each carry an amendment note saying so, instead of this decision quietly
reinterpreting them from outside. The split is also honest about consent:
decryption happens when someone asks for it, not silently on a timer.

An export stands in for its `.pb` only once the scan has *read* it, so a
Session with a usable export is no longer an Unreadable Artifact and no longer
forces the `≥` marker; a Session without one still does, and so does one whose
export fails to parse — judging by filename alone would let a corrupt export
silence the marker while contributing nothing, which is the one outcome the
marker exists to prevent. An export naming no generations is not a failure:
"this Session billed nothing" is an answer, and real installs hold such
Sessions. Export and database paths key events on the same
`source:session:responseId`, so a Session present as both contributes its
generations exactly once. Exports carry a `schema` field and an unrecognised
value warns instead of being parsed on optimism — a stale companion writing a
shape the adapter predates is a malformed instance of a supported shape
(ADR-0015), not a new Artifact class.

The companion discovers everything at run time — the Source's process for its
CSRF token, the OS for that process's listening ports — because a port pinned
at development time is wrong on the next machine, and usually on the next
launch of the same one. It writes each export by rename, so a scan running on
its timer reads the old file or the new one and never a half-written one.

It ships as a Tauri sidecar and is offered as a button beside the "≥" reason,
which is the only place the marker is explained and so the only place its
remedy belongs. Bundling it is what makes the rule true for someone who merely
installed the app rather than cloning the repo; a companion nobody has is the
same as no companion. Shipping it as a *separate executable* rather than
folding the code into the app is the point: the scan cannot reach it even by
accident, and the boundary stays something you can check rather than something
the code merely promises. Pressing the button rescans afterwards, because the
Artifacts the companion writes are not usage until a Scan has read them.
