# TokenLedger

The domain of TokenLedger: a desktop app — distributed to end users, one
install per machine — that reads the local logs of AI coding tools on that
machine and reports how many tokens were consumed and what that usage is
worth at public list prices. This glossary is the ubiquitous language — the
precise meaning of each domain term, independent of how the code implements it.

## Language

**Usage Record**:
The token usage attributed to one unit of billable work from a Source. The
"unit of work" is one API call/response for Claude, Codex, Gemini, and
Antigravity, one user Turn for Grok, but one whole Session for Hermes — so a
Usage Record is not synonymous with one API request. (Implemented as
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
usage trends, and per-Source breakdowns. Activity is the one part of the tab
that ignores that selection. What it shows is always a view of the Ledger; it
never holds usage data of its own. Usage data appears on no other tab.
_Avoid_: Dashboard, home screen

**Activity**:
The Overview's fixed-window view of the Ledger: token activity per calendar
day over the trailing 12 months, across all Sources, deliberately independent
of the Overview's date window and Source selection. Presented as a heatmap
card and, via its Enlarge control, as a full-screen rotatable 3D perspective;
every figure it reports — including its Cost — describes that same fixed
window, never the selected range.
_Avoid_: Heatmap, contribution graph, calendar

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
The application's resident presence in the system menu bar: an icon with
Today's token total and Cost beside it, and a menu presenting Today's view of
the Ledger — headline Cost, tokens, Requests, pace against yesterday, and
per-Source figures — plus the app actions. "Today" is the local calendar day;
the surface presents no other date window. On a day with no usage the bar
shows the icon alone. Every Cost figure follows the same rules as everywhere
else: Partial Cost's "≥" marker, Unpriced never shown as $0, Display Currency
honored.
_Avoid_: Tray, status item

### Sources and granularity

**Source**:
An AI tool whose local logs TokenLedger reads: Claude Code, Codex, Gemini CLI,
Hermes, Grok Build, or Google Antigravity (IDE and CLI conversations count as
the one Antigravity Source).
_Avoid_: Provider, tool, agent, integration

**Session**:
One continuous run of a Source's agent, comprising one or more Requests. Every
Source organises its logs into Sessions; Hermes is the one that stores usage at
Session granularity (one Usage Record per Session).
_Avoid_: Conversation, run, thread

**Request**:
One API call to a Model. The displayed **Requests** figure is the total number
of API calls — the count of Usage Records for Claude/Codex/Gemini/Antigravity
(one call each), but the summed `api_call_count` for Hermes (whose one Session
Record stands for many calls). Grok logs expose Turn boundaries only, so each
Grok Record counts as one Request even though a Turn spans several calls.
Requests is a sum of calls, never a row count.
_Avoid_: Call count, hits

**Project**:
The working directory a Usage Record was produced in, identified by its
absolute path so the same directory groups together across Sources. A git
worktree rolls up to its parent repository rather than appearing as its own
Project.
_Avoid_: Repo, workspace, directory

**Model**:
The specific model a Usage Record used, identified by its raw logged name (e.g.
`claude-opus-4-8`, `gpt-5.4`). The raw name is what is displayed and what a
price resolves against; name normalisation exists only for price matching, not
for display.
_Avoid_: Engine, LLM, variant

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

### Money

**Cost**:
The public list-price value of a set of tokens — an estimate of what the usage
would have cost at pay-as-you-go API rates. It is not money that was billed:
every Source here is subscription, free-tier, or self-hosted, so TokenLedger
never sees a real invoice. Surfaced in the UI as "Est. cost".
_Avoid_: Spend, actual cost, bill

**Display Currency**:
The currency Cost figures are rendered in. Every stored rate and Cost — List
Prices, Overrides, all catalog data — is denominated in USD; a user-supplied
fixed exchange rate converts figures at display time only. Nothing stored
ever leaves USD, so changing Display Currency rewrites no data.
_Avoid_: Local currency, FX conversion

**List Price**:
The public per-token rate for a Model, taken from a price catalog (a published
list of model prices). A Model may be covered by more than one catalog; see
ADR-0003 for how a single rate is chosen.
_Avoid_: Rate card, tariff

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
The Cost of a set of Usage Records that mixes priced and Unpriced Models: a sum
over only the priced tokens, shown with a "≥" marker and a count of the Unpriced
Models, so the figure is never mistaken for a complete total.
_Avoid_: Partial total, incomplete cost

**Cache-Estimated**:
The state of a Model that is priced for input and output but whose Cache tokens
have no rate, so its Cost is real yet excludes those counted-but-unpriced cache
tokens. A weaker gap than Unpriced: it is flagged per-Model (a cost marker) but,
unlike an Unpriced Model, does not turn the view's total into a "≥" Partial Cost.
_Avoid_: Cache-free, partial price
