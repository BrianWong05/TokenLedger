// Both the live scan statuses (SourceStatus) and the persisted per-scan state
// (SourceUnreadable) carry these two fields; the rule reads either shape.
export interface UnreadableCounts {
  artifactsUnreadable: number;
  unreadableMaxMtime: number | null;
}

// A token total is a floor — shown with the same ≥ marker as Partial Cost —
// when a Source holds Unreadable Artifacts whose content could fall in the
// window (ADR-0017). Content is never newer than its file, so
// `mtime >= window start` is the test; nothing bounds the content's age
// downward (Antigravity's migration rewrote old Sessions as new files), so
// the window's end never clears a marker, an unknown mtime marks
// conservatively, and a null start means unbounded. Mirror of
// `tokens_are_floor` in src-tauri/src/readout.rs, pinned by
// readout-cases.json's floors rows.
export function unreadableSourcesIn<T extends UnreadableCounts>(
  sources: T[],
  windowStartSec: number | null,
): T[] {
  return sources.filter(
    (s) =>
      s.artifactsUnreadable > 0 &&
      (windowStartSec === null ||
        s.unreadableMaxMtime === null ||
        s.unreadableMaxMtime >= windowStartSec),
  );
}

// Both the live scan statuses and the persisted per-scan state carry these,
// so the rule below reads either shape.
export interface UnbookedCounts {
  requests: number;
  firstAt: number | null;
  lastAt: number | null;
}

// A figure is also a floor when a Source holds Unbooked Requests the window
// could contain (TOKL-25): Requests read and understood that booked no Usage
// Record, because the Source reported no tokens for them. This bounds the
// Requests figure and every token total alike — the Requests happened, so did
// the tokens they spent, and neither is in the Ledger.
//
// Unlike an Unreadable Artifact these carry their own timestamps, so there is a
// real span to test rather than only a file mtime: a window strictly outside it
// is left exact. A null bound is a row written before the span was recorded
// (schema v21) and marks conservatively; a null window edge is unbounded that
// way. Mirror of `unbooked_are_floor` in src-tauri/src/readout.rs, pinned by
// readout-cases.json's floors rows.
export function unbookedSourcesIn<T extends UnbookedCounts>(
  sources: T[],
  windowStartSec: number | null,
  windowEndSec: number | null,
): T[] {
  return sources.filter(
    (s) =>
      s.requests > 0 &&
      (windowStartSec === null || s.lastAt === null || s.lastAt >= windowStartSec) &&
      (windowEndSec === null || s.firstAt === null || s.firstAt <= windowEndSec),
  );
}
