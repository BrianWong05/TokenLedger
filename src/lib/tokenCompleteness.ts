import type { SourceStatus } from '../types';

// A token total is a floor — shown with the same ≥ marker as Partial Cost —
// when a Source holds Unreadable Artifacts whose content could fall in the
// window (ADR-0017). Content is never newer than its file, so
// `mtime >= window start` is the test; nothing bounds the content's age
// downward (Antigravity's migration rewrote old Sessions as new files), so
// the window's end never clears a marker. A null start means unbounded.
export function unreadableSourcesIn(
  sources: SourceStatus[],
  windowStartSec: number | null,
): SourceStatus[] {
  return sources.filter(
    (s) =>
      s.artifactsUnreadable > 0 &&
      (windowStartSec === null || (s.unreadableMaxMtime ?? 0) >= windowStartSec),
  );
}
