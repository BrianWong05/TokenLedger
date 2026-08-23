import { describe, it, expect } from 'vitest';
import type { SeriesPoint, BreakdownRow, CtxResource, CtxToolRow, CtxSkillRow, CtxExecRow } from '../types';
import { seriesPoint } from './seriesPoint';
import type { Bucket, PresetSlot, PresetSlots } from './data';
import { emptyBySource } from './meta';
import {
  effectiveBounds,
  windowModelSplit,
  splitRows,
  peakOf,
  bucketStats,
  seriesToDays,
  heatStats,
  heatFilters,
  windowOf,
  granularityOf,
  hourlyDayOf,
  pointsIn,
  bucketsFromPoints,
  smallMultiples,
  rankModels,
  modelTools,
  calendarSpan,
  presetsOf,
  presetWindow,
  monthCells,
  dailyTotals,
  dailyTableRows,
  projectTableRows,
  modelBars,
  catTotals,
  ctxMeta,
  rangeToFilters,
  bucketFilters,
  bucketCsv,
  csvFilename,
  categorizeTool,
  allocateByWeight,
  toolTree,
  skillBars,
  mcpBars,
  bucketView,
  execFacets,
  profileView,
} from './data';

function pt(over: Partial<SeriesPoint>): SeriesPoint {
  return seriesPoint({
    bucket: '2026-07-09',
    inputTokens: 100,
    outputTokens: 50,
    cacheReadTokens: 200,
    cacheWriteTokens: 30,
    totalTokens: 380,
    cost: 0.5,
    requests: 2,
    convs: 1,
    ...over,
  });
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
  it('fills byTool and rank-based levels', () => {
    const pts = [
      pt({ bucket: '2026-07-09', source: 'claude', totalTokens: 100 }),
      pt({ bucket: '2026-07-09', source: 'codex', totalTokens: 300 }),
      pt({ bucket: '2026-07-08', source: 'claude', totalTokens: 1000 }),
    ];
    const days = seriesToDays(pts, TODAY);
    const d9 = days.find((d) => d.iso === '2026-07-09')!;
    expect(d9.tokens).toBe(400);
    expect(d9.byTool.claude).toBe(100);
    expect(d9.byTool.codex).toBe(300);
    const d8 = days.find((d) => d.iso === '2026-07-08')!;
    expect(d8.level).toBe(4);
    expect(d9.level).toBeGreaterThan(0);
    expect(d8.level).toBeGreaterThan(d9.level);
  });
  it('busiest day is always brightest, even with a short all-equal history', () => {
    const pts = [
      pt({ bucket: '2026-07-08', totalTokens: 500 }),
      pt({ bucket: '2026-07-09', totalTokens: 500 }),
      pt({ bucket: '2026-07-10', totalTokens: 500 }),
    ];
    const days = seriesToDays(pts, TODAY);
    const active = days.filter((d) => d.tokens > 0);
    expect(active).toHaveLength(3);
    expect(active.every((d) => d.level === 4)).toBe(true);
  });
  it('retains a source that was not yet catalogued', () => {
    const day = seriesToDays([pt({ source: 'future-source', totalTokens: 12 })], TODAY)
      .find((item) => item.iso === '2026-07-09')!;
    expect(day.byTool['future-source']).toBe(12);
  });
});

describe('heatStats', () => {
  it('sums tokens, counts active days, longest streak, and picks the peak', () => {
    const pts = [
      pt({ bucket: '2026-07-06', totalTokens: 100 }),
      pt({ bucket: '2026-07-07', totalTokens: 900 }), // peak
      pt({ bucket: '2026-07-08', totalTokens: 200 }),
      // gap on 07-09
      pt({ bucket: '2026-07-10', totalTokens: 300 }),
    ];
    const s = heatStats(seriesToDays(pts, TODAY));
    expect(s.totalTokens).toBe(1500);
    expect(s.activeDays).toBe(4);
    expect(s.streak).toBe(3); // 07-06..07-08, broken by the 07-09 gap
    expect(s.bestDay.iso).toBe('2026-07-07');
  });
});

describe('heatFilters', () => {
  it('bounds the trailing-365-day window in epoch seconds, unfiltered otherwise', () => {
    const f = heatFilters(TODAY);
    // TODAY is 2026-07-10 local; the window's days match seriesToDays:
    // first day 2025-07-11, exclusive end at midnight 2026-07-11.
    expect(f.startTs).toBe(Math.floor(new Date(2025, 6, 11).getTime() / 1000));
    expect(f.endTs).toBe(Math.floor(new Date(2026, 6, 11).getTime() / 1000));
    expect(f.tools).toEqual([]);
    expect(f.models).toEqual([]);
    expect(f.project).toBeNull();
  });
});

describe('bucketFilters', () => {
  const secs = (d: Date) => Math.floor(d.getTime() / 1000);
  it('bounds an hour bucket to [hour, +1h)', () => {
    const f = bucketFilters('2026-07-15 09:00', 'hour');
    expect(f.startTs).toBe(secs(new Date(2026, 6, 15, 9)));
    expect(f.endTs).toBe(secs(new Date(2026, 6, 15, 10)));
  });
  it('rolls the last hour of the day over to the next midnight', () => {
    const f = bucketFilters('2026-07-15 23:00', 'hour');
    expect(f.startTs).toBe(secs(new Date(2026, 6, 15, 23)));
    expect(f.endTs).toBe(secs(new Date(2026, 6, 16, 0)));
  });
  it('bounds a day bucket to [midnight, +1 day)', () => {
    const f = bucketFilters('2026-07-15', 'day');
    expect(f.startTs).toBe(secs(new Date(2026, 6, 15)));
    expect(f.endTs).toBe(secs(new Date(2026, 6, 16)));
  });
  it('bounds a week bucket to [start day, +7 days)', () => {
    const f = bucketFilters('2026-07-12', 'week');
    expect(f.startTs).toBe(secs(new Date(2026, 6, 12)));
    expect(f.endTs).toBe(secs(new Date(2026, 6, 19)));
  });
  it('bounds a month bucket to its calendar month, rolling the year at December', () => {
    const jul = bucketFilters('2026-07', 'month');
    expect(jul.startTs).toBe(secs(new Date(2026, 6, 1)));
    expect(jul.endTs).toBe(secs(new Date(2026, 7, 1)));
    const dec = bucketFilters('2026-12', 'month');
    expect(dec.startTs).toBe(secs(new Date(2026, 11, 1)));
    expect(dec.endTs).toBe(secs(new Date(2027, 0, 1)));
  });
  it('carries the empty tool/model/project filter like the window helpers', () => {
    const f = bucketFilters('2026-07-15', 'day');
    expect(f.tools).toEqual([]);
    expect(f.models).toEqual([]);
    expect(f.project).toBeNull();
  });
});

describe('csvFilename', () => {
  it('derives a filesystem-safe name from the bucket key', () => {
    expect(csvFilename('2026-07-15')).toBe('usage-2026-07-15.csv'); // day/week
    expect(csvFilename('2026-07-15 09:00')).toBe('usage-2026-07-15-09-00.csv'); // hour
    expect(csvFilename('2026-07')).toBe('usage-2026-07.csv'); // month
  });
});

describe('bucketCsv', () => {
  const bucket = {
    key: '2026-07-15',
    label: '15',
    byTool: emptyBySource(),
    byModel: { 'claude-opus-4-8': 300, 'gpt-5.5-codex': 100 },
    unattributedTokens: 0, hasUnpriced: false, total: 400,
  };
  const modelTool = { 'claude-opus-4-8': 'claude', 'gpt-5.5-codex': 'codex' };

  it('emits a header and one row per model, largest first, with tool + share', () => {
    expect(bucketCsv(bucket, modelTool)).toBe(
      'model,tool,tokens,share\n' +
        'claude-opus-4-8,Claude,300,0.7500\n' +
        'gpt-5.5-codex,Codex,100,0.2500\n',
    );
  });

  it('skips zero-token models and includes every nonzero one (no top-N cap)', () => {
    const many = {
      key: '2026-07-15', label: '15', byTool: emptyBySource(),
      byModel: { a: 7, b: 6, c: 5, d: 4, e: 3, f: 2, g: 1, h: 0 },
      unattributedTokens: 0, hasUnpriced: false, total: 28,
    };
    const rows = bucketCsv(many, {}).trim().split('\n');
    expect(rows).toHaveLength(1 + 7); // header + 7 nonzero models, h dropped
    expect(rows[1].startsWith('a,')).toBe(true);
    expect(rows.some((r) => r.startsWith('h,'))).toBe(false);
  });

  it('CSV-escapes a field containing a comma and blanks an unknown tool', () => {
    const b = { key: 'k', label: 'k', byTool: emptyBySource(), byModel: { 'weird,name': 50 }, unattributedTokens: 0, hasUnpriced: false, total: 50 };
    expect(bucketCsv(b, {})).toBe('model,tool,tokens,share\n"weird,name",,50,1.0000\n');
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

describe('granularityOf', () => {
  // The presets carry no case of their own, so their spans are pinned here:
  // Day 1 -> hour, Week 7 and Month 30 -> day.
  it('steps hour -> day -> week -> month at the span boundaries', () => {
    expect([1, 2, 7, 30, 31].map(granularityOf)).toEqual(['hour', 'day', 'day', 'day', 'day']);
    expect([32, 120].map(granularityOf)).toEqual(['week', 'week']);
    expect(granularityOf(121)).toBe('month');
  });
});

describe('hourlyDayOf', () => {
  it('names the day for the Day preset and any one-day custom range, null once it widens', () => {
    const now = new Date(2026, 6, 16);
    const first = '2026-01-01';
    const last = '2026-07-16';
    expect(hourlyDayOf('day', '', '', first, last, now)).toBe('2026-07-16');
    expect(hourlyDayOf('custom', '2026-07-10', '2026-07-10', first, last, now)).toBe('2026-07-10');
    expect(hourlyDayOf('custom', '2026-07-09', '2026-07-10', first, last, now)).toBeNull();
    expect(hourlyDayOf('week', '', '', first, last, now)).toBeNull();
  });
});

describe('bucketsFromPoints', () => {
  it('keeps an unknown Ledger source in the bucket total and small multiples', () => {
    const bucket = bucketsFromPoints([pt({ source: 'future-source', totalTokens: 12 })], 'day')[0];
    expect(bucket.byTool['future-source']).toBe(12);
    expect(bucket.total).toBe(12);
    expect(smallMultiples([bucket])).toEqual([
      expect.objectContaining({ key: 'future-source', label: 'future-source', total: 12 }),
    ]);
  });

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
  it('zero-fills idle days across the window', () => {
    // Usage on 2 of 7 days: the trend must still show 7 buckets so gaps are
    // visible and avg-per-day divides by the calendar length.
    const bks = bucketsFromPoints(
      [pt({ bucket: '2026-07-04', totalTokens: 10 }), pt({ bucket: '2026-07-08', totalTokens: 5 })],
      'day',
      '2026-07-04',
      '2026-07-10',
    );
    expect(bks).toHaveLength(7);
    expect(bks.map((b) => b.total)).toEqual([10, 0, 0, 0, 5, 0, 0]);
    expect(bks[0].label).toBe('4');
  });
  it('zero-fills all 24 hours of a day', () => {
    const bks = bucketsFromPoints(
      [pt({ bucket: '2026-07-10 09:00', totalTokens: 7 })],
      'hour',
      '2026-07-10',
      '2026-07-10',
    );
    expect(bks).toHaveLength(24);
    expect(bks[9].total).toBe(7);
    expect(bks[0].label).toBe('0');
  });
  it('merges byModel across sources and keeps the bucket key', () => {
    const bks = bucketsFromPoints(
      [pt({ bucket: '2026-07-09', source: 'claude', byModel: { 'claude-fable-5': 300, 'claude-opus-4-8': 80 } }),
       pt({ bucket: '2026-07-09', source: 'codex', byModel: { 'gpt-5.6-sol': 50 } })],
      'day',
    );
    expect(bks[0].key).toBe('2026-07-09');
    expect(bks[0].byModel).toEqual({ 'claude-fable-5': 300, 'claude-opus-4-8': 80, 'gpt-5.6-sol': 50 });
  });

  it('keeps Unattributed Usage outside Model maps while carrying both Cost gaps', () => {
    const [bucket] = bucketsFromPoints(
      [
        pt({
          bucket: '2026-07-09', totalTokens: 350,
          byModel: { 'claude-opus-4-8': 300 },
          unattributedTokens: 50, hasUnpriced: true,
        }),
      ],
      'day',
    );

    expect(bucket.byModel).toEqual({ 'claude-opus-4-8': 300 });
    expect(bucket.unattributedTokens).toBe(50);
    expect(bucket.hasUnpriced).toBe(true);
  });
});

describe('smallMultiples', () => {
  it('omits tools with zero usage in the period', () => {
    const bks = bucketsFromPoints(
      [pt({ bucket: '2026-07-09', source: 'claude', totalTokens: 10 }),
       pt({ bucket: '2026-07-09', source: 'codex', totalTokens: 5 })],
      'day',
    );
    const items = smallMultiples(bks);
    expect(items.map((it) => it.key)).toEqual(['claude', 'codex']);
  });
});

describe('rankModels', () => {
  it('orders models by window total, descending', () => {
    const bks = bucketsFromPoints(
      [pt({ bucket: '2026-07-09', byModel: { a: 10, b: 300 } }),
       pt({ bucket: '2026-07-10', byModel: { a: 500, c: 40 } })],
      'day',
    );
    expect(rankModels(bks)).toEqual(['a', 'b', 'c']);
  });
});

describe('modelTools', () => {
  it('maps each model to its source', () => {
    const map = modelTools([
      pt({ source: 'claude', byModel: { 'claude-fable-5': 300 } }),
      pt({ source: 'codex', byModel: { 'gpt-5.6-sol': 50 } }),
    ]);
    expect(map).toEqual({ 'claude-fable-5': 'claude', 'gpt-5.6-sol': 'codex' });
  });
});

describe('calendarSpan', () => {
  it('counts inclusive calendar days', () => {
    expect(calendarSpan('2026-07-10', '2026-07-10')).toBe(1);
    expect(calendarSpan('2026-07-04', '2026-07-10')).toBe(7);
    expect(calendarSpan('2026-01-01', '2026-12-31')).toBe(365);
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
      hasUnpriced: true, unattributedTokens: 0,
    };
    expect(projectTableRows([row])[0]).toEqual({
      label: '/p/alpha', total: 10, input: 1, output: 2, cached: 3, reasoning: null, convs: 2,
    });
  });
  it('projectTableRows folds subdir rows into the ancestor project', () => {
    const base: BreakdownRow = {
      key: '/p/easydrone', inputTokens: 1, outputTokens: 2, cacheReadTokens: 3,
      cacheWriteTokens: 0, totalTokens: 6, requests: 1, cost: null,
      source: null, reasoningTokens: null, convs: 1, cacheEstimated: false,
      hasUnpriced: true, unattributedTokens: 0,
    };
    const rows = projectTableRows([
      base,
      { ...base, key: '/p/easydrone/client', reasoningTokens: 7 },
      { ...base, key: '/p/easydrone/server' },
      { ...base, key: '/p/easydrone-v2' }, // sibling with shared prefix — not a subdir
      { ...base, key: '/p/easydrone/deep/nested' }, // grandchild — direct parent only, no fold
    ]);
    expect(rows.map((r) => r.label).sort()).toEqual([
      '/p/easydrone', '/p/easydrone-v2', '/p/easydrone/deep/nested',
    ]);
    const drone = rows.find((r) => r.label === '/p/easydrone')!;
    expect(drone.total).toBe(18);
    expect(drone.convs).toBe(3);
    expect(drone.reasoning).toBe(7);
  });
});

describe('modelBars + catTotals + rangeToFilters', () => {
  it('modelBars filters by source and carries the flag', () => {
    const rows: BreakdownRow[] = [
      { key: 'claude-opus-4-8', inputTokens: 10, outputTokens: 10, cacheReadTokens: 60,
        cacheWriteTokens: 20, totalTokens: 100, requests: 1, cost: 1.5,
        source: 'claude', reasoningTokens: null, convs: 1, cacheEstimated: true,
        hasUnpriced: false, unattributedTokens: 0 },
      { key: 'gpt-5.4', inputTokens: 1, outputTokens: 1, cacheReadTokens: 1,
        cacheWriteTokens: 1, totalTokens: 4, requests: 1, cost: null,
        source: 'codex', reasoningTokens: null, convs: 1, cacheEstimated: false,
        hasUnpriced: true, unattributedTokens: 0 },
    ];
    const bars = modelBars(rows, 'claude', 200);
    expect(bars).toHaveLength(1);
    expect(bars[0].share).toBeCloseTo(0.5);
    expect(bars[0].cacheEstimated).toBe(true);
    expect(bars[0].segs.map((s) => s.frac)).toEqual([0.1, 0.1, 0.6, 0.2]);
    // The bar carries the row it was built from — the Model detail dialog reads
    // its figures from there, so a substituted or stale row would misreport
    // every bucket. Identity, not a copy: nothing may re-derive these.
    expect(bars[0].row).toBe(rows[0]);
  });

  // The Model rows and the tool total reach modelBars from independently timed
  // sources: rows from the store's last fetched window, the total from a
  // render-clock slice of the series. Between a day rolling over (or a range
  // switch) and the refetch landing they describe DIFFERENT windows, and a
  // clamped denominator turned that into shares of billions of percent on a
  // real screen. A share nothing can measure is unknown, not a number.
  it('modelBars reports no share when the total cannot measure the row', () => {
    const row = (totalTokens: number): BreakdownRow => ({
      key: 'claude-opus-5', inputTokens: totalTokens, outputTokens: 0, cacheReadTokens: 0,
      cacheWriteTokens: 0, totalTokens, requests: 1, cost: 1,
      source: 'claude', reasoningTokens: null, convs: 1, cacheEstimated: false,
      hasUnpriced: false, unattributedTokens: 0,
    });

    // The reported shape: a new day's total of 0 against the old day's rows.
    expect(modelBars([row(75_060_962)], 'claude', 0)[0].share).toBeNull();
    // The milder mismatch, which a clamp would have rendered as 4,600%.
    expect(modelBars([row(75_000_000)], 'claude', 1_600_000)[0].share).toBeNull();
    // Coherent windows still report a share, including the whole of one.
    expect(modelBars([row(50)], 'claude', 200)[0].share).toBeCloseTo(0.25);
    expect(modelBars([row(200)], 'claude', 200)[0].share).toBeCloseTo(1);
  });

  it('modelBars carries a null Model as a distinct non-Model row', () => {
    const rows: BreakdownRow[] = [
      { key: 'claude-opus-4-8', inputTokens: 50, outputTokens: 50, cacheReadTokens: 0,
        cacheWriteTokens: 0, totalTokens: 100, requests: 1, cost: 1,
        source: 'claude', reasoningTokens: null, convs: 1, cacheEstimated: false,
        hasUnpriced: false, unattributedTokens: 0 },
      { key: null, inputTokens: 20, outputTokens: 30, cacheReadTokens: 0,
        cacheWriteTokens: 0, totalTokens: 50, requests: 1, cost: null,
        source: 'claude', reasoningTokens: null, convs: 1, cacheEstimated: false,
        hasUnpriced: false, unattributedTokens: 50 },
    ];

    expect(modelBars(rows, 'claude', 150).map((bar) => bar.name)).toEqual([
      'claude-opus-4-8',
      null,
    ]);
  });
  it('catTotals sums one tool', () => {
    const t = catTotals(
      [pt({ source: 'claude' }), pt({ source: 'codex', inputTokens: 999 })],
      'claude',
    );
    expect(t).toEqual({ input: 100, output: 50, cacheRead: 200, cacheWrite: 30 });
  });
  it('rangeToFilters maps presets through rangeWindow', () => {
    expect(rangeToFilters('total', '', '')).toEqual({ tools: [], models: [], project: null });
    const f = rangeToFilters('custom', '2026-07-01', '2026-07-02');
    expect(f.startTs).toBeDefined();
    expect(f.endTs).toBeDefined();
  });
  it('rangeToFilters normalizes a reversed custom range like windowOf', () => {
    const forward = rangeToFilters('custom', '2026-07-01', '2026-07-08');
    const reversed = rangeToFilters('custom', '2026-07-08', '2026-07-01');
    expect(reversed).toEqual(forward);
    expect(reversed.startTs!).toBeLessThan(reversed.endTs!);
  });
});

describe('ctxMeta', () => {
  const res: CtxResource[] = [
    { source: 'claude', kind: 'skill', name: 'graphify' },
    { source: 'claude', kind: 'skill', name: 'verify' },
    { source: 'claude', kind: 'mcp_server', name: 'pencil' },
    { source: 'claude', kind: 'mcp_server', name: 'chrome' },
    { source: 'claude', kind: 'agent', name: 'Explore' },
    { source: 'claude', kind: 'memory_file', name: 'MEMORY.md' },
    { source: 'codex', kind: 'mcp_server', name: 'pencil' },
  ];
  it('renders counts in canonical order with pluralization', () => {
    expect(ctxMeta(res, 'claude')).toBe('2 skills · 2 MCP servers · 1 agent · 1 memory file');
  });
  it('omits zero kinds and scopes to the tool', () => {
    expect(ctxMeta(res, 'codex')).toBe('1 MCP server');
    expect(ctxMeta(res, 'hermes')).toBe('');
  });
});

describe('categorizeTool', () => {
  it('maps the known names', () => {
    expect(categorizeTool('Read')).toBe('File Ops');
    expect(categorizeTool('Edit')).toBe('File Ops');
    expect(categorizeTool('Grep')).toBe('Search');
    expect(categorizeTool('Bash')).toBe('Execution');
    expect(categorizeTool('TaskUpdate')).toBe('Task Mgmt');
    expect(categorizeTool('TodoWrite')).toBe('Task Mgmt');
    expect(categorizeTool('Task')).toBe('Agent');
    expect(categorizeTool('Agent')).toBe('Agent');
    expect(categorizeTool('WebFetch')).toBe('Web');
    expect(categorizeTool('mcp__pencil__batch_get')).toBe('MCP: pencil');
    expect(categorizeTool('Skill')).toBe('Skill');
    expect(categorizeTool('SomethingNew')).toBe('Other');
  });
});

describe('allocateByWeight', () => {
  it('children sum exactly to total (largest remainder)', () => {
    const out = allocateByWeight(100, [
      { key: 'a', weight: 1 }, { key: 'b', weight: 1 }, { key: 'c', weight: 1 },
    ]);
    const vals = [...out.values()];
    expect(vals.reduce((x, y) => x + y, 0)).toBe(100);
    expect(Math.max(...vals) - Math.min(...vals)).toBeLessThanOrEqual(1);
  });
  it('zero weights yield zeros', () => {
    const out = allocateByWeight(50, [{ key: 'a', weight: 0 }]);
    expect(out.get('a')).toBe(0);
  });
});

describe('toolTree', () => {
  const rows: CtxToolRow[] = [
    { source: 'claude', name: 'Bash', estTokens: 300, calls: 5 },
    { source: 'claude', name: 'Read', estTokens: 150, calls: 3 },
    { source: 'claude', name: 'Edit', estTokens: 50, calls: 2 },
  ];
  it('allocates parent total down two summing levels', () => {
    const tree = toolTree(rows, 1000);
    expect(tree.reduce((a, c) => a + c.tokens, 0)).toBe(1000);
    for (const cat of tree) {
      expect(cat.tools.reduce((a, t) => a + t.tokens, 0)).toBe(cat.tokens);
    }
    const exec = tree.find((c) => c.label === 'Execution')!;
    expect(exec.tokens).toBe(600); // 300/500 of 1000
  });
  it('null total or no rows → empty tree', () => {
    expect(toolTree(rows, null)).toEqual([]);
    expect(toolTree([], 1000)).toEqual([]);
  });
});

describe('mcpBars', () => {
  const rows: CtxToolRow[] = [
    { source: 'claude', name: 'mcp__pencil__batch_get', estTokens: 300, calls: 3 },
    { source: 'claude', name: 'mcp__pencil__get_screenshot', estTokens: 100, calls: 1 },
    { source: 'claude', name: 'mcp__codebase-memory-mcp__search_graph', estTokens: 600, calls: 4 },
    { source: 'claude', name: 'Bash', estTokens: 9999, calls: 50 },
  ];

  it('groups a server’s tools, ignores everything not MCP, heaviest first', () => {
    const bars = mcpBars(rows, 1000);
    expect(bars).toEqual([
      { name: 'codebase-memory-mcp', tokens: 600, calls: 4 },
      { name: 'pencil', tokens: 400, calls: 4 },
    ]);
  });

  it('allocates the MCP total, so the servers sum to the row above them', () => {
    // Weights 600/400 against a total that divides unevenly: largest-remainder
    // keeps the children summing exactly, which is what makes the drill-down
    // readable as a breakdown rather than a second, disagreeing estimate.
    const bars = mcpBars(rows, 999);
    expect(bars.reduce((a, b) => a + b.tokens, 0)).toBe(999);
  });

  it('is empty when the Source cannot attribute MCP, or served no calls', () => {
    expect(mcpBars(rows, null)).toEqual([]);
    expect(mcpBars([{ source: 'claude', name: 'Bash', estTokens: 10, calls: 1 }], 1000)).toEqual([]);
    expect(mcpBars(rows, 0)).toEqual([]);
  });

  it('agrees with the same server under the tool tree', () => {
    // The panel reaches a server two ways — Tool calls › MCP: pencil, and the
    // MCP servers row — allocating from different parents. Both parents scale
    // the same weights by the same billed/total factor, so a server must read
    // the same on both paths or the card contradicts itself.
    const both: CtxToolRow[] = [
      { source: 'claude', name: 'Bash', estTokens: 400, calls: 9 },
      { source: 'claude', name: 'mcp__pencil__batch_get', estTokens: 300, calls: 3 },
      { source: 'claude', name: 'mcp__terminal__read', estTokens: 300, calls: 2 },
    ];
    const viaTree = toolTree(both, 1000).find((c) => c.label === 'MCP: pencil')!.tokens;
    const viaRow = mcpBars(both, 600).find((b) => b.name === 'pencil')!.tokens;
    expect(viaRow).toBe(viaTree);
  });
});

describe('bucketView', () => {
  it('derives messages group and total', () => {
    const v = bucketView({ source: 'claude', history: 800, newInput: 100, system: 50, response: 40, reasoning: null })!;
    expect(v.messages).toBe(940);
    expect(v.total).toBe(990);
    expect(v.reasoning).toBeNull();
  });
  it('null in → null out', () => {
    expect(bucketView(null)).toBeNull();
  });
});

describe('skillBars', () => {
  const rows: CtxSkillRow[] = [
    { source: 'claude', name: 'grilling', estTokens: 5000, uses: 3 },
    { source: 'claude', name: 'playground:playground', estTokens: 900, uses: 1 },
    { source: 'claude', name: 'prototype', estTokens: 400, uses: 1 },
    { source: 'codex', name: 'other', estTokens: 9999, uses: 9 },
  ];

  it('keeps only the selected Source, in the order the query returned', () => {
    const bars = skillBars(rows, 'claude');
    expect(bars.map((b) => b.name)).toEqual(['grilling', 'playground:playground', 'prototype']);
    expect(bars[0]).toEqual({ name: 'grilling', tokens: 5000, uses: 3, rest: 0 });
  });

  it('folds everything past the cap into one remainder carrying its totals', () => {
    const bars = skillBars(rows, 'claude', 1);
    expect(bars).toHaveLength(2);
    // The remainder is unnamed on purpose: the label is the component's.
    expect(bars[1]).toEqual({ name: '', tokens: 1300, uses: 2, rest: 2 });
  });

  it('is empty for a Source that establishes no skills', () => {
    expect(skillBars(rows, 'gemini')).toEqual([]);
  });
});

describe('execFacets', () => {
  const rows: CtxExecRow[] = [
    { source: 'claude', kind: 'git_local', exe: 'git', cmd: 'git add', estTokens: 300, calls: 5 },
    { source: 'claude', kind: 'git_local', exe: 'git', cmd: 'git commit', estTokens: 100, calls: 2 },
    { source: 'claude', kind: 'test', exe: 'npm', cmd: 'npm test', estTokens: 100, calls: 3 },
  ];
  it('groups three ways and allocates the bash total exactly per facet', () => {
    const f = execFacets(rows, 1000)!;
    for (const facet of [f.byType, f.byExecutable, f.byCommand]) {
      expect(facet.reduce((a, r) => a + r.tokens, 0)).toBe(1000);
    }
    expect(f.byType.find((r) => r.key === 'git_local')!.tokens).toBe(800); // 400/500
    expect(f.byType.find((r) => r.key === 'git_local')!.calls).toBe(7);
    expect(f.byExecutable.find((r) => r.key === 'git')!.tokens).toBe(800);
    expect(f.byCommand.find((r) => r.key === 'git add')!.tokens).toBe(600);
    expect(f.byCommand[0].key).toBe('git add'); // sorted desc
  });
  it('null total or no rows → null', () => {
    expect(execFacets(rows, null)).toBeNull();
    expect(execFacets([], 1000)).toBeNull();
  });
});

describe('profileView', () => {
  it('merges a Model across the Sources that report it', () => {
    // codex and pi both report gpt-5.6-sol — one Model, not two rows.
    const p = profileView(
      [
        pt({ source: 'codex', totalTokens: 60, byModel: { 'gpt-5.6-sol': 60 } }),
        pt({ source: 'pi', totalTokens: 40, byModel: { 'gpt-5.6-sol': 40 } }),
      ],
      [],
    );
    expect(p.models).toHaveLength(1);
    expect(p.models[0]).toMatchObject({ name: 'gpt-5.6-sol', tokens: 100, share: 1 });
  });

  it('holds Unattributed Usage in the denominator without giving it a row', () => {
    const p = profileView(
      [pt({ source: 'pi', totalTokens: 100, byModel: { 'kimi-for-coding': 75 }, unattributedTokens: 25 })],
      [],
    );
    expect(p.models.map((m) => m.name)).toEqual(['kimi-for-coding']);
    expect(p.models[0].share).toBe(0.75); // not 1.0 — the 25 unattributed still count
  });

  it('ranks every Model in the window, leaving the top-N cut to the card', () => {
    const win = ['a', 'b', 'c', 'd', 'e', 'f', 'g'].map((m, i) =>
      pt({ totalTokens: 100 - i, byModel: { [m]: 100 - i } }),
    );
    expect(profileView(win, []).models.map((m) => m.name)).toEqual(['a', 'b', 'c', 'd', 'e', 'f', 'g']);
  });

  it('breaks a token tie by name', () => {
    const p = profileView(
      [pt({ totalTokens: 10, byModel: { zeta: 10 } }), pt({ totalTokens: 10, byModel: { alpha: 10 } })],
      [],
    );
    expect(p.models.map((m) => m.name)).toEqual(['alpha', 'zeta']);
  });

  it('counts only the window it was given, not the whole Ledger', () => {
    // The same series, sliced two ways: what the caller passes as the window is
    // the only thing the Models and their shares may describe.
    const all = [
      pt({ bucket: '2026-07-10', totalTokens: 100, byModel: { today: 100 } }),
      pt({ bucket: '2026-06-10', totalTokens: 900, byModel: { older: 900 } }),
    ];
    const p = profileView(all.slice(0, 1), all);
    expect(p.models).toEqual([{ name: 'today', tokens: 100, share: 1 }]);
    expect(p.windowTokens).toBe(100);
  });

  it('separates an empty window from one that is all Unattributed', () => {
    // Both leave the card with no rows; only the first is "no usage".
    expect(profileView([], []).windowTokens).toBe(0);
    const allUnattributed = profileView(
      [pt({ source: 'pi', totalTokens: 40, byModel: {}, unattributedTokens: 40 })],
      [],
    );
    expect(allUnattributed.models).toEqual([]);
    expect(allUnattributed.windowTokens).toBe(40);
  });

  it('has no Models for a window with no usage, whatever the Ledger holds', () => {
    const p = profileView([], [pt({ bucket: '2026-06-10', totalTokens: 800 })]);
    expect(p.models).toEqual([]);
    expect(p.startedIso).toBe('2026-06-10'); // the footer still describes the Ledger
  });
});

describe('profileView footer facts', () => {
  it('reads the Ledger span from the whole series, never from the window', () => {
    const pts = [
      pt({ bucket: '2026-07-10', totalTokens: 100 }),
      pt({ bucket: '2026-06-10', totalTokens: 800 }),
      pt({ bucket: '2026-06-10', source: 'codex', totalTokens: 5 }), // same day, 2nd Source
    ];
    const p = profileView(pts.slice(0, 1), pts); // window = the newest day only
    expect(p.startedIso).toBe('2026-06-10');
    expect(p.activeDays).toBe(2); // days, not points
  });

  it('counts no active day for a zero-token bucket', () => {
    const p = profileView([], [pt({ bucket: '2026-07-09', totalTokens: 0 })]);
    expect(p.activeDays).toBe(0);
    expect(p.startedIso).toBeNull();
  });
});

describe('presetsOf', () => {
  // Fixed "today" so the month/year boundaries are the ones being asserted.
  const LAST = '2026-07-29';

  it('offers only windows the range segments do not already cover', () => {
    expect(presetsOf('2020-01-01', LAST).map((p) => p.key))
      .toEqual(['yesterday', 'thisMonth', 'last90', 'thisYear']);
  });

  it('reads This month as the calendar month, not a trailing 30 days', () => {
    const p = presetsOf('2020-01-01', LAST).find((x) => x.key === 'thisMonth')!;
    expect(p).toEqual({ key: 'thisMonth', from: '2026-07-01', to: LAST });
  });

  it('clamps a window that starts before the first record', () => {
    const p = presetsOf('2026-07-10', LAST).find((x) => x.key === 'thisYear')!;
    expect(p.from).toBe('2026-07-10');
  });

  it('drops a window that ends before the first record entirely', () => {
    // First record today: yesterday selects nothing, so it is not offered.
    expect(presetsOf(LAST, LAST).map((p) => p.key)).toEqual(['thisMonth', 'last90', 'thisYear']);
  });

  // Configured Presets (Settings → Custom range). Slots are positional: a
  // cleared one stays a hole rather than pulling the later ones up.
  describe('configured presets', () => {
    const keys = (slots: PresetSlots) => presetsOf('2020-01-01', LAST, slots).map((p) => p.key);
    const one = (slot: PresetSlot, firstIso = '2020-01-01') =>
      presetsOf(firstIso, LAST, [slot]).find((p) => p.key === slot.key);

    it('appends them after the shipped four, in slot order', () => {
      expect(keys([{ key: 'lastYear' }, { key: 'rolling', days: 14 }])).toEqual([
        'yesterday', 'thisMonth', 'last90', 'thisYear', 'lastYear', 'rolling',
      ]);
    });

    it('leaves a cleared slot a hole instead of compacting the ones after it', () => {
      expect(keys([null, { key: 'lastMonth' }, null, { key: 'lastYear' }]))
        .toEqual(['yesterday', 'thisMonth', 'last90', 'thisYear', 'lastMonth', 'lastYear']);
    });

    it('reads a rolling shortcut as the N days ending today', () => {
      // 14 days *including* today, matching the shipped 90-day window.
      expect(one({ key: 'rolling', days: 14 }))
        .toEqual({ key: 'rolling', days: 14, from: '2026-07-16', to: LAST });
    });

    it('reads Last month as the whole previous calendar month', () => {
      expect(one({ key: 'lastMonth' }))
        .toEqual({ key: 'lastMonth', from: '2026-06-01', to: '2026-06-30' });
    });

    it('reads Last quarter as the previous calendar quarter', () => {
      // LAST is in Q3, so the completed quarter before it is Apr–Jun.
      expect(one({ key: 'lastQuarter' }))
        .toEqual({ key: 'lastQuarter', from: '2026-04-01', to: '2026-06-30' });
    });

    it('wraps Last quarter into the previous year from Q1', () => {
      const p = presetsOf('2020-01-01', '2026-02-10', [{ key: 'lastQuarter' }])
        .find((x) => x.key === 'lastQuarter');
      expect(p).toEqual({ key: 'lastQuarter', from: '2025-10-01', to: '2025-12-31' });
    });

    it('reads Last year as the whole previous calendar year', () => {
      expect(one({ key: 'lastYear' }))
        .toEqual({ key: 'lastYear', from: '2025-01-01', to: '2025-12-31' });
    });

    it('clamps one that starts before the first record', () => {
      expect(one({ key: 'lastMonth' }, '2026-06-15')?.from).toBe('2026-06-15');
    });

    it('drops one that ends before the first record entirely', () => {
      // A ledger starting mid-June: last year selects nothing, last month still
      // selects its tail.
      const ks = presetsOf('2026-06-20', LAST, [{ key: 'lastYear' }, { key: 'lastMonth' }])
        .map((p) => p.key);
      expect(ks).not.toContain('lastYear');
      expect(ks).toContain('lastMonth');
    });

    it('keeps shortcuts that clamp onto the same window as each other', () => {
      // A young Ledger collapses these two onto one window. A row someone
      // configured does not vanish for it — it comes back as history grows.
      const ps = presetsOf('2026-06-25', LAST, [{ key: 'lastMonth' }, { key: 'lastQuarter' }]);
      expect(ps.slice(-2)).toEqual([
        { key: 'lastMonth', from: '2026-06-25', to: '2026-06-30' },
        { key: 'lastQuarter', from: '2026-06-25', to: '2026-06-30' },
      ]);
    });
  });
});

// The Settings rows caption one configured Preset each, so they resolve one at
// a time against the same extent the picker uses.
describe('presetWindow', () => {
  const LAST = '2026-07-29';

  it('resolves a single configured Preset to its window', () => {
    expect(presetWindow({ key: 'lastMonth' }, '2020-01-01', LAST))
      .toEqual({ key: 'lastMonth', from: '2026-06-01', to: '2026-06-30' });
    expect(presetWindow({ key: 'rolling', days: 14 }, '2020-01-01', LAST))
      .toEqual({ key: 'rolling', days: 14, from: '2026-07-16', to: LAST });
  });

  it('clamps one that starts before the first record', () => {
    expect(presetWindow({ key: 'lastMonth' }, '2026-06-15', LAST)?.from).toBe('2026-06-15');
  });

  it('returns null for one the picker will not offer at all', () => {
    // A Ledger starting mid-June: last year ends long before the first record.
    expect(presetWindow({ key: 'lastYear' }, '2026-06-20', LAST)).toBeNull();
  });
});

describe('monthCells', () => {
  it('pads to whole Monday-first weeks', () => {
    const cells = monthCells(2026, 6); // July 2026 starts on a Wednesday
    expect(cells.length % 7).toBe(0);
    // Mon + Tue blank, so the 1st sits in the Wednesday column.
    expect(cells.slice(0, 3)).toEqual([null, null, '2026-07-01']);
    expect(cells.filter(Boolean)).toHaveLength(31);
    expect(cells[2 + 30]).toBe('2026-07-31');
  });
});

describe('dailyTotals', () => {
  it('sums every Source into one figure per day', () => {
    const totals = dailyTotals([
      pt({ bucket: '2026-07-01', source: 'claude', totalTokens: 100 }),
      pt({ bucket: '2026-07-01', source: 'codex', totalTokens: 25 }),
      pt({ bucket: '2026-07-03', source: 'claude', totalTokens: 7 }),
    ]);
    expect(totals).toEqual({ '2026-07-01': 125, '2026-07-03': 7 });
    // Days with no Usage Record are absent, not zero — the picker reads a
    // missing key as no usage.
    expect(totals['2026-07-02']).toBeUndefined();
  });
});

// --- The Trend Enlarge's read-out arithmetic, behind the selector seam ---
// These were computed inside TrendModal and reachable only through its DOM;
// value-level cases live here, the component renders a model it did not
// compute.

const bk = (key: string, total: number, byModel: Record<string, number> = {}, extras: Partial<Bucket> = {}): Bucket => ({
  key, label: key, byTool: {}, byModel, unattributedTokens: 0, hasUnpriced: false, total, ...extras,
});

describe('effectiveBounds', () => {
  it('falls back to the data extent when a raw custom input is empty', () => {
    expect(effectiveBounds('', '', '2026-01-01', '2026-08-01')).toEqual({ from: '2026-01-01', to: '2026-08-01' });
    expect(effectiveBounds('2026-02-01', '', '2026-01-01', '2026-08-01')).toEqual({ from: '2026-02-01', to: '2026-08-01' });
  });

  it('keeps both bounds when both are given', () => {
    expect(effectiveBounds('2026-02-01', '2026-03-01', '2026-01-01', '2026-08-01')).toEqual({ from: '2026-02-01', to: '2026-03-01' });
  });
});

describe('windowModelSplit', () => {
  it('sums byModel, unions modelSources, and totals Unattributed across buckets', () => {
    const split = windowModelSplit([
      bk('2026-07-01', 155, { m1: 100, shared: 50 }, { modelSources: { m1: ['claude'], shared: ['codex'] }, unattributedTokens: 5 }),
      bk('2026-07-02', 28, { shared: 25 }, { modelSources: { shared: ['pi', 'codex'] }, unattributedTokens: 3 }),
    ]);
    expect(split.byModel).toEqual({ m1: 100, shared: 75 });
    expect(split.modelSources).toEqual({ m1: ['claude'], shared: ['codex', 'pi'] });
    expect(split.unattributedTokens).toBe(8);
  });
});

describe('splitRows', () => {
  it('ranks Models largest first, folds the rest, Unattributed Usage last', () => {
    const rows = splitRows({ byModel: { a: 70, b: 20, c: 8, d: 2 }, modelSources: undefined, unattributedTokens: 5 }, { a: 'claude', b: 'codex', c: 'claude', d: 'pi' }, 2);
    expect(rows.map((r) => r.kind)).toEqual(['model', 'model', 'more', 'unattributed']);
    expect(rows[0]).toMatchObject({ kind: 'model', model: 'a', val: 70 });
    expect(rows[2]).toMatchObject({ kind: 'more', count: 2, val: 10 });
    expect(rows[3]).toMatchObject({ kind: 'unattributed', val: 5 });
  });

  it('resolves each owner from THIS split, not the window-wide map', () => {
    const rows = splitRows({ byModel: { a: 10 }, modelSources: { a: ['pi'] }, unattributedTokens: 0 }, { a: 'codex' }, 6);
    expect(rows[0]).toMatchObject({ kind: 'model', model: 'a' });
    expect((rows[0] as { owner: { keys: string[] } }).owner.keys).toEqual(['pi']);
  });

  it('omits the fold and the Unattributed row when there is nothing to say', () => {
    const rows = splitRows({ byModel: { a: 10, b: 5 }, modelSources: undefined, unattributedTokens: 0 }, {}, 6);
    expect(rows.map((r) => r.kind)).toEqual(['model', 'model']);
  });
});

describe('peakOf', () => {
  it('picks the biggest bucket; a tie keeps the earlier one; empty is undefined', () => {
    const first = bk('2026-07-01', 5);
    expect(peakOf([first, bk('2026-07-02', 9), bk('2026-07-03', 9)])?.key).toBe('2026-07-02');
    expect(peakOf([first, bk('2026-07-02', 5)])).toBe(first);
    expect(peakOf([])).toBeUndefined();
  });
});

describe('bucketStats', () => {
  it('ranks by total (ties share the better rank) and rounds delta vs the displayed average', () => {
    const buckets = [bk('a', 100), bk('b', 300), bk('c', 300), bk('d', 0)];
    expect(bucketStats(buckets, buckets[1], 175)).toEqual({ rank: 1, deltaPct: 71 });
    expect(bucketStats(buckets, buckets[2], 175)).toEqual({ rank: 1, deltaPct: 71 });
    expect(bucketStats(buckets, buckets[0], 175)).toEqual({ rank: 3, deltaPct: -43 });
  });

  it('reads a zero bucket against a live displayed average as -100%', () => {
    // An hourly window mid-fetch: buckets are zero while the footer average
    // (from the daily points) is not — the delta reads against the screen.
    expect(bucketStats([bk('a', 0)], bk('a', 0), 200)).toEqual({ rank: 1, deltaPct: -100 });
  });

  it('pins delta to 0 for a window with no usage rather than dividing by it', () => {
    const buckets = [bk('a', 0), bk('b', 0)];
    expect(bucketStats(buckets, buckets[0], 0)).toEqual({ rank: 1, deltaPct: 0 });
  });
});
