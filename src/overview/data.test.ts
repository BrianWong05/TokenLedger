import { describe, it, expect } from 'vitest';
import type { SeriesPoint, BreakdownRow, CtxResourceCount, CtxToolRow, CtxExecRow } from '../types';
import { emptyByTool } from './meta';
import {
  seriesToDays,
  heatStats,
  heatFilters,
  windowOf,
  pointsIn,
  bucketsFromPoints,
  smallMultiples,
  rankModels,
  modelTools,
  calendarSpan,
  presetsOf,
  monthCells,
  dailyTotals,
  dailyTableRows,
  projectTableRows,
  modelBars,
  catTotals,
  ctxTotals,
  ctxMeta,
  rangeToFilters,
  bucketFilters,
  bucketCsv,
  csvFilename,
  categorizeTool,
  allocateByWeight,
  toolTree,
  bucketView,
  execFacets,
  profileView,
  profileSessionFilters,
} from './data';

function pt(over: Partial<SeriesPoint>): SeriesPoint {
  return {
    bucket: '2026-07-09',
    source: 'claude',
    byModel: {},
    unattributedTokens: 0,
    hasUnpriced: false,
    inputTokens: 100,
    outputTokens: 50,
    cacheReadTokens: 200,
    cacheWriteTokens: 30,
    totalTokens: 380,
    reasoningTokens: null,
    cost: 0.5,
    requests: 2,
    convs: 1,
    ctxMessages: null,
    ctxSystem: null,
    ctxReasoning: null,
    ctxToolcalls: null,
    ctxAgents: null,
    ctxMcp: null,
    ctxSkills: null,
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
    byTool: emptyByTool(),
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
      key: '2026-07-15', label: '15', byTool: emptyByTool(),
      byModel: { a: 7, b: 6, c: 5, d: 4, e: 3, f: 2, g: 1, h: 0 },
      unattributedTokens: 0, hasUnpriced: false, total: 28,
    };
    const rows = bucketCsv(many, {}).trim().split('\n');
    expect(rows).toHaveLength(1 + 7); // header + 7 nonzero models, h dropped
    expect(rows[1].startsWith('a,')).toBe(true);
    expect(rows.some((r) => r.startsWith('h,'))).toBe(false);
  });

  it('CSV-escapes a field containing a comma and blanks an unknown tool', () => {
    const b = { key: 'k', label: 'k', byTool: emptyByTool(), byModel: { 'weird,name': 50 }, unattributedTokens: 0, hasUnpriced: false, total: 50 };
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

describe('ctxTotals', () => {
  it('sums per-tool ctx preserving null (never 0)', () => {
    const pts: SeriesPoint[] = [
      pt({ source: 'claude', inputTokens: 100, cacheReadTokens: 200, cacheWriteTokens: 30,
           ctxMessages: 250, ctxSystem: 60, ctxReasoning: 20, ctxToolcalls: 90 }),
      pt({ source: 'claude', inputTokens: 50, cacheReadTokens: 0, cacheWriteTokens: 0,
           ctxMessages: 40, ctxSystem: 10, ctxReasoning: 0 }),
      pt({ source: 'codex', ctxMessages: 999 }), // other tool: excluded
    ];
    const t = ctxTotals(pts, 'claude');
    expect(t.billed).toBe(380); // (100+200+30) + (50+0+0)
    expect(t.reused).toBe(200);
    expect(t.messages).toBe(290);
    expect(t.system).toBe(70);
    expect(t.reasoning).toBe(20);
    expect(t.toolcalls).toBe(90); // one null contributor does not zero it
    expect(t.agents).toBeNull();  // nothing reported anywhere: null, not 0
  });

  it('all-null source stays all null (hermes)', () => {
    const t = ctxTotals([pt({ source: 'hermes' })], 'hermes');
    expect(t.messages).toBeNull();
    expect(t.billed).toBe(330); // header still real: 100+200+30
  });

  it('keeps unavailable context distinct from an observed zero', () => {
    expect(ctxTotals([pt({ source: 'hermes', ctxMessages: null })], 'hermes').messages).toBeNull();
    expect(ctxTotals([pt({ source: 'claude', ctxMessages: 0 })], 'claude').messages).toBe(0);
  });
});

describe('ctxMeta', () => {
  const res: CtxResourceCount[] = [
    { source: 'claude', kind: 'skill', count: 32 },
    { source: 'claude', kind: 'mcp_server', count: 2 },
    { source: 'claude', kind: 'agent', count: 1 },
    { source: 'claude', kind: 'memory_file', count: 1 },
    { source: 'codex', kind: 'mcp_server', count: 5 },
  ];
  it('renders counts in canonical order with pluralization', () => {
    expect(ctxMeta(res, 'claude')).toBe('32 skills · 2 MCP servers · 1 agent · 1 memory file');
  });
  it('omits zero kinds and scopes to the tool', () => {
    expect(ctxMeta(res, 'codex')).toBe('5 MCP servers');
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
  // TODAY is 2026-07-10, so the 7d window opens at 07-04 and the 30d at 06-11.
  const spread = [
    pt({ bucket: '2026-07-10', source: 'claude', totalTokens: 100, byModel: { 'claude-opus-4-6': 100 } }),
    pt({ bucket: '2026-07-04', source: 'codex', totalTokens: 50, byModel: { 'gpt-5.4': 50 } }),
    pt({ bucket: '2026-07-03', source: 'codex', totalTokens: 20, byModel: { 'gpt-5.4': 20 } }),
    pt({ bucket: '2026-06-11', source: 'pi', totalTokens: 30, byModel: { 'kimi-for-coding': 30 } }),
    pt({ bucket: '2026-06-10', source: 'pi', totalTokens: 800, byModel: { 'kimi-for-coding': 800 } }),
  ];

  it('bounds 7d and 30d at the same trailing edges the range picker uses', () => {
    const p = profileView(spread, null, TODAY);
    expect(p.d7).toBe(150); // 07-10 + 07-04; 07-03 is the day past the edge
    expect(p.d30).toBe(200); // + 07-03 + 06-11; 06-10 is out
  });

  it('averages over active days, not calendar days', () => {
    const p = profileView(spread, null, TODAY);
    expect(p.activeDays).toBe(5);
    expect(p.perActiveDay).toBe(1000 / 5); // lifetime ÷ days with usage
    expect(p.startedIso).toBe('2026-06-10');
  });

  it('reports an unknown average as null rather than zero', () => {
    const p = profileView([], null, TODAY);
    expect(p.perActiveDay).toBeNull();
    expect(p.startedIso).toBeNull();
    expect(p.activeDays).toBe(0);
    expect(p.models).toEqual([]);
  });

  it('counts no active day for a zero-token bucket', () => {
    const p = profileView([pt({ bucket: '2026-07-09', totalTokens: 0 })], null, TODAY);
    expect(p.activeDays).toBe(0);
    expect(p.startedIso).toBeNull();
  });

  it('merges a Model across the Sources that report it', () => {
    // pi and codex both report gpt-5.6-sol — one Model, not two rows.
    const p = profileView(
      [
        pt({ bucket: '2026-07-09', source: 'codex', totalTokens: 60, byModel: { 'gpt-5.6-sol': 60 } }),
        pt({ bucket: '2026-07-09', source: 'pi', totalTokens: 40, byModel: { 'gpt-5.6-sol': 40 } }),
      ],
      null,
      TODAY,
    );
    expect(p.models).toHaveLength(1);
    expect(p.models[0]).toMatchObject({ name: 'gpt-5.6-sol', tokens: 100, share: 1 });
  });

  it('holds Unattributed Usage in the denominator without giving it a row', () => {
    const p = profileView(
      [
        pt({
          bucket: '2026-07-09', source: 'pi', totalTokens: 100,
          byModel: { 'kimi-for-coding': 75 }, unattributedTokens: 25,
        }),
      ],
      null,
      TODAY,
    );
    expect(p.models.map((m) => m.name)).toEqual(['kimi-for-coding']);
    expect(p.models[0].share).toBe(0.75); // not 1.0 — the 25 unattributed still count
  });

  it('keeps the top five by tokens, breaking ties by name', () => {
    const pts = ['a', 'b', 'c', 'd', 'e', 'f'].map((m, i) =>
      pt({ bucket: '2026-07-09', totalTokens: 10, byModel: { [m]: i === 5 ? 10 : 100 - i } }),
    );
    const p = profileView(pts, null, TODAY);
    expect(p.models.map((m) => m.name)).toEqual(['a', 'b', 'c', 'd', 'e']);
  });

  it('passes the Session count straight through, null included', () => {
    expect(profileView(spread, 42, TODAY).sessions30d).toBe(42);
    expect(profileView(spread, null, TODAY).sessions30d).toBeNull();
  });
});

describe('profileSessionFilters', () => {
  it('opens 30 local days back including today, with no upper bound', () => {
    // Same shape as the 'month' preset (rangeWindow: back(29)), but fixed —
    // the Profile never follows the range picker.
    const f = profileSessionFilters(TODAY);
    expect(f.startTs).toBe(Math.floor(new Date(2026, 5, 11).getTime() / 1000));
    expect(f.endTs).toBeUndefined();
    expect(f.tools).toEqual([]);
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
