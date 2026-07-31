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
a pi summary or tool result; one user Turn for Grok; but one whole Session for
Hermes. Failed or aborted pi work counts when it reports non-zero usage, while
all-zero placeholders do not. (Implemented as
`UsageEvent`.)
_Avoid_: Event, row, entry

**Ledger**:
The permanent record of every Usage Record ever ingested — the system of
record, not a cache. Because Sources prune their logs (Claude Code deletes
transcripts after ~30 days), a Usage Record persists in the Ledger after its
source log is gone; scans only ever add Records, never delete them.
_Avoid_: Cache, database, store

**Overview**:
The application's home tab: the presentation of the Ledger over a
user-selected date window and Source selection — headline token total, Cost,
usage trends, and per-Source breakdowns. Activity and Profile are the two parts
of the tab that ignore that selection. What it shows is always a view of the
Ledger; it never holds usage data of its own. Usage data appears on no other tab.
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
plus an inspector that always holds exactly one bucket: its rank in the
window, per-Model split, and its own exactly-computed Cost.
_Avoid_: Chart, graph, histogram

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
Every Cost figure follows the same rules as everywhere else: Partial Cost's
"≥" marker, Unpriced never shown as $0, a window with no usage at $0.00,
Display Currency honored.
The platform's own facility that hosts it — a Linux system tray and the
AppIndicator library behind it, the Windows notification area — keeps its
native name, because that is a place, not this application's presence in it.
What is never called a tray is the Menu Bar Extra itself.
_Avoid_: Tray, status item, menu (ADR-0007 replaced the native menu with the
panel; the menu survives only where the platform delivers no icon clicks)

### Sources and granularity

**Source**:
An AI tool whose local logs TokenLedger reads: Claude Code, Codex, Gemini CLI,
Hermes, Grok Build, Google Antigravity (IDE and CLI conversations count as the
one Antigravity Source), or pi.
_Avoid_: Provider, tool, agent, integration

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
for display. Pi uses an assistant entry's backend `responseModel` when present
and falls back to its selected `model`; a built-in pi summary Request inherits
the active Model from its parent branch, while an extension-provided summary
does not.
_Avoid_: Engine, LLM, variant

**Unattributed Usage**:
A Usage Record whose Source reports tokens but no Model identity, including pi
tool-result usage and extension-provided summary usage. Its tokens and Request
are counted, but it contributes no Cost and is displayed separately rather than
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
