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
// `tokens_are_floor` in src-tauri/src/tray.rs; keep the two shapes in step.
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
