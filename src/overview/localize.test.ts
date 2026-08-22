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

const unbooked = (source: string, requests: number, firstAt: number | null, lastAt: number | null) => ({
  source,
  requests,
  firstAt,
  lastAt,
});

describe('tokenFloor', () => {
  it('marks a window an Unreadable Artifact could reach, naming the Source', () => {
    const floor = tokenFloor([unreadable('antigravity', 100, 1_000)], 1_000, 'en', [], null);
    expect(floor.marked).toBe(true);
    expect(floor.reason).toBe('Antigravity: 100 sessions unreadable');
  });

  it('does not mark a window starting after every unreadable write', () => {
    const floor = tokenFloor([unreadable('antigravity', 100, 999)], 1_000, 'en', [], null);
    expect(floor).toEqual({ marked: false, reason: '' });
  });

  it('an unknown mtime marks conservatively; a null start means unbounded', () => {
    expect(tokenFloor([unreadable('antigravity', 6, null)], 1_000, 'en', [], null).marked).toBe(true);
    expect(tokenFloor([unreadable('antigravity', 6, 5)], null, 'en', [], null).marked).toBe(true);
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
      [],
      null,
    );
    expect(floor.reason).toBe('Antigravity: 2 sessions unreadable · Zed: 1 session unreadable');
  });

  it('speaks the app language', () => {
    const floor = tokenFloor([unreadable('antigravity', 6, null)], null, 'zh-Hant', [], null);
    expect(floor.reason).toBe('Antigravity: 6 個工作階段無法讀取');
  });

  it('no Sources means no floor', () => {
    expect(tokenFloor([], null, 'en', [], null)).toEqual(NO_FLOOR);
  });
});

// TOKL-25: the second cause of the same floor. An Unbooked Request carries its
// own second, so unlike an Unreadable Artifact it can be ruled OUT of a window
// — which is the whole reason the span is stored rather than just a count.
describe('tokenFloor with Unbooked Requests', () => {
  it('marks a window the Requests fall in, naming the Source and the count', () => {
    const floor = tokenFloor([], 1_000, 'en', [unbooked('qoder', 628, 500, 1_500)], null);
    expect(floor.marked).toBe(true);
    expect(floor.reason).toBe('Qoder: 628 Requests report no tokens — not booked');
  });

  it('leaves a window that ends before the first Request exact', () => {
    // The bound an Unreadable Artifact cannot offer: content is never dated
    // downward, a Request is.
    expect(tokenFloor([], 100, 'en', [unbooked('qoder', 628, 5_000, 9_000)], 999)).toEqual(NO_FLOOR);
  });

  it('leaves a window that starts after the last Request exact', () => {
    expect(tokenFloor([], 10_000, 'en', [unbooked('qoder', 628, 500, 1_500)], null)).toEqual(NO_FLOOR);
  });

  it('a row with no recorded span marks conservatively', () => {
    // Written before schema v21: it cannot say when, so it does not pretend.
    expect(tokenFloor([], 10_000, 'en', [unbooked('qoder', 628, null, null)], 20_000).marked).toBe(true);
  });

  it('a zero count is never a floor', () => {
    expect(tokenFloor([], null, 'en', [unbooked('qoder', 0, 1, 2)], null)).toEqual(NO_FLOOR);
  });

  it('joins both causes into one reason', () => {
    const floor = tokenFloor(
      [unreadable('antigravity', 100, null)],
      null,
      'en',
      [unbooked('qoder', 628, 500, 1_500)],
      null,
    );
    expect(floor.reason).toBe(
      'Antigravity: 100 sessions unreadable · Qoder: 628 Requests report no tokens — not booked',
    );
  });

  it('speaks the app language', () => {
    const floor = tokenFloor([], null, 'zh-Hant', [unbooked('qoder', 628, null, null)], null);
    expect(floor.reason).toBe('Qoder: 628 個要求未回報 token — 未計入');
  });
});

describe('markedTokenFigure', () => {
  it('prefixes a marked figure with ≥, whatever precision the surface used', () => {
    expect(markedTokenFigure('1.01M', { marked: true, reason: 'x' })).toBe('≥ 1.01M');
    expect(markedTokenFigure('964.2K', NO_FLOOR)).toBe('964.2K');
  });
});
