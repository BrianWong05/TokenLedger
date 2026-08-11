# TokenLedger

The domain of TokenLedger: a desktop app — distributed to end users, one
install per machine — that reads the local logs of AI coding tools on that
machine and reports how many tokens were consumed and what that usage is
worth at public list prices. This glossary is the ubiquitous language — the
precise meaning of each domain term, independent of how the code implements it.

## Language

**Usage Record**:
The token usage attributed to one unit of billable work from a Source. The
"unit of work" is one API call/response for Claude, Codex, Gemini,
Antigravity, and a pi assistant message; one reported auxiliary usage block for
a pi summary or tool result; one usage-ledger row for Goose; one user Turn for
Grok; but one whole Session for Hermes, Kilo, or Zed when their
Artifacts expose no trustworthy finer timestamps — with OpenCode splitting
a Session into one Record per Model its Requests prove, plus one Record
for any Requests whose Model is unproven. Failed or aborted work
counts when it reports non-zero usage, while a zero-token observation is not a
Usage Record. (Implemented as `UsageEvent`.)
_Avoid_: Event, row, entry

**Ledger**:
The permanent record of every Usage Record ever ingested — the system of
record, not a cache. Because Sources prune their logs (Claude Code deletes
transcripts after ~30 days), a Usage Record persists in the Ledger after its
source log is gone; scans only ever add Records, never delete them — except
to supersede a coarser Record with Records that the Source proves carry the
same usage, as OpenCode's per-Model split does.
_Avoid_: Cache, database, store

**Scan**:
One pass over the Source Artifacts on this machine, parsing what they hold into
Usage Records and adding them to the Ledger. A Scan only ever reads: it never
writes to a Source's files and never talks to a Source's servers. It happens on
launch, every few hours on a resident cadence so a hidden app keeps recording,
on the Overview's auto-refresh timer while that window is focused, and on
demand when a person presses Rescan. Only the auto-refresh timer is the
reader's to set, and it is the only one they can turn off — the resident
cadence is not theirs to stop, because a Ledger that recorded only while
someone watched would lose the logs its Sources prune. Turning the timer off
therefore stops this window re-reading, never the recording.
_Avoid_: Sync, import, fetch — each suggests usage arriving from somewhere
else; a Scan only reads what is already on this machine

**Overview**:
The application's home tab: the presentation of the Ledger over a
user-selected date window and Source selection — headline token total, Cost,
usage trends, and per-Source breakdowns. Activity and Profile are the two parts
of the tab that ignore that selection. What it shows is always a view of the
Ledger; it never holds usage data of its own. Its Source selection contains
only Sources represented in the Ledger, including Sources whose Artifacts have
since disappeared. Usage data appears on no other tab.
_Avoid_: Dashboard, home screen

**Activity**:
The Overview's fixed-window view of the Ledger: token activity per calendar
day over the trailing 12 months, across all Sources, deliberately independent
of the Overview's date window and Source selection. Presented as a heatmap
card and, via its Enlarge control, as a full-screen rotatable 3D perspective;
every figure it reports — including its Cost — describes that same fixed
window, never the selected range.
_Avoid_: Heatmap, contribution graph, calendar

**Profile**:
The Overview's portrait of the whole Ledger, deliberately unmoved by the
Overview's date window and Source selection: token volume over fixed trailing
windows, the Models holding the largest lifetime share, the Sessions of the last
30 days, and how long the Ledger has been accumulating. Its Model shares are
measured against every lifetime token, Unattributed Usage included, so they sum
to less than the whole and Unattributed itself is never one of the Models named.
It reports no Cost at all. Where it says usage began, it means the earliest
Usage Record in the Ledger — not the machine's first ever use of a Source, which
may predate anything TokenLedger ingested.
_Avoid_: Stats, summary, model breakdown

**Trend**:
The Overview's presentation of the Ledger as consumption over time within the
selected date window: tokens per bucket — an hour, day, week, or month, chosen
automatically to fit the window — stacked by Source. Its Enlarge presents the
same view full-screen with a date window of its own and, if wanted, an
explicitly chosen bucket size (daily, weekly, or monthly) in place of the
automatic fit — both independent of the Overview's and forgotten on close —
plus the window's own per-Model split beneath the footer figures, and an
inspector that always holds exactly one bucket: its rank in the window,
per-Model split, and its own exactly-computed Cost.
_Avoid_: Chart, graph, histogram

**Preset**:
A named shortcut in the Custom range picker that resolves to a date window when
clicked — distinct from the Range segments (Day, Week, Month, Total, Custom),
which are the app's fixed vocabulary and are not Presets. Four ship with the
app; a reader may configure up to four more in Settings, each either a rolling
window of N days ending today or a completed calendar period. A configured one
is theirs to manage in full: they add it, read the window it currently resolves
to, change a rolling one's day count in place, order the four against each
other, and remove any of them — the shipped four are none of those things, and
cannot be edited, reordered or removed. Order is part of what a reader
configures, not an accident of when they added each one: the picker lists
configured Presets in the order Settings shows them. A Preset is a way of
*asking* for a window, never a stored window itself: it is resolved afresh
against the Ledger's extent every time the picker opens, so one whose window
falls entirely before the first Usage Record is not offered at all — Settings
says as much on a Preset in that state rather than naming a window nothing can
pick. Named Preset in identifiers and in the UI; "shortcut" is long-standing
informal prose for the same thing and stays fine in comments.
_Avoid_: Saved range, bookmark, quick range — each names a stored window, which
a Preset is not

**Pricing**:
The tab that presents rates, never usage: every Model seen in the Ledger with
its resolved List Price, the catalog it came from, its Override if any, and
its pricing state (Unpriced or Cache-Estimated). The one place rates are
edited — selecting a Model in the Overview opens this same editor in place.
_Avoid_: Rate card, price list, models tab

**Menu Bar Extra**:
The application's resident presence in the system's status area — the one
name for that presence on every platform, however the platform presents it.
Its fullest form is an icon with Today's token total and Cost beside it, and
the panel that icon toggles, presenting a selected window of the Ledger —
headline Cost, tokens, Requests, pace against the window before it, Cost per
bucket across the window, per-Source and per-Model figures, Cache Hit Rate,
the costliest Project, and how long ago the last scan ran — plus the app
actions. Where the platform cannot show text beside the icon, Today's figures
move to the icon's hover text; where the platform delivers no icon clicks,
there is no panel and the icon instead carries a menu — a read-only Today
row plus the app actions — with the panel's read-out left to the Overview.
The bar (or hover) figures and the panel carry different windows on purpose:
the icon's figures are always Today, the local calendar day, while the panel
selects its own (Today, Yesterday, or the trailing 30 days) and everything it
shows describes that selection. A day with no usage reads "0 · $0.00" rather
than leaving the icon to stand alone.
Every figure follows the same rules as everywhere else: Partial Cost's
"≥" marker, the Unreadable Artifact "≥" on token figures, Unpriced never
shown as $0, a window with no usage at $0.00, Display Currency honored.
The platform's own facility that hosts it — a Linux system tray and the
AppIndicator library behind it, the Windows notification area — keeps its
native name, because that is a place, not this application's presence in it.
What is never called a tray is the Menu Bar Extra itself.
_Avoid_: Tray, status item, menu (ADR-0007 replaced the native menu with the
panel; the menu survives only where the platform delivers no icon clicks)

### Sources and granularity

**Source**:
An independently operated AI coding tool whose local Source Artifacts expose a
timestamp and non-zero token count. Its identity outlives branding changes;
alternate surfaces, storage formats, and model backends accessed through that
tool remain one Source rather than becoming Sources themselves.
_Avoid_: Provider, tool, agent, integration

**Source Artifact**:
A local file or database from which TokenLedger derives Usage Records. It may
be a Source's native record or an already-populated third-party cache, and one
Source may have many independently supported Artifacts; TokenLedger reads each
passively, and rejecting one Artifact never rejects the Source itself.
_Avoid_: Data source, import file

**Unreadable Artifact**:
A Source Artifact that is present on every scan yet cannot be parsed by the
scan, because reading it would mean running the Source's own programs, which
ADR-0013 forbids — an encrypted Session with no published scheme. Not a
malformed instance of a supported shape, so it emits no warning (there is no
action to request); it is counted instead, and every token total whose window
its content could fall in is shown as a floor with the same "≥" marker as
Partial Cost (ADR-0017), so a Source with unreadable Sessions and a Source
read in full never look alike. Content is never newer than its file, so only a
window starting after the Artifact's last write is definitely complete. It
stops being unreadable once an Export Artifact the scan can read stands in for
it (ADR-0018) — one whose export file fails to parse is still unreadable, or
its tokens would leave the total with no "≥" left to admit it.
_Avoid_: Blocked artifact, encrypted artifact, unsupported artifact, skipped
file

**Export Artifact**:
A passive Artifact a Companion writes so the scan can read usage a Source
keeps encrypted — for Antigravity, `<session>.tokenledger.json` beside the
`.pb` it stands in for. It carries a schema version, and one the Ledger does
not recognise is a malformed instance of a supported shape (a warning), not a
new Artifact class. The scan only ever reads it, so acquisition stays passive
(ADR-0018).
_Avoid_: Dump, cache, sync file, decrypted artifact

**Companion**:
A program shipped beside the Ledger but deliberately outside it, run because a
person asked, which may do the one thing the scan must not: talk to a Source.
It writes Export Artifacts and never writes the Ledger's database, so the
passive boundary (ADR-0013) stays something you can check rather than
something the code promises.
_Avoid_: Plugin, integration, sync agent, importer

**Source Capability**:
A kind of attribution a Source can truthfully expose, independent of whether a
particular Source Artifact happens to contain it. An unavailable Capability is
unknown, never a measured zero.
_Avoid_: Feature, field, support level

**Session**:
One continuous run of a Source's agent, comprising one or more Requests. Every
Source organises its logs into Sessions; Hermes is the one that stores usage at
Session granularity (one Usage Record per Session), while a branched pi Session
retains the Requests from every branch, including branches no longer active.
Copying pi history into a fork or clone creates no new Requests; the child
becomes a separate Session only when it produces its first new Request. Pi
usage without a reliable Session identity remains in the Ledger but contributes
to no Session count.
_Avoid_: Conversation, run, thread

**Request**:
One observable unit of model work, normally one API call. The displayed
**Requests** figure is exact for Claude, Codex, Gemini, Antigravity, pi assistant
messages, and Hermes (via its summed `api_call_count`); each Grok Turn and each
pi auxiliary usage block counts as one Request even when it may aggregate
several calls, making those contributions a documented lower bound. Requests
is a sum of source-observable calls or call groups, never a Ledger row count.
_Avoid_: Call count, hits

**Project**:
The working directory a Usage Record was produced in, identified by its
absolute path so the same directory groups together across Sources. A git
worktree rolls up to its parent repository rather than appearing as its own
Project, and a pi Request copied into a fork or clone keeps the Project where it
originally occurred. If pi reports no usable working directory, the Usage
Record has no Project rather than one inferred from a lossy path encoding.
_Avoid_: Repo, workspace, directory

**Model**:
The specific model a Usage Record used, identified by its raw logged name (e.g.
`claude-opus-4-8`, `gpt-5.4`). The raw name is what is displayed and what a
price resolves against; name normalisation exists only for price matching, not
for display. The one departure: when a Source logs an internal routing ALIAS
instead of a model name (Qoder's `qmodel_38max`), the parser translates it to
the Model it designates (`qwen3.8-max`) — an alias names no model, sits in no
catalog, and would price against nothing. Pi uses an assistant entry's backend
`responseModel` when present
and falls back to its selected `model`; a built-in pi summary Request inherits
the active Model from its parent branch, while an extension-provided summary
does not.
_Avoid_: Engine, LLM, variant

**Unattributed Usage**:
A Usage Record whose Source reports tokens but no reliable Model identity,
including pi tool-result usage, extension-provided summary usage, and Artifacts
that cannot correlate a Model to each unit of work. Its tokens and Request are
counted, but it contributes no Cost and is displayed separately rather than
assigned to a guessed Model; alone it has no Cost, while a mixed aggregate has
Partial Cost.
_Avoid_: Unknown Model, other Model

### Token categories

The four buckets that partition a Usage Record's tokens with no overlap.
Their defining property is mutual exclusivity: every token counted is in
exactly one bucket.

**Input Tokens**:
Fresh prompt tokens the model read that were not served from cache. Excludes
cache reads — this exclusion is what makes totals and Cache Hit Rate coherent
across Sources (Codex and Gemini report cached tokens inside input natively;
adapters subtract to honour this rule). Grok logs carry only an
undifferentiated running total, which is booked entirely as Input.
_Avoid_: Prompt tokens

**Output Tokens**:
Tokens the model generated, including reasoning/thinking tokens.
_Avoid_: Completion tokens, response tokens

**Cache Read Tokens**:
Prompt tokens served from a prior prompt cache rather than reprocessed.
_Avoid_: Cached tokens, cache hit tokens

**Cache Write Tokens**:
Prompt tokens written into the prompt cache for later reuse. Priced by
time-to-live: a 5-minute write and a 1-hour write cost different rates, so the
two TTLs are tracked separately for pricing but are the same category here.
_Avoid_: Cache creation tokens

**Cache Hit Rate**:
The fraction of prompt tokens served from cache:
Cache Read ÷ (Input + Cache Read + Cache Write). Well-defined only because
Input excludes cache reads (ADR-0001).
_Avoid_: Cache ratio, hit ratio

**Context**:
Where a Request's billed input came from — the same tokens the categories above
count by billing type (Input + Cache Read + Cache Write), attributed instead by
origin. Two tiers, and the difference between them is load-bearing: **messages,
system, and reasoning** partition the billed total exactly, while **tool calls,
subagents, MCP, and skills** are overlapping subsets of messages, estimated from
content size and never summing to a whole. Reasoning covers the current turn
alone, because the API strips it from later ones; system is estimated once, at a
Session's first Request. Only Claude, Codex, and pi report it — a Source that
cannot attribute a category yields no figure for it, displayed as "—" and never
as zero, and a Session resumed with its running state lost is *tainted*: it
attributes nothing thereafter rather than attributing a guess.
_Avoid_: Context window, breakdown, composition

**Resource**:
A named participant observed in a Session's Context — a skill, MCP server,
subagent, or memory file. Resources are recorded by name and counted. Context's
categories say *how much*, Resources say *which ones*, so a Resource displays
nothing rather than "—", the dash being reserved for a category a Source failed
to attribute. Two Resources are also weighed, each by a different mechanism. A
skill's instructions enter the Context as a block of their own, so each skill
carries the estimated tokens it loaded across the window and how many times it
loaded them — every invocation re-loads the whole body. An MCP server stamps its
name on every tool call it serves, so each server carries the estimated tokens of
that traffic and how many calls produced it; the tool definitions it publishes
are excluded, because they reach the model inside the system prompt, where
nothing marks whose they are. Nothing else carries such a mark, so nothing else
is weighed. A skill is named as it is invoked, which keeps a plugin's skill
distinct from a local skill sharing its name.
_Avoid_: Context item, roster

### Money

**Cost**:
The public list-price value of a set of tokens — an estimate of what the usage
would have cost at pay-as-you-go API rates. It is not money that was billed:
every Source here is subscription, free-tier, or self-hosted, so TokenLedger
never sees a real invoice. Surfaced in the UI as "Est. cost". A window holding
no tokens at all has a Cost of zero — $0.00 on every surface, the one zero that
is a figure rather than a gap, and the opposite of Unpriced.
_Avoid_: Spend, actual cost, bill

**Display Currency**:
The currency Cost figures are rendered in. Every stored rate and Cost — List
Prices, Overrides, all catalog data — is denominated in USD; a user-supplied
fixed exchange rate converts figures at display time only. Nothing stored
ever leaves USD, so changing Display Currency rewrites no data.
_Avoid_: Local currency, FX conversion

**List Price**:
The per-token rate set by the organisation that publishes a Model — Anthropic's
for Claude, Z.AI's for GLM. One Model may be served by dozens of hosts at rates
differing several-fold; only the publisher's is its List Price. A catalog is
where that rate is looked up, never what makes it authoritative. Where one
publisher sells the same Model through two surfaces at two rates — Google quotes
Gemini differently on its direct API and on Vertex AI — the List Price is the
one for the surface the Sources here actually use, which is the direct API. See
ADR-0009 for how a single rate is chosen.
_Avoid_: Rate card, tariff

**Routed Rate**:
A per-Model figure a catalog derives by routing across every host serving that
Model, and so moved by whichever hosts are discounting rather than by any
publisher's decision. Not a List Price: no organisation sets it, and a Cost
computed from one is correspondingly weaker.
_Avoid_: Market rate, blended price, list price

**Override**:
A user-supplied per-token rate for a Model that takes precedence over its List
Price. The mechanism for pricing self-hosted Models that no catalog covers.
_Avoid_: Manual price, custom rate

**Unpriced**:
The state of a Model that has neither an Override nor a matching List Price. Its
tokens are still counted, but it contributes no Cost and is surfaced as
"unpriced" — never as $0, so that a genuinely free Model and an unknown price
never look alike.
_Avoid_: Free, zero-cost, missing

**Partial Cost**:
The Cost of a set of Usage Records that mixes priced usage with Unpriced Models
or Unattributed Usage: a sum over only the priced tokens, shown with a "≥"
marker and the missing-pricing reasons, so the figure is never mistaken for a
complete total.
_Avoid_: Partial total, incomplete cost

**Cache-Estimated**:
The state of a Model that is priced for input and output but whose Cache tokens
have no rate, so its Cost is real yet excludes those counted-but-unpriced cache
tokens. A weaker gap than Unpriced: it is flagged per-Model (a cost marker) but,
unlike an Unpriced Model, does not turn the view's total into a "≥" Partial Cost.
_Avoid_: Cache-free, partial price
