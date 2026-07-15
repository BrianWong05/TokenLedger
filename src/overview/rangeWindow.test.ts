import { describe, it, expect, vi, afterEach } from 'vitest';
import { windowOf, rangeToFilters, isoOf } from './data';
import type { Window } from './data';
import type { Range8b } from './meta';
import { parseLocalDate } from '../lib/dateRange';
import type { Filters } from '../types';

// --- reference implementations: the three pre-refactor bodies, verbatim ---
// rangeWindow (data.ts) now unifies windowOf, rangeToFilters and the deleted
// rangeToBounds; this suite asserts the surviving exports still match the old
// bodies cell-for-cell across a DST / boundary date matrix. RangePreset,
// CustomRange and DateRange were removed from src/types.ts with rangeToBounds,
// so their old definitions are inlined here for the copies.
type RangePreset = 'today' | '7d' | '30d' | 'all';
interface CustomRange { start: string; end: string; }
type DateRange = RangePreset | CustomRange;

function midnight(d: Date): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate());
}
function secs(d: Date): number {
  return Math.floor(d.getTime() / 1000);
}
function refRangeToBounds(range: DateRange, now: Date = new Date()): { startTs?: number; endTs?: number } {
  if (typeof range === 'string') {
    switch (range) {
      case 'today':
        return { startTs: secs(midnight(now)) };
      case '7d': {
        const start = midnight(now);
        start.setDate(start.getDate() - 6);
        return { startTs: secs(start) };
      }
      case '30d': {
        const start = midnight(now);
        start.setDate(start.getDate() - 29);
        return { startTs: secs(start) };
      }
      case 'all':
        return {};
    }
  }
  const r = range as CustomRange;
  const start = parseLocalDate(r.start);
  const end = parseLocalDate(r.end);
  end.setDate(end.getDate() + 1); // inclusive end date -> exclusive next-day midnight
  return { startTs: secs(start), endTs: secs(end) };
}
function refWindowOf(range: Range8b, customFrom: string, customTo: string, today: Date = new Date()): Window {
  const end = new Date(today.getFullYear(), today.getMonth(), today.getDate());
  const iso = isoOf(end);
  const back = (n: number) => {
    const d = new Date(end);
    d.setDate(d.getDate() - n);
    return isoOf(d);
  };
  switch (range) {
    case 'day': return { fromIso: iso, toIso: iso };
    case 'week': return { fromIso: back(6), toIso: iso };
    case 'month': return { fromIso: back(29), toIso: iso };
    case 'total': return {};
    case 'custom': {
      const lo = customFrom <= customTo ? customFrom : customTo;
      const hi = customFrom <= customTo ? customTo : customFrom;
      return { fromIso: lo, toIso: hi };
    }
  }
}
function refRangeToFilters(range: Range8b, customFrom: string, customTo: string): Filters {
  if (range === 'custom') {
    const lo = customFrom <= customTo ? customFrom : customTo;
    const hi = customFrom <= customTo ? customTo : customFrom;
    return { tools: [], models: [], project: null, ...refRangeToBounds({ start: lo, end: hi }) };
  }
  const dr: DateRange =
    range === 'day' ? 'today' : range === 'week' ? '7d' : range === 'month' ? '30d' : 'all';
  return { tools: [], models: [], project: null, ...refRangeToBounds(dr) };
}

// today matrix: DST spring-forward, fall-back, month/year boundaries.
const DATES: [string, Date][] = [
  ['spring-forward', new Date(2026, 2, 8)],  // 2026-03-08 US DST +1h
  ['fall-back',      new Date(2025, 10, 1)], // 2025-11-01
  ['month-start',    new Date(2026, 2, 1)],  // 2026-03-01
  ['month-end',      new Date(2026, 6, 31)], // 2026-07-31
  ['year-start',     new Date(2026, 0, 1)],  // 2026-01-01
];
const PRESETS: Range8b[] = ['day', 'week', 'month', 'total'];
const CUSTOMS: [string, string, string][] = [
  ['normal',     '2026-02-10', '2026-03-15'],
  ['reversed',   '2026-03-15', '2026-02-10'],
  ['single-day', '2026-03-08', '2026-03-08'],
];

describe('rangeWindow equivalence (windowOf)', () => {
  for (const [label, today] of DATES) {
    for (const range of PRESETS) {
      it(`${range} @ ${label} matches old windowOf`, () => {
        expect(windowOf(range, '', '', today)).toEqual(refWindowOf(range, '', '', today));
      });
    }
    for (const [clabel, from, to] of CUSTOMS) {
      it(`custom ${clabel} @ ${label} matches old windowOf`, () => {
        expect(windowOf('custom', from, to, today)).toEqual(refWindowOf('custom', from, to, today));
      });
    }
  }
});

describe('rangeWindow equivalence (rangeToFilters)', () => {
  // rangeToFilters reads now via new Date(); fake the clock so both bodies see
  // the same instant without changing its (today-less) signature.
  afterEach(() => vi.useRealTimers());
  for (const [label, today] of DATES) {
    for (const range of PRESETS) {
      it(`${range} @ ${label} matches old rangeToFilters`, () => {
        vi.useFakeTimers();
        vi.setSystemTime(today);
        expect(rangeToFilters(range, '', '')).toEqual(refRangeToFilters(range, '', ''));
      });
    }
    for (const [clabel, from, to] of CUSTOMS) {
      it(`custom ${clabel} @ ${label} matches old rangeToFilters`, () => {
        vi.useFakeTimers();
        vi.setSystemTime(today);
        expect(rangeToFilters('custom', from, to)).toEqual(refRangeToFilters('custom', from, to));
      });
    }
  }
});
