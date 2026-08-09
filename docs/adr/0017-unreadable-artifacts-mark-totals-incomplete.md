# Unreadable Artifacts mark totals incomplete instead of warning

**Amended by ADR-0018**: "can never be parsed" is now "cannot be parsed by the
scan". An Artifact stops being unreadable, and stops marking totals, once a
Companion has written an Export Artifact the scan can read for it — but only
when that export actually parses, never merely because a file of the right
name sits beside it.

A present Source Artifact that can never be parsed passively — encrypted with
no published scheme, readable only by running the Source's own programs, which
ADR-0013 forbids — is an Unreadable Artifact: a third class beside ADR-0015's
missing path (a normal empty Source) and its malformed instance of a supported
shape (a Source-specific warning). It emits no warning, and ADR-0015's warning
rule does not apply: a warning requests action, none exists, and a request
repeated on every scan with no remedy is noise. But silence about the warning
is not silence about the numbers. A warning and a completeness marker are
different speech acts — the marker asks nothing and only qualifies a figure —
so the scan counts Unreadable Artifacts and keeps their latest mtime, and
every token total whose window their content could fall in is shown as a
floor with the same "≥" marker as Partial Cost, so a Source with unreadable
Sessions and a Source read in full never look alike. Content is never newer
than its file, and nothing bounds its age downward (a migration can rewrite
old Sessions as new files), so a window is definitely complete only when it
starts after every Unreadable Artifact's last write.
