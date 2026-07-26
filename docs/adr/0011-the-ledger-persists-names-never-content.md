# The Ledger persists names, paths, and bounded signatures — never content

TokenLedger reads logs that contain the user's prompts, the model's responses,
thinking, images, tool arguments, and tool-result bodies. It reads all of that
to do its job: context attribution sizes each category by its bytes. The
question this decision settles is what may survive the scan into the Ledger.

The rule: **content is read transiently and never persisted.** What may persist
is limited to counts (tokens, calls, byte-derived estimates), identifiers
(Session, Model, dedup keys), filesystem paths the user already knows
(`events.project`, `events.source_file`), the *names* of things invoked
(`ctx_tools.name`, `ctx_resources.name` — tool, MCP server, skill, subagent),
and **bounded signatures** as defined below. Message text, tool-result bodies,
image data, and thinking never reach a table in any form, whole or excerpted.

A **bounded signature** is a derived string whose length and shape are fixed by
the classifier, not by the input: it cannot grow with the content it describes.
The one instance today is the Bash drill-down's `ctx_exec.cmd`, which
`exec_class::exec_cmd` reduces to exactly two words — the executable's basename
plus the first non-flag, non-assignment argument (`git commit`, `npm install`,
`cat /Users/you/clients/acme/prod.env`).

## Considered options

**The full command.** Maximum drill-down value and the obvious implementation.
Rejected: a shell command is user content — it carries paths, URLs, hostnames,
inline heredocs, and occasionally a secret passed as an argument. Persisting it
is persisting content by another name.

**The executable alone.** Perfectly safe and nearly useless: "you ran `git` 400
times" answers no question a user has, and collapses the distinction between
`git status` and `git push` that makes the facet worth showing.

**Two words.** The midpoint, chosen deliberately. It preserves the
subcommand-level distinction that carries the analytical value while bounding
what can leak to a single argument.

## Consequences

The second word is the exposure, and it is a real one: a first argument is
frequently a path, and a path is frequently descriptive (`cat
~/clients/acme/prod.env`). This is accepted, disclosed in the README's privacy
section rather than buried, and bounded by the fact that `ctx_exec` is
scan-state-derived — clearing scan state and rescanning rebuilds it, so a user
who objects has a remedy that does not cost them their Ledger.

Any new facet or drill-down is bound by this rule. Persisting an MCP call's
arguments, a fetched URL, a file's contents, or a prompt excerpt re-litigates
this decision and needs its own ADR — the test is whether the stored string's
size is fixed by the classifier or by the user's data. Adding a Source needs no
new machinery: emit counts, identifiers, paths, and names.
