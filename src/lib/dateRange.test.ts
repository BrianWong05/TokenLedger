import { describe, it, expect } from 'vitest';
import { rangeToBounds } from './dateRange';

// Fixed "now": 2026-07-07 14:30 local time.
const NOW = new Date(2026, 6, 7, 14, 30, 0);
const midnightSecs = (y: number, m: number, d: number) =>
  Math.floor(new Date(y, m, d).getTime() / 1000);

describe('rangeToBounds', () => {
  it('today: local midnight start, no end', () => {
    const r = rangeToBounds('today', NOW);
    expect(r.startTs).toBe(midnightSecs(2026, 6, 7));
    expect(r.endTs).toBeUndefined();
  });

  it('7d: midnight six days back, no end', () => {
    const r = rangeToBounds('7d', NOW);
    expect(r.startTs).toBe(midnightSecs(2026, 6, 1));
    expect(r.endTs).toBeUndefined();
  });

  it('30d: midnight 29 days back, no end', () => {
    const r = rangeToBounds('30d', NOW);
    expect(r.startTs).toBe(midnightSecs(2026, 5, 8)); // 2026-06-08
    expect(r.endTs).toBeUndefined();
  });

  it('all: both bounds undefined', () => {
    expect(rangeToBounds('all', NOW)).toEqual({});
  });

  it('custom: inclusive end becomes exclusive next-day midnight', () => {
    const r = rangeToBounds({ start: '2026-07-01', end: '2026-07-03' }, NOW);
    expect(r.startTs).toBe(midnightSecs(2026, 6, 1));
    expect(r.endTs).toBe(midnightSecs(2026, 6, 4)); // 07-03 + 1 day, exclusive
  });
});
