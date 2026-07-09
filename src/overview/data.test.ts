import { describe, it, expect } from 'vitest';
import type { SeriesPoint, BreakdownRow } from '../types';
import {
  seriesToDays,
  windowOf,
  pointsIn,
  bucketsFromPoints,
  dailyTableRows,
  projectTableRows,
  modelBars,
  catTotals,
  rangeToFilters,
} from './data';

function pt(over: Partial<SeriesPoint>): SeriesPoint {
  return {
    bucket: '2026-07-09',
    source: 'claude',
    inputTokens: 100,
    outputTokens: 50,
    cacheReadTokens: 200,
    cacheWriteTokens: 30,
    totalTokens: 380,
    reasoningTokens: null,
    cost: 0.5,
    requests: 2,
    convs: 1,
    ...over,
  };
}

const TODAY = new Date(2026, 6, 10); // 2026-07-10 local

describe('seriesToDays', () => {
  it('builds a trailing 365-day window ending today', () => {
    const days = seriesToDays([], TODAY);
    expect(days).toHaveLength(365);
    expect(days[364].iso).toBe('2026-07-10');
    expect(days[0].iso).toBe('2025-07-11');
    expect(days.every((d) => d.tokens === 0 && d.level === 0)).toBe(true);
  });
  it('fills byTool, cost, and quartile levels', () => {
    const pts = [
      pt({ bucket: '2026-07-09', source: 'claude', totalTokens: 100, cost: 0.1 }),
      pt({ bucket: '2026-07-09', source: 'codex', totalTokens: 300, cost: 0.2 }),
      pt({ bucket: '2026-07-08', source: 'claude', totalTokens: 1000, cost: 1 }),
    ];
    const days = seriesToDays(pts, TODAY);
    const d9 = days.find((d) => d.iso === '2026-07-09')!;
    expect(d9.tokens).toBe(400);
    expect(d9.byTool.claude).toBe(100);
    expect(d9.byTool.codex).toBe(300);
    expect(d9.cost).toBeCloseTo(0.3);
    const d8 = days.find((d) => d.iso === '2026-07-08')!;
    expect(d8.level).toBeGreaterThanOrEqual(d9.level);
    expect(d9.level).toBeGreaterThan(0);
  });
});

describe('windowOf + pointsIn', () => {
  const pts = [
    pt({ bucket: '2026-07-10' }),
    pt({ bucket: '2026-07-04' }),
    pt({ bucket: '2026-05-01' }),
  ];
  it('day = today only', () => {
    const win = windowOf('day', '', '', TODAY);
    expect(pointsIn(pts, win).map((p) => p.bucket)).toEqual(['2026-07-10']);
  });
  it('week = trailing 7 days', () => {
    const win = windowOf('week', '', '', TODAY);
    expect(pointsIn(pts, win)).toHaveLength(2);
  });
  it('total = everything', () => {
    expect(pointsIn(pts, windowOf('total', '', '', TODAY))).toHaveLength(3);
  });
  it('custom = inclusive bounds', () => {
    const win = windowOf('custom', '2026-05-01', '2026-07-04', TODAY);
    expect(pointsIn(pts, win)).toHaveLength(2);
  });
});

describe('bucketsFromPoints', () => {
  it('daily buckets keep per-tool splits', () => {
    const bks = bucketsFromPoints(
      [pt({ bucket: '2026-07-09', source: 'claude', totalTokens: 10 }),
       pt({ bucket: '2026-07-09', source: 'codex', totalTokens: 5 }),
       pt({ bucket: '2026-07-10', source: 'claude', totalTokens: 7 })],
      'day',
    );
    expect(bks).toHaveLength(2);
    expect(bks[0].byTool.claude).toBe(10);
    expect(bks[0].total).toBe(15);
  });
  it('hour buckets label by hour', () => {
    const bks = bucketsFromPoints([pt({ bucket: '2026-07-10 09:00' })], 'hour');
    expect(bks[0].label).toBe('9');
  });
});

describe('tables', () => {
  it('dailyTableRows keeps reasoning null when never reported', () => {
    const rows = dailyTableRows([
      pt({ bucket: '2026-07-09', reasoningTokens: null }),
      pt({ bucket: '2026-07-09', source: 'codex', reasoningTokens: 5 }),
      pt({ bucket: '2026-07-08', reasoningTokens: null }),
    ]);
    const d9 = rows.find((r) => r.label === '2026-07-09')!;
    expect(d9.reasoning).toBe(5);
    expect(d9.convs).toBe(2);
    const d8 = rows.find((r) => r.label === '2026-07-08')!;
    expect(d8.reasoning).toBeNull();
  });
  it('projectTableRows maps breakdown rows', () => {
    const row: BreakdownRow = {
      key: '/p/alpha', inputTokens: 1, outputTokens: 2, cacheReadTokens: 3,
      cacheWriteTokens: 4, totalTokens: 10, requests: 5, cost: null,
      source: null, reasoningTokens: null, convs: 2, cacheEstimated: false,
    };
    expect(projectTableRows([row])[0]).toEqual({
      label: '/p/alpha', total: 10, input: 1, output: 2, cached: 3, reasoning: null, convs: 2,
    });
  });
});

describe('modelBars + catTotals + rangeToFilters', () => {
  it('modelBars filters by source and carries the flag', () => {
    const rows: BreakdownRow[] = [
      { key: 'claude-opus-4-8', inputTokens: 10, outputTokens: 10, cacheReadTokens: 60,
        cacheWriteTokens: 20, totalTokens: 100, requests: 1, cost: 1.5,
        source: 'claude', reasoningTokens: null, convs: 1, cacheEstimated: true },
      { key: 'gpt-5.4', inputTokens: 1, outputTokens: 1, cacheReadTokens: 1,
        cacheWriteTokens: 1, totalTokens: 4, requests: 1, cost: null,
        source: 'codex', reasoningTokens: null, convs: 1, cacheEstimated: false },
    ];
    const bars = modelBars(rows, 'claude', 200);
    expect(bars).toHaveLength(1);
    expect(bars[0].share).toBeCloseTo(0.5);
    expect(bars[0].cacheEstimated).toBe(true);
    expect(bars[0].segs.map((s) => s.frac)).toEqual([0.1, 0.1, 0.6, 0.2]);
  });
  it('catTotals sums one tool', () => {
    const t = catTotals(
      [pt({ source: 'claude' }), pt({ source: 'codex', inputTokens: 999 })],
      'claude',
    );
    expect(t).toEqual({ input: 100, output: 50, cacheRead: 200, cacheWrite: 30 });
  });
  it('rangeToFilters maps presets through rangeToBounds', () => {
    expect(rangeToFilters('total', '', '')).toEqual({ tools: [], models: [], project: null });
    const f = rangeToFilters('custom', '2026-07-01', '2026-07-02');
    expect(f.startTs).toBeDefined();
    expect(f.endTs).toBeDefined();
  });
});
