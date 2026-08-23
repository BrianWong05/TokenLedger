import { describe, it, expect } from 'vitest';
import { tokenFloor, markedTokenFigure, NO_FLOOR } from './localize';

// The ≥ token floor as one shape (ADR-0017): tokenFloor answers whether a
// window's figure is a floor and what its hover reason says, the way
// isPartialCost + formatDisplayCost answer it for Cost. Before this seam the
// fact travelled as three encodings across six render sites (a Set, nullable
// strings, a record), each hand-concatenating its own '≥ '.

const unreadable = (source: string, count: number, mtime: number | null) => ({
  source,
  artifactsUnreadable: count,
  unreadableMaxMtime: mtime,
});

describe('tokenFloor', () => {
  it('marks a window an Unreadable Artifact could reach, naming the Source', () => {
    const floor = tokenFloor([unreadable('antigravity', 100, 1_000)], 1_000, 'en');
    expect(floor.marked).toBe(true);
    expect(floor.reason).toBe('Antigravity: 100 sessions unreadable');
  });

  it('does not mark a window starting after every unreadable write', () => {
    const floor = tokenFloor([unreadable('antigravity', 100, 999)], 1_000, 'en');
    expect(floor).toEqual({ marked: false, reason: '' });
  });

  it('an unknown mtime marks conservatively; a null start means unbounded', () => {
    expect(tokenFloor([unreadable('antigravity', 6, null)], 1_000, 'en').marked).toBe(true);
    expect(tokenFloor([unreadable('antigravity', 6, 5)], null, 'en').marked).toBe(true);
  });

  it('joins multiple Sources into one reason, skipping clean ones', () => {
    const floor = tokenFloor(
      [
        unreadable('antigravity', 2, null),
        unreadable('hermes', 0, null),
        unreadable('zed', 1, null),
      ],
      null,
      'en',
    );
    expect(floor.reason).toBe('Antigravity: 2 sessions unreadable · Zed: 1 session unreadable');
  });

  it('speaks the app language', () => {
    const floor = tokenFloor([unreadable('antigravity', 6, null)], null, 'zh-Hant');
    expect(floor.reason).toBe('Antigravity: 6 個工作階段無法讀取');
  });

  it('no Sources means no floor', () => {
    expect(tokenFloor([], null, 'en')).toEqual(NO_FLOOR);
  });
});

// TOKL-25's settled shape: Unbooked Requests qualify no figure. tokenFloor no
// longer accepts them at all — the type system is the guard here — so the one
// behavioural pin left is that the Overview states them in the scan footer
// without marking anything (Overview.test.tsx).

describe('markedTokenFigure', () => {
  it('prefixes a marked figure with ≥, whatever precision the surface used', () => {
    expect(markedTokenFigure('1.01M', { marked: true, reason: 'x' })).toBe('≥ 1.01M');
    expect(markedTokenFigure('964.2K', NO_FLOOR)).toBe('964.2K');
  });
});
