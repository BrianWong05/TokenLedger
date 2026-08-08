import { describe, expect, it } from 'vitest';
import { unreadableSourcesIn } from './tokenCompleteness';

// Same cases as `tokens_are_floor_tests_mtime_against_window_start` in
// src-tauri/src/tray.rs — the two implementations must stay in step.
describe('unreadableSourcesIn', () => {
  const src = (artifactsUnreadable: number, unreadableMaxMtime: number | null) => ({
    source: 'antigravity',
    artifactsUnreadable,
    unreadableMaxMtime,
  });
  const marks = (s: ReturnType<typeof src>, start: number | null) =>
    unreadableSourcesIn([s], start).length > 0;

  it('tests mtime against the window start, marking conservatively', () => {
    expect(marks(src(100, 1_000), 1_000)).toBe(true);
    expect(marks(src(100, 1_001), 1_000)).toBe(true);
    expect(marks(src(100, 999), 1_000)).toBe(false);
    expect(marks(src(100, null), 1_000)).toBe(true); // unknown recency → conservative
    expect(marks(src(0, null), 0)).toBe(false);
    expect(unreadableSourcesIn([], 0)).toEqual([]);
  });

  it('an unbounded window is always marked when unreadables exist', () => {
    expect(marks(src(100, 999), null)).toBe(true);
  });
});
