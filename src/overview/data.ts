// Real-data layer for the Overview: shared design meta plus pure reshaping of
// backend responses (SeriesPoint/BreakdownRow) into the shapes the components
// consume. No fetching here — Overview8b orchestrates IPC calls.
import type { BreakdownRow, Filters, SeriesPoint, DateRange } from '../types';
import { rangeToBounds } from '../lib/dateRange';

export type ToolKey = 'claude' | 'codex' | 'gemini' | 'hermes';

export interface ToolMeta {
  key: ToolKey;
  label: string;
  source: string; // full source name, e.g. "Claude Code"
  color: string;
}

export const TOOLS: ToolMeta[] = [
  { key: 'claude', label: 'Claude', source: 'Claude Code', color: '#3b82f6' },
  { key: 'codex', label: 'Codex', source: 'Codex', color: '#37c98b' },
  { key: 'gemini', label: 'Gemini', source: 'Gemini CLI', color: '#e2a63b' },
  { key: 'hermes', label: 'Hermes', source: 'Hermes', color: '#f472b6' },
];

// The four canonical token categories (CONTEXT.md).
export const CATEGORIES = [
  { key: 'input', label: 'Input', color: '#7c5cff' },
  { key: 'output', label: 'Output', color: '#2fbf71' },
  { key: 'cacheRead', label: 'Cache read', color: '#3aa0ff' },
  { key: 'cacheWrite', label: 'Cache write', color: '#f0a03c' },
] as const;

// Heatmap ramps: index 0 = empty cell, 1..4 = ascending intensity.
export const THEMES: Record<string, string[]> = {
  ocean: ['#12161f', '#173a63', '#1f5aa6', '#2f80ed', '#63a4ff'],
  emerald: ['#12161f', '#14503a', '#1a7d55', '#25a56f', '#4ad991'],
  neon: ['#12161f', '#312a63', '#4b3aa6', '#6d4fed', '#9a7cff'],
  amber: ['#12161f', '#5a4114', '#8a6417', '#c98f25', '#f0b84a'],
};
export const THEME_OPTIONS = [
  { value: 'ocean', label: 'Blue' },
  { value: 'emerald', label: 'Green' },
  { value: 'neon', label: 'Violet' },
  { value: 'amber', label: 'Amber' },
];

export interface Day {
  index: number;
  date: Date;
  iso: string;
  weekday: number; // 0 = Sun
  col: number;
  row: number;
  tokens: number;
  cost: number;
  level: 0 | 1 | 2 | 3 | 4;
  byTool: Record<ToolKey, number>;
}

export interface Bucket {
  label: string;
  byTool: Record<ToolKey, number>;
  total: number;
}

export interface TableRow {
  label: string; // iso date (daily) or project path — also the sort key
  total: number;
  input: number;
  output: number;
  cached: number;            // cache read tokens
  reasoning: number | null;  // null = no contributing source reported it
  convs: number;
}

export type Range8b = 'day' | 'week' | 'month' | 'total' | 'custom';
export const RANGES_8B: { key: Range8b; label: string; long: string }[] = [
  { key: 'day', label: 'Day', long: 'Today' },
  { key: 'week', label: 'Week', long: 'Last 7 days' },
  { key: 'month', label: 'Month', long: 'Last 30 days' },
  { key: 'total', label: 'Total', long: 'All time' },
  { key: 'custom', label: 'Custom', long: 'Custom range' },
];

export function emptyByTool(): Record<ToolKey, number> {
  return { claude: 0, codex: 0, gemini: 0, hermes: 0 };
}

function isoOf(d: Date): string {
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return `${d.getFullYear()}-${m}-${day}`;
}

// ---- heatmap days ----

// Trailing 365 local days ending today, filled from per-(day, source) rows.
// Intensity levels come from the quartiles of the nonzero-day distribution.
export function seriesToDays(points: SeriesPoint[], today: Date = new Date()): Day[] {
  const byDate = new Map<string, SeriesPoint[]>();
  for (const p of points) {
    const arr = byDate.get(p.bucket);
    if (arr) arr.push(p);
    else byDate.set(p.bucket, [p]);
  }
  const end = new Date(today.getFullYear(), today.getMonth(), today.getDate());
  const start = new Date(end);
  start.setDate(start.getDate() - 364);
  const startDow = start.getDay();

  const days: Day[] = [];
  for (let i = 0; i < 365; i++) {
    const date = new Date(start);
    date.setDate(start.getDate() + i);
    const iso = isoOf(date);
    const byTool = emptyByTool();
    let tokens = 0;
    let cost = 0;
    for (const p of byDate.get(iso) ?? []) {
      if (p.source in byTool) byTool[p.source as ToolKey] += p.totalTokens;
      tokens += p.totalTokens;
      cost += p.cost;
    }
    const cell = i + startDow;
    days.push({
      index: i, date, iso, weekday: date.getDay(),
      col: Math.floor(cell / 7), row: cell % 7,
      tokens, cost, level: 0, byTool,
    });
  }

  const nonzero = days.filter((d) => d.tokens > 0).map((d) => d.tokens).sort((a, b) => a - b);
  const q = (f: number) => nonzero[Math.min(nonzero.length - 1, Math.floor(f * nonzero.length))] ?? 0;
  const [q1, q2, q3] = [q(0.25), q(0.5), q(0.75)];
  for (const d of days) {
    d.level = d.tokens <= 0 ? 0 : d.tokens <= q1 ? 1 : d.tokens <= q2 ? 2 : d.tokens <= q3 ? 3 : 4;
  }
  return days;
}

// ---- range windows over series points ----

export interface Window {
  fromIso?: string; // inclusive
  toIso?: string;   // inclusive
}

// Must agree with rangeToFilters/rangeToBounds: day = today, week = trailing 7
// local days, month = trailing 30, total = unbounded, custom = [from, to].
export function windowOf(range: Range8b, customFrom: string, customTo: string, today: Date = new Date()): Window {
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

export function pointsIn(points: SeriesPoint[], win: Window): SeriesPoint[] {
  return points.filter(
    (p) => (!win.fromIso || p.bucket >= win.fromIso) && (!win.toIso || p.bucket <= win.toIso.slice(0, 10) + '~'),
  );
}

export function rangeToFilters(range: Range8b, customFrom: string, customTo: string): Filters {
  const dr: DateRange =
    range === 'day' ? 'today'
    : range === 'week' ? '7d'
    : range === 'month' ? '30d'
    : range === 'total' ? 'all'
    : { start: customFrom, end: customTo };
  return { tools: [], models: [], project: null, ...rangeToBounds(dr) };
}

// ---- trend buckets ----

export type Granularity = 'hour' | 'day' | 'week' | 'month';
const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

// Adaptive granularity: hourly for a single day, daily up to a month,
// weekly up to ~a quarter, monthly beyond.
export function granularityOf(range: Range8b, spanDays: number): Granularity {
  if (range === 'day') return 'hour';
  if (range === 'week' || range === 'month') return 'day';
  return spanDays <= 31 ? 'day' : spanDays <= 120 ? 'week' : 'month';
}

function weekKey(iso: string): string {
  const [y, m, d] = iso.split('-').map(Number);
  const date = new Date(y, m - 1, d);
  date.setDate(date.getDate() - date.getDay()); // back to Sunday
  return isoOf(date);
}

export function bucketsFromPoints(pts: SeriesPoint[], per: Granularity): Bucket[] {
  const keyOf = (p: SeriesPoint) =>
    per === 'hour' ? p.bucket
    : per === 'day' ? p.bucket.slice(0, 10)
    : per === 'week' ? weekKey(p.bucket.slice(0, 10))
    : p.bucket.slice(0, 7); // YYYY-MM
  const map = new Map<string, Record<ToolKey, number>>();
  for (const p of pts) {
    const k = keyOf(p);
    const by = map.get(k) ?? emptyByTool();
    if (p.source in by) by[p.source as ToolKey] += p.totalTokens;
    map.set(k, by);
  }
  const labelOf = (k: string) =>
    per === 'hour' ? String(parseInt(k.slice(11, 13), 10))
    : per === 'day' ? String(parseInt(k.slice(8, 10), 10))
    : per === 'month' ? MONTHS[parseInt(k.slice(5, 7), 10) - 1]
    : k; // week: placeholder, renumbered below
  const out = [...map.entries()]
    .sort((a, b) => (a[0] < b[0] ? -1 : 1))
    .map(([k, by]) => ({
      label: labelOf(k),
      byTool: by,
      total: (Object.values(by) as number[]).reduce((a, b) => a + b, 0),
    }));
  if (per === 'week') out.forEach((b, i) => (b.label = 'W' + (i + 1)));
  return out;
}

// ---- aggregations ----

export function sumPoints(pts: SeriesPoint[]): number {
  return pts.reduce((a, p) => a + p.totalTokens, 0);
}

export function toolTotalsOfPoints(pts: SeriesPoint[]): Record<ToolKey, number> {
  const out = emptyByTool();
  for (const p of pts) if (p.source in out) out[p.source as ToolKey] += p.totalTokens;
  return out;
}

// Per-tool sparkline series over the buckets (small multiples).
export function smallMultiples(bks: Bucket[]) {
  const totals = emptyByTool();
  for (const b of bks) for (const t of TOOLS) totals[t.key] += b.byTool[t.key];
  const grand = (Object.values(totals) as number[]).reduce((a, b) => a + b, 0) || 1;
  return TOOLS.map((t) => ({
    ...t,
    total: totals[t.key],
    share: totals[t.key] / grand,
    series: bks.map((b) => b.byTool[t.key]),
  }));
}

export interface CatTotals {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
}

export function catTotals(pts: SeriesPoint[], tool: ToolKey): CatTotals {
  const t: CatTotals = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 };
  for (const p of pts) {
    if (p.source !== tool) continue;
    t.input += p.inputTokens;
    t.output += p.outputTokens;
    t.cacheRead += p.cacheReadTokens;
    t.cacheWrite += p.cacheWriteTokens;
  }
  return t;
}

// ---- table rows ----

export function dailyTableRows(pts: SeriesPoint[]): TableRow[] {
  const byDay = new Map<string, TableRow>();
  for (const p of pts) {
    const label = p.bucket.slice(0, 10);
    const r = byDay.get(label) ?? {
      label, total: 0, input: 0, output: 0, cached: 0, reasoning: null, convs: 0,
    };
    r.total += p.totalTokens;
    r.input += p.inputTokens;
    r.output += p.outputTokens;
    r.cached += p.cacheReadTokens;
    if (p.reasoningTokens != null) r.reasoning = (r.reasoning ?? 0) + p.reasoningTokens;
    // Sessions never span sources, so summing per-source convs within one day is safe.
    r.convs += p.convs;
    byDay.set(label, r);
  }
  return [...byDay.values()];
}

export function projectTableRows(rows: BreakdownRow[]): TableRow[] {
  return rows.map((r) => ({
    label: r.key,
    total: r.totalTokens,
    input: r.inputTokens,
    output: r.outputTokens,
    cached: r.cacheReadTokens,
    reasoning: r.reasoningTokens,
    convs: r.convs,
  }));
}

// ---- models panel ----

export interface ModelBar {
  name: string;
  tokens: number;
  cost: number | null; // null = unpriced
  share: number;       // of the tool's range total
  segs: { key: string; color: string; frac: number }[];
  cacheEstimated: boolean;
}

export function modelBars(rows: BreakdownRow[], tool: ToolKey, toolTokens: number): ModelBar[] {
  return rows
    .filter((r) => r.source === tool)
    .map((r) => {
      const total = Math.max(1, r.totalTokens);
      const cats = [r.inputTokens, r.outputTokens, r.cacheReadTokens, r.cacheWriteTokens];
      return {
        name: r.key,
        tokens: r.totalTokens,
        cost: r.cost,
        share: r.totalTokens / Math.max(1, toolTokens),
        segs: CATEGORIES.map((c, i) => ({ key: c.key, color: c.color, frac: cats[i] / total })),
        cacheEstimated: r.cacheEstimated,
      };
    });
}
