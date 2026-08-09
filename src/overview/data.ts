// Real-data layer for the Overview: pure reshaping of backend responses
// (SeriesPoint/BreakdownRow) into the shapes the components consume. No fetching
// here — overviewStore orchestrates the Ledger reads. Design meta (SOURCES,
// CATEGORIES, themes, ranges, types) lives in ./meta.
import type { BreakdownRow, Filters, SeriesPoint, CtxResource, CtxBuckets, CtxToolRow, CtxSkillRow, CtxExecRow } from '../types';
import { parseLocalDate } from '../lib/dateRange';
import { SOURCES, CATEGORIES, emptyBySource, orderedSourceKeys, sourceMeta, type SourceKey, type SourceMeta, type Range8b } from './meta';
import type { Lang } from '../lib/i18n';

export const UNATTRIBUTED_COLOR = '#7a8499';
import { monthShortL, overviewT, type OverviewKey } from './localize';

// One bucket's (or day's) models as [name, tokens] pairs, largest first with
// zero-token models dropped — the shape every per-bucket read-out (card and
// heatmap tooltips, the enlarge inspector, the CSV) starts from.
export function rankedModels(byModel: Record<string, number>): [string, number][] {
  return Object.entries(byModel)
    .filter(([, v]) => v > 0)
    .sort((a, b) => b[1] - a[1]);
}

// Models present in the buckets, ordered by window total descending.
export function rankModels(bks: Bucket[]): string[] {
  const totals = new Map<string, number>();
  for (const b of bks) {
    for (const [m, v] of Object.entries(b.byModel)) {
      if (v > 0) totals.set(m, (totals.get(m) ?? 0) + v);
    }
  }
  return [...totals.entries()].sort((a, b) => b[1] - a[1]).map(([m]) => m);
}

// The Source metadata owning a model, including a neutral historical-key fallback.
function modelSourceMeta(modelTool: Record<string, string>, m: string): SourceMeta | undefined {
  return modelTool[m] ? sourceMeta(modelTool[m]) : undefined;
}

// A model's stack color: the brand accent of its owning tool (grey fallback for
// an unknown source). Shared by the trend card and its enlarge.
export function modelColor(modelTool: Record<string, string>, m: string): string {
  return modelSourceMeta(modelTool, m)?.color ?? '#5f6880';
}

// Models ordered for a stacked bar: grouped by owning Source (in SOURCES order),
// largest-first within each Source. The stable sort over rankModels keeps the
// largest-first order inside each Source block, so bars read as contiguous Source
// blocks. Shared by the trend card and its enlarge.
export function stackModels(bks: Bucket[], modelTool: Record<string, string>): string[] {
  const toolIdx = (m: string) => {
    const i = SOURCES.findIndex((source) => source.key === modelTool[m]);
    return i < 0 ? SOURCES.length : i;
  };
  return rankModels(bks).sort((a, b) => toolIdx(a) - toolIdx(b));
}

// Model -> owning tool across the whole window, last-write-wins. Models CAN span
// sources (pi and codex both report `gpt-5.6-sol`), so this is only a fallback
// for read-outs with no per-bucket record — prefer modelOwner() below.
export function modelTools(pts: SeriesPoint[]): Record<string, string> {
  const out: Record<string, string> = {};
  for (const p of pts) for (const m of Object.keys(p.byModel)) out[m] = p.source;
  return out;
}

// Which Source produced a Model *here* (in one bucket or day), for read-outs
// that name or colour it. byModel merges a Model's tokens across Sources, so a
// window-wide map picks the wrong owner for some buckets: resolve from the
// bucket's own record and fall back to `modelTool` only when it has none.
export interface ModelOwner {
  keys: string[];
  label?: string; // 'Codex', or 'Codex + pi' when genuinely produced by both
  color: string;
}

export function modelOwner(
  modelSources: Record<string, string[]> | undefined,
  modelTool: Record<string, string>,
  m: string,
): ModelOwner {
  const here = modelSources?.[m];
  const keys = here?.length ? here : modelTool[m] ? [modelTool[m]] : [];
  const metas = keys.map(sourceMeta);
  return {
    keys,
    label: keys.length ? keys.map((k, i) => metas[i]?.label ?? k).join(' + ') : undefined,
    // One owner -> that Source's accent. Mixed or unknown keeps the window-wide
    // colour so a segment never claims a Source it may not be.
    color: (keys.length === 1 ? metas[0]?.color : undefined) ?? modelColor(modelTool, m),
  };
}

export interface Day {
  index: number;
  date: Date;
  iso: string;
  weekday: number; // 0 = Sun
  col: number;
  row: number;
  tokens: number;
  level: 0 | 1 | 2 | 3 | 4;
  byTool: Record<string, number>;
  byModel: Record<string, number>;
  // Source(s) that produced each Model on this day — see modelOwner().
  modelSources?: Record<string, string[]>;
  unattributedTokens: number;
}

export interface Bucket {
  key: string;  // bucket key: iso day, 'YYYY-MM-DD HH:00', week-start iso, or 'YYYY-MM'
  label: string;
  byTool: Record<string, number>;
  byModel: Record<string, number>;
  // Source(s) that produced each Model IN THIS BUCKET. byModel merges a Model's
  // tokens across Sources, and two Sources can run the same Model name (pi and
  // codex both report `gpt-5.6-sol`), so a range-wide model->Source map picks
  // the wrong owner for some buckets. Optional: only bucketsFromPoints fills it.
  modelSources?: Record<string, string[]>;
  unattributedTokens: number;
  hasUnpriced: boolean;
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

// Local date -> 'YYYY-MM-DD'. The one formatter — keep in sync with nothing:
// every overview consumer imports this.
export function isoOf(d: Date): string {
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return `${d.getFullYear()}-${m}-${day}`;
}

// Inclusive number of calendar days between two ISO dates.
export function calendarSpan(fromIso: string, toIso: string): number {
  const ms = parseLocalDate(toIso).getTime() - parseLocalDate(fromIso).getTime();
  return Math.max(1, Math.round(ms / 86_400_000) + 1);
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
    const byTool = emptyBySource();
    const byModel: Record<string, number> = {};
    const modelSources: Record<string, string[]> = {};
    let unattributedTokens = 0;
    let tokens = 0;
    for (const p of byDate.get(iso) ?? []) {
      byTool[p.source] = (byTool[p.source] ?? 0) + p.totalTokens;
      for (const [m, v] of Object.entries(p.byModel)) {
        byModel[m] = (byModel[m] ?? 0) + v;
        const owners = (modelSources[m] ??= []);
        if (!owners.includes(p.source)) owners.push(p.source);
      }
      unattributedTokens += p.unattributedTokens;
      tokens += p.totalTokens;
    }
    const cell = i + startDow;
    days.push({
      index: i, date, iso, weekday: date.getDay(),
      col: Math.floor(cell / 7), row: cell % 7,
      tokens, level: 0, byTool, byModel, modelSources, unattributedTokens,
    });
  }

  // Level = quartile of the day's RANK among active days, so the busiest day
  // is always brightest and a short all-equal history doesn't render dimmest.
  const nonzero = days.filter((d) => d.tokens > 0).map((d) => d.tokens).sort((a, b) => a - b);
  const countLE = (t: number) => {
    let lo = 0;
    let hi = nonzero.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (nonzero[mid] <= t) lo = mid + 1;
      else hi = mid;
    }
    return lo;
  };
  for (const d of days) {
    if (d.tokens <= 0) {
      d.level = 0;
      continue;
    }
    const rank = countLE(d.tokens) / nonzero.length; // (0, 1]
    d.level = Math.min(4, Math.max(1, Math.ceil(rank * 4))) as Day['level'];
  }
  return days;
}

// Filters for the heatmap's trailing-365-day window (the same days
// seriesToDays fills), as epoch-second bounds: [midnight 364 days ago,
// midnight after today). The enlarge fetches its Summary with these so the
// Cost figure and its Partial-Cost marker describe exactly the days shown.
// The Profile's Session tile: the same trailing-30-day window the range
// picker's 'month' preset uses, but fixed — the Profile ignores the selection.
export function profileSessionFilters(today: Date = new Date()): Filters {
  const end = new Date(today.getFullYear(), today.getMonth(), today.getDate());
  const start = new Date(end);
  start.setDate(start.getDate() - 29);
  return {
    tools: [], models: [], project: null,
    startTs: Math.floor(start.getTime() / 1000),
  };
}

export function heatFilters(today: Date = new Date()): Filters {
  const end = new Date(today.getFullYear(), today.getMonth(), today.getDate());
  const start = new Date(end);
  start.setDate(start.getDate() - 364);
  const next = new Date(end);
  next.setDate(next.getDate() + 1);
  return {
    tools: [], models: [], project: null,
    startTs: Math.floor(start.getTime() / 1000),
    endTs: Math.floor(next.getTime() / 1000),
  };
}

export interface HeatStats {
  totalTokens: number;
  activeDays: number;
  streak: number; // longest run of consecutive active days
  bestDay: Day; // busiest day (peak)
}

// Aggregate read-outs over the heatmap's 365 days — shared by the Activity card
// and its full-screen enlarge so both report identical figures.
export function heatStats(days: Day[]): HeatStats {
  const totalTokens = days.reduce((a, d) => a + d.tokens, 0);
  const activeDays = days.filter((d) => d.tokens > 0).length;
  let streak = 0, run = 0;
  for (const d of days) {
    if (d.tokens > 0) { run += 1; streak = Math.max(streak, run); } else run = 0;
  }
  const bestDay = days.reduce((a, d) => (d.tokens > a.tokens ? d : a), days[0]);
  return { totalTokens, activeDays, streak, bestDay };
}

// ---- range windows over series points ----

export interface Window {
  fromIso?: string; // inclusive
  toIso?: string;   // inclusive
}

// The single home for the Range8b -> local-day window. Both representations
// derive from it: windowOf returns the inclusive ISO pair (client-side slicing),
// rangeToFilters returns the epoch-seconds bounds (Ledger queries).
//   day = today · week = trailing 7 local days · month = trailing 30 ·
//   total = unbounded · custom = normalized [lo, hi].
// LOAD-BEARING: presets leave endTs open (undefined) — the Ledger query gets
// only a lower bound for day/week/month; 'total' leaves both open. Only a custom
// range sends an exclusive upper bound: endTs = (toIso + 1 day) local midnight.
function rangeWindow(
  range: Range8b, customFrom: string, customTo: string, today: Date,
): { fromIso?: string; toIso?: string; startTs?: number; endTs?: number } {
  const end = new Date(today.getFullYear(), today.getMonth(), today.getDate());
  const iso = isoOf(end);
  const back = (n: number) => {
    const d = new Date(end);
    d.setDate(d.getDate() - n);
    return isoOf(d);
  };
  const midnightSecs = (isoStr: string, plusDays = 0) => {
    const d = parseLocalDate(isoStr);
    d.setDate(d.getDate() + plusDays);
    return Math.floor(d.getTime() / 1000);
  };
  switch (range) {
    case 'day': return { fromIso: iso, toIso: iso, startTs: midnightSecs(iso) };
    case 'week': { const f = back(6); return { fromIso: f, toIso: iso, startTs: midnightSecs(f) }; }
    case 'month': { const f = back(29); return { fromIso: f, toIso: iso, startTs: midnightSecs(f) }; }
    case 'total': return {};
    case 'custom': {
      const lo = customFrom <= customTo ? customFrom : customTo;
      const hi = customFrom <= customTo ? customTo : customFrom;
      return { fromIso: lo, toIso: hi, startTs: midnightSecs(lo), endTs: midnightSecs(hi, 1) };
    }
  }
}

// ---- custom-range picker inputs ----

// The shortcuts the picker offers. Deliberately none that a segment already
// covers: Week is the trailing 7 days, Month the trailing 30, Total everything.
// "This month" earns its place by being the calendar month, not a trailing 30.
export type PresetKey = 'yesterday' | 'thisMonth' | 'last90' | 'thisYear';

export interface RangePreset { key: PresetKey; from: string; to: string }

export function presetsOf(firstIso: string, lastIso: string): RangePreset[] {
  const shift = (iso: string, n: number) => {
    const d = parseLocalDate(iso);
    d.setDate(d.getDate() + n);
    return isoOf(d);
  };
  const yesterday = shift(lastIso, -1);
  return ([
    { key: 'yesterday', from: yesterday, to: yesterday },
    { key: 'thisMonth', from: `${lastIso.slice(0, 8)}01`, to: lastIso },
    { key: 'last90', from: shift(lastIso, -89), to: lastIso },
    { key: 'thisYear', from: `${lastIso.slice(0, 4)}-01-01`, to: lastIso },
  ] as RangePreset[])
    // a window falling entirely before the first record would select nothing
    .filter((p) => p.to >= firstIso)
    .map((p) => ({ ...p, from: p.from < firstIso ? firstIso : p.from }));
}

// One month as Monday-first calendar cells; nulls pad the leading and trailing
// week so the grid stays seven wide.
export function monthCells(year: number, month: number): (string | null)[] {
  const lead = (new Date(year, month, 1).getDay() + 6) % 7;
  const days = new Date(year, month + 1, 0).getDate();
  const cells: (string | null)[] = Array(lead).fill(null);
  for (let d = 1; d <= days; d++) cells.push(isoOf(new Date(year, month, d)));
  while (cells.length % 7) cells.push(null);
  return cells;
}

// Tokens per day across every Source — the picker's per-day usage marks.
export function dailyTotals(points: SeriesPoint[]): Record<string, number> {
  const out: Record<string, number> = {};
  for (const p of points) out[p.bucket] = (out[p.bucket] ?? 0) + p.totalTokens;
  return out;
}

export function windowOf(range: Range8b, customFrom: string, customTo: string, today: Date = new Date()): Window {
  const { fromIso, toIso } = rangeWindow(range, customFrom, customTo, today);
  return { fromIso, toIso };
}

export function pointsIn(points: SeriesPoint[], win: Window): SeriesPoint[] {
  return points.filter(
    (p) => (!win.fromIso || p.bucket >= win.fromIso) && (!win.toIso || p.bucket <= win.toIso.slice(0, 10) + '~'),
  );
}

export function rangeToFilters(range: Range8b, customFrom: string, customTo: string): Filters {
  const { startTs, endTs } = rangeWindow(range, customFrom, customTo, new Date());
  const f: Filters = { tools: [], models: [], project: null };
  if (startTs !== undefined) f.startTs = startTs;
  if (endTs !== undefined) f.endTs = endTs;
  return f;
}

// Exact epoch-second bounds [start, end) for a single trend bucket, so the
// enlarge inspector can fetch that bucket's Cost with the same honesty rules as
// any other window. The key format follows its granularity: 'YYYY-MM-DD HH:00'
// (hour), 'YYYY-MM-DD' (day, or a week's start day), 'YYYY-MM' (month).
export function bucketFilters(key: string, per: Granularity): Filters {
  const secs = (d: Date) => Math.floor(d.getTime() / 1000);
  let start: Date;
  let end: Date;
  if (per === 'hour') {
    start = parseLocalDate(key.slice(0, 10));
    start.setHours(parseInt(key.slice(11, 13), 10));
    end = new Date(start);
    end.setHours(start.getHours() + 1);
  } else if (per === 'month') {
    const y = parseInt(key.slice(0, 4), 10);
    const m = parseInt(key.slice(5, 7), 10) - 1;
    start = new Date(y, m, 1);
    end = new Date(y, m + 1, 1);
  } else {
    // day and week both key off a start day; a week spans seven days.
    start = parseLocalDate(key);
    end = new Date(start);
    end.setDate(end.getDate() + (per === 'week' ? 7 : 1));
  }
  return { tools: [], models: [], project: null, startTs: secs(start), endTs: secs(end) };
}

// A filesystem-safe default name for a bucket's CSV export: the bucket key with
// its spaces/colons flattened, e.g. usage-2026-07-15.csv (day/week),
// usage-2026-07-15-09-00.csv (hour), usage-2026-07.csv (month).
export function csvFilename(key: string): string {
  return `usage-${key.replace(/[: ]/g, '-')}.csv`;
}

// A CSV of one trend bucket's per-Model usage: header plus one row per Model
// (largest first), each with its owning tool label and its share of the bucket.
// Pure — the file write itself happens in the Rust save_csv command.
export function bucketCsv(bucket: Bucket, modelTool: Record<string, string>): string {
  const esc = (s: string) => (/[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s);
  const toolOf = (m: string) => modelSourceMeta(modelTool, m)?.label ?? modelTool[m] ?? '';
  const total = bucket.total || 1;
  const lines = ['model,tool,tokens,share'];
  for (const [m, v] of rankedModels(bucket.byModel)) {
    lines.push([esc(m), esc(toolOf(m)), String(v), (v / total).toFixed(4)].join(','));
  }
  return lines.join('\n') + '\n';
}

// ---- trend buckets ----

export type Granularity = 'hour' | 'day' | 'week' | 'month';

// Adaptive granularity: hourly for a single day, daily up to a month, weekly up
// to ~a quarter, monthly beyond. The span decides alone — the presets need no
// case of their own, since Day spans 1, Week 7 and Month 30 land here anyway.
export function granularityOf(spanDays: number): Granularity {
  return spanDays === 1 ? 'hour' : spanDays <= 31 ? 'day' : spanDays <= 120 ? 'week' : 'month';
}

// A range resolved to what the trend actually draws: the slicing window, the
// closed day bounds it zero-fills between (open ends fall back to the Ledger's
// own extent), and the bucket size that fits them. THE one place the automatic
// granularity rule runs — everything downstream reads the answer from here.
interface WindowFit {
  win: Window;      // for slicing points: open bounds stay open
  fromIso: string;  // closed bounds, for zero-filling and hourly identity
  toIso: string;
  per: Granularity;
}

function windowFit(
  range: Range8b, from: string, to: string, firstIso: string, lastIso: string, now: Date,
): WindowFit {
  const win = windowOf(range, from, to, now);
  const fromIso = win.fromIso ?? firstIso;
  const toIso = win.toIso ?? lastIso;
  return { win, fromIso, toIso, per: granularityOf(calendarSpan(fromIso, toIso)) };
}

// The local day a window buckets by hour, or null when it buckets coarser, so
// hourPoints get fetched exactly when the bars need them — and naming the day
// lets a holder tell an hourly series describing THIS window from a leftover.
export function hourlyDayOf(
  range: Range8b, from: string, to: string, firstIso: string, lastIso: string, now: Date = new Date(),
): string | null {
  const { fromIso, per } = windowFit(range, from, to, firstIso, lastIso, now);
  return per === 'hour' ? fromIso : null;
}

function weekKey(iso: string): string {
  const date = parseLocalDate(iso);
  date.setDate(date.getDate() - date.getDay()); // back to Sunday
  return isoOf(date);
}

// Every bucket key for [fromIso, toIso] at the given granularity, so idle
// periods render as zero bars instead of silently disappearing (which would
// also inflate the avg-per-bucket stat).
function allKeys(per: Granularity, fromIso: string, toIso: string): string[] {
  if (per === 'hour') {
    return Array.from({ length: 24 }, (_, h) => `${fromIso} ${String(h).padStart(2, '0')}:00`);
  }
  const keys: string[] = [];
  const d = parseLocalDate(fromIso);
  const end = parseLocalDate(toIso);
  if (per === 'week') d.setDate(d.getDate() - d.getDay());
  if (per === 'month') d.setDate(1);
  while (d <= end) {
    keys.push(per === 'month' ? isoOf(d).slice(0, 7) : isoOf(d));
    if (per === 'day') d.setDate(d.getDate() + 1);
    else if (per === 'week') d.setDate(d.getDate() + 7);
    else d.setMonth(d.getMonth() + 1);
  }
  return keys;
}

export function bucketsFromPoints(
  pts: SeriesPoint[],
  per: Granularity,
  fromIso?: string,
  toIso?: string,
  lang: Lang = 'en',
): Bucket[] {
  const keyOf = (p: SeriesPoint) =>
    per === 'hour' ? p.bucket
    : per === 'day' ? p.bucket.slice(0, 10)
    : per === 'week' ? weekKey(p.bucket.slice(0, 10))
    : p.bucket.slice(0, 7); // YYYY-MM
  const map = new Map<string, {
    byTool: Record<string, number>;
    byModel: Record<string, number>;
    modelSources: Record<string, string[]>;
    unattributedTokens: number;
    hasUnpriced: boolean;
  }>();
  for (const p of pts) {
    const k = keyOf(p);
    const g = map.get(k) ?? {
      byTool: emptyBySource(), byModel: {}, modelSources: {},
      unattributedTokens: 0, hasUnpriced: false,
    };
    g.byTool[p.source] = (g.byTool[p.source] ?? 0) + p.totalTokens;
    for (const [m, v] of Object.entries(p.byModel)) {
      g.byModel[m] = (g.byModel[m] ?? 0) + v;
      // Remember who produced it here, so a Model name shared by two Sources is
      // attributed per bucket rather than range-wide.
      const owners = (g.modelSources[m] ??= []);
      if (!owners.includes(p.source)) owners.push(p.source);
    }
    g.unattributedTokens += p.unattributedTokens;
    g.hasUnpriced ||= p.hasUnpriced;
    map.set(k, g);
  }
  const labelOf = (k: string) =>
    per === 'hour' ? String(parseInt(k.slice(11, 13), 10))
    : per === 'day' ? String(parseInt(k.slice(8, 10), 10))
    : per === 'month' ? monthShortL(parseInt(k.slice(5, 7), 10) - 1, lang)
    : k; // week: placeholder, renumbered below
  // Zero-fill the whole window when its bounds are known; fall back to
  // data-present keys otherwise.
  const keys = fromIso && toIso ? allKeys(per, fromIso, toIso) : [...map.keys()].sort();
  const out = keys.map((k) => {
    const g = map.get(k) ?? {
      byTool: emptyBySource(), byModel: {}, modelSources: {},
      unattributedTokens: 0, hasUnpriced: false,
    };
    return {
      key: k,
      label: labelOf(k),
      byTool: g.byTool,
      byModel: g.byModel,
      modelSources: g.modelSources,
      unattributedTokens: g.unattributedTokens,
      hasUnpriced: g.hasUnpriced,
      total: (Object.values(g.byTool) as number[]).reduce((a, b) => a + b, 0),
    };
  });
  if (per === 'week') out.forEach((b, i) => (b.label = 'W' + (i + 1)));
  return out;
}

export interface TrendSlice {
  rpts: SeriesPoint[];               // the window-filtered daily points
  trend: Bucket[];                   // stacked buckets at the window's granularity
  per: Granularity;
  modelTool: Record<string, string>;
  total: number;
}

// The trend derivation shared by the Overview and its enlarge: window the
// series, pick the granularity that fits, and bucket it — hourly buckets for a
// single-day window come from a separately-fetched hourPoints. `from`/`to` are
// the effective window bounds (the custom inputs, else firstIso/lastIso); they
// only matter for a custom range (windowOf ignores them for presets).
// `override` pins the bucket size in place of the automatic fit (the enlarge's
// interval selector); callers wanting the automatic rule pass nothing. Hour is
// excluded by type — hourly bucketing exists only via windowFit's single-day
// rule, which hourlyDayOf reads too, so hourPoints are there when it fires.
export function trendSlice(
  allPoints: SeriesPoint[],
  hourPoints: SeriesPoint[],
  range: Range8b,
  from: string,
  to: string,
  firstIso: string,
  lastIso: string,
  now: Date = new Date(),
  lang: Lang = 'en',
  override?: Exclude<Granularity, 'hour'>,
): TrendSlice {
  const { win, fromIso: winFrom, toIso: winTo, per: fit } = windowFit(range, from, to, firstIso, lastIso, now);
  const rpts = pointsIn(allPoints, win);
  const per = override ?? fit;
  const trend =
    per === 'hour'
      ? bucketsFromPoints(hourPoints, 'hour', winFrom, winTo, lang)
      : bucketsFromPoints(rpts, per, winFrom, winTo, lang);
  return { rpts, trend, per, modelTool: modelTools(rpts), total: sumPoints(rpts) };
}

// ---- aggregations ----

export function sumPoints(pts: SeriesPoint[]): number {
  return pts.reduce((a, p) => a + p.totalTokens, 0);
}

export function toolTotalsOfPoints(pts: SeriesPoint[]): Record<string, number> {
  const out = emptyBySource();
  for (const p of pts) out[p.source] = (out[p.source] ?? 0) + p.totalTokens;
  return out;
}

export interface SmallMultipleItem extends SourceMeta {
  total: number;
  share: number;
  series: number[];
}

// Per-tool sparkline series over the buckets (small multiples).
export function smallMultiples(bks: Bucket[]): SmallMultipleItem[] {
  const totals = emptyBySource();
  for (const b of bks) for (const [key, value] of Object.entries(b.byTool)) totals[key] = (totals[key] ?? 0) + value;
  const keys = orderedSourceKeys(Object.keys(totals));
  const grand = keys.reduce((total, key) => total + totals[key], 0) || 1;
  return keys.filter((key) => totals[key] > 0).map((key) => ({
    ...sourceMeta(key),
    total: totals[key],
    share: totals[key] / grand,
    series: bks.map((b) => b.byTool[key] ?? 0),
  }));
}

export interface CatTotals {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
}

export function catTotals(pts: SeriesPoint[], tool: SourceKey): CatTotals {
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
  // Sessions run in a repo's subdirs (easydrone/client, easydrone/server) get
  // their own cwd key — fold each into its direct parent row so a repo reads
  // as one project. Direct parent only: any-depth folding collapses every
  // project under a home-dir row when one session ran in ~.
  // ponytail: no chaining and no git-root lookup; revisit if repos nest deeper.
  const keyOf = (row: BreakdownRow) => row.key ?? 'unknown';
  const keySet = new Set(rows.map(keyOf));
  const rootOf = (key: string) => {
    const parent = key.slice(0, key.lastIndexOf('/'));
    return keySet.has(parent) ? parent : key;
  };

  const byRoot = new Map<string, TableRow>();
  for (const p of rows) {
    const label = rootOf(keyOf(p));
    const r = byRoot.get(label) ?? {
      label, total: 0, input: 0, output: 0, cached: 0, reasoning: null, convs: 0,
    };
    r.total += p.totalTokens;
    r.input += p.inputTokens;
    r.output += p.outputTokens;
    r.cached += p.cacheReadTokens;
    if (p.reasoningTokens != null) r.reasoning = (r.reasoning ?? 0) + p.reasoningTokens;
    // Sessions never span projects, so summing convs across folded rows is safe.
    r.convs += p.convs;
    byRoot.set(label, r);
  }
  return [...byRoot.values()];
}

// ---- models panel ----

export interface ModelBar {
  name: string | null;
  tokens: number;
  cost: number | null; // null = unpriced
  share: number;       // of the tool's range total
  segs: { key: string; color: string; frac: number }[];
  cacheEstimated: boolean;
}

export function modelBars(rows: BreakdownRow[], tool: SourceKey, toolTokens: number): ModelBar[] {
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

// ---- profile panel ----

export interface ProfileModel {
  name: string;
  tokens: number;
  share: number; // of ALL lifetime tokens, incl. Unattributed — never sums to 1
}

export interface ProfileView {
  d7: number;
  d30: number;
  perActiveDay: number | null; // null = no active day to divide by
  sessions30d: number | null;  // null = the 30-day Summary hasn't landed
  models: ProfileModel[];
  startedIso: string | null;   // null = nothing in the Ledger
  activeDays: number;
}

export const PROFILE_TOP_N = 5;

// The Profile: a portrait of the whole Ledger, deliberately blind to the
// Overview's date window and Source selection (like Activity). Everything but
// the Session count derives from the unbounded daily series the store already
// holds; `sessions30d` comes from a fixed-window Summary because distinct
// Sessions cannot be summed out of per-day counts (a Session spanning days is
// counted once per day it touches — see queries.rs).
//
// Model shares are measured against ALL lifetime tokens including Unattributed
// Usage, which is why they sum to less than 1: Unattributed has no Model to be
// (ADR-0008), so it holds a share without ever being a row.
export function profileView(
  pts: SeriesPoint[],
  sessions30d: number | null,
  today: Date,
): ProfileView {
  const end = new Date(today.getFullYear(), today.getMonth(), today.getDate());
  const backIso = (n: number) => {
    const d = new Date(end);
    d.setDate(d.getDate() - n);
    return isoOf(d);
  };
  // Trailing windows follow the range picker's own convention (data.ts
  // rangeWindow): 7d = today + the 6 before it.
  const from7 = backIso(6);
  const from30 = backIso(29);

  let d7 = 0;
  let d30 = 0;
  let lifetime = 0;
  let startedIso: string | null = null;
  const activeDays = new Set<string>();
  const byModel = new Map<string, number>();

  for (const p of pts) {
    if (p.totalTokens <= 0) continue; // a zero-token day is not an active day
    lifetime += p.totalTokens;
    activeDays.add(p.bucket);
    if (startedIso === null || p.bucket < startedIso) startedIso = p.bucket;
    if (p.bucket >= from7) d7 += p.totalTokens;
    if (p.bucket >= from30) d30 += p.totalTokens;
    // byModel already merges a Model's tokens across Sources within a bucket;
    // a Model is its raw logged name, not a (name, Source) pair.
    for (const [m, tokens] of Object.entries(p.byModel)) {
      byModel.set(m, (byModel.get(m) ?? 0) + tokens);
    }
  }

  const models = [...byModel.entries()]
    .map(([name, tokens]) => ({ name, tokens, share: tokens / Math.max(1, lifetime) }))
    .sort((a, b) => b.tokens - a.tokens || a.name.localeCompare(b.name))
    .slice(0, PROFILE_TOP_N);

  return {
    d7,
    d30,
    perActiveDay: activeDays.size > 0 ? lifetime / activeDays.size : null,
    sessions30d,
    models,
    startedIso,
    activeDays: activeDays.size,
  };
}

// ---- context breakdown panel ----

export interface CtxTotals {
  billed: number; // input + cacheRead + cacheWrite — the context transmitted
  reused: number; // cacheRead
  messages: number | null;
  system: number | null;
  reasoning: number | null;
  toolcalls: number | null;
  agents: number | null;
  mcp: number | null;
  skills: number | null;
}

// Null-preserving sum: a null contributor never zeroes the total, and a
// category nobody reported stays null (renders "—", same rule as reasoning).
function addOpt(a: number | null, b: number | null): number | null {
  return b == null ? a : (a ?? 0) + b;
}

export function ctxTotals(pts: SeriesPoint[], tool: SourceKey): CtxTotals {
  const t: CtxTotals = {
    billed: 0, reused: 0,
    messages: null, system: null, reasoning: null,
    toolcalls: null, agents: null, mcp: null, skills: null,
  };
  for (const p of pts) {
    if (p.source !== tool) continue;
    t.billed += p.inputTokens + p.cacheReadTokens + p.cacheWriteTokens;
    t.reused += p.cacheReadTokens;
    t.messages = addOpt(t.messages, p.ctxMessages);
    t.system = addOpt(t.system, p.ctxSystem);
    t.reasoning = addOpt(t.reasoning, p.ctxReasoning);
    t.toolcalls = addOpt(t.toolcalls, p.ctxToolcalls);
    t.agents = addOpt(t.agents, p.ctxAgents);
    t.mcp = addOpt(t.mcp, p.ctxMcp);
    t.skills = addOpt(t.skills, p.ctxSkills);
  }
  return t;
}

const CTX_KINDS: { kind: string; key: OverviewKey }[] = [
  { kind: 'skill', key: 'overview.kind.skill' },
  { kind: 'mcp_server', key: 'overview.kind.mcpServer' },
  { kind: 'agent', key: 'overview.kind.agent' },
  { kind: 'memory_file', key: 'overview.kind.memoryFile' },
];

export function ctxMeta(res: CtxResource[], tool: SourceKey, lang: Lang = 'en'): string {
  const bits: string[] = [];
  for (const { kind, key } of CTX_KINDS) {
    // Rows are distinct (source, kind, name), so the kind's count is its row count.
    const n = res.filter((r) => r.source === tool && r.kind === kind).length;
    // English pluralises the kind word; Chinese has no plural.
    if (n > 0) bits.push(`${n} ${overviewT(lang, key)}${lang === 'zh-Hant' ? '' : n === 1 ? '' : 's'}`);
  }
  return bits.join(' · ');
}

// ---- tool drill-down (spec 2026-07-10-context-drilldown) ----

// An MCP tool is named `mcp__<server>__<tool>`, so every call it makes says
// which server served it. Null for anything that is not an MCP tool.
function mcpServer(name: string): string | null {
  if (!name.startsWith('mcp__')) return null;
  return name.split('__')[1] || 'unknown';
}

// Category map mirrors TokenTracker's, trimmed to names seen in our logs.
export function categorizeTool(name: string): string {
  if (name === 'Task' || name === 'Agent') return 'Agent';
  if (/^Task(Create|Update|Get|List|Output|Stop)$/.test(name) || name.startsWith('Todo')) return 'Task Mgmt';
  if (/^(Read|Write|Edit|Glob)$/.test(name)) return 'File Ops';
  if (name === 'Grep') return 'Search';
  if (name === 'Bash') return 'Execution';
  if (/^Web(Fetch|Search)$/.test(name)) return 'Web';
  const server = mcpServer(name);
  if (server) return `MCP: ${server}`;
  if (name === 'Skill') return 'Skill';
  return 'Other';
}

// Largest-remainder integer allocation: results sum exactly to total.
// Ties broken by key ascending for determinism.
export function allocateByWeight(
  total: number,
  entries: { key: string; weight: number }[],
): Map<string, number> {
  const out = new Map<string, number>();
  const W = entries.reduce((a, e) => a + Math.max(0, e.weight), 0);
  if (W <= 0 || total <= 0) {
    for (const e of entries) out.set(e.key, 0);
    return out;
  }
  let allocated = 0;
  const rems: { key: string; rem: number }[] = [];
  for (const e of entries) {
    const exact = (total * Math.max(0, e.weight)) / W;
    const base = Math.floor(exact);
    out.set(e.key, base);
    allocated += base;
    rems.push({ key: e.key, rem: exact - base });
  }
  rems.sort((a, b) => b.rem - a.rem || (a.key < b.key ? -1 : 1));
  for (let i = 0; i < total - allocated; i++) {
    const k = rems[i % rems.length].key;
    out.set(k, (out.get(k) ?? 0) + 1);
  }
  return out;
}

// One expanded Skills row. `rest` > 0 marks the folded remainder instead of a
// skill, so the label stays the component's (and the translator's) business.
export interface SkillBar { name: string; tokens: number; uses: number; rest: number }

// The window's skills for one Source, heaviest first — the query already sorts,
// so this only scopes and folds. Everything past `top` collapses into one
// remainder row rather than running the panel off the card.
export function skillBars(rows: CtxSkillRow[], source: string, top = 10): SkillBar[] {
  const mine = rows.filter((r) => r.source === source);
  const out: SkillBar[] = mine
    .slice(0, top)
    .map((r) => ({ name: r.name, tokens: r.estTokens, uses: r.uses, rest: 0 }));
  const folded = mine.slice(top);
  if (folded.length) {
    out.push({
      name: '',
      tokens: folded.reduce((a, r) => a + r.estTokens, 0),
      uses: folded.reduce((a, r) => a + r.uses, 0),
      rest: folded.length,
    });
  }
  return out;
}

// One expanded MCP servers row. Traffic only: the tool definitions a server
// publishes reach the model inside the system prompt, which names no owner,
// so they are not in `tokens`.
export interface McpBar { name: string; tokens: number; calls: number }

// The window's MCP servers for the selected Source, heaviest first. Every
// token the MCP category counts comes from an `mcp__*` tool row and nothing
// else does, so allocating that total by the stored weights is exact — the
// servers sum back to the row they hang under, like the tool tree's levels.
export function mcpBars(rows: CtxToolRow[], mcpTotal: number | null): McpBar[] {
  // Falsy covers both "the Source cannot attribute MCP" and "it attributed
  // nothing", which would otherwise expand into a list of zeros.
  if (!mcpTotal) return [];
  const byServer = new Map<string, { weight: number; calls: number }>();
  for (const r of rows) {
    const server = mcpServer(r.name);
    if (!server) continue;
    const acc = byServer.get(server) ?? { weight: 0, calls: 0 };
    acc.weight += r.estTokens;
    acc.calls += r.calls;
    byServer.set(server, acc);
  }
  if (byServer.size === 0) return [];
  const tokens = allocateByWeight(
    mcpTotal,
    [...byServer].map(([key, a]) => ({ key, weight: a.weight })),
  );
  return [...byServer]
    .map(([name, a]) => ({ name, tokens: tokens.get(name) ?? 0, calls: a.calls }))
    .sort((a, b) => b.tokens - a.tokens || (a.name < b.name ? -1 : 1));
}

export interface ToolLeaf { name: string; tokens: number; calls: number }
export interface ToolCategory { label: string; tokens: number; tools: ToolLeaf[] }

// Allocate the estimated Tool-calls total down category → tool by the stored
// content weights, so children sum to their parent at both levels.
export function toolTree(rows: CtxToolRow[], toolcallsTotal: number | null): ToolCategory[] {
  if (toolcallsTotal == null || rows.length === 0) return [];
  const byCat = new Map<string, CtxToolRow[]>();
  for (const r of rows) {
    const cat = categorizeTool(r.name);
    const arr = byCat.get(cat);
    if (arr) arr.push(r);
    else byCat.set(cat, [r]);
  }
  const catWeights = [...byCat.entries()].map(([label, rs]) => ({
    key: label,
    weight: rs.reduce((a, r) => a + r.estTokens, 0),
  }));
  const catTokens = allocateByWeight(toolcallsTotal, catWeights);
  const tree = [...byCat.entries()].map(([label, rs]) => {
    const tokens = catTokens.get(label) ?? 0;
    const leafTokens = allocateByWeight(
      tokens,
      rs.map((r) => ({ key: r.name, weight: r.estTokens })),
    );
    const tools = rs
      .map((r) => ({ name: r.name, tokens: leafTokens.get(r.name) ?? 0, calls: r.calls }))
      .sort((a, b) => b.tokens - a.tokens);
    return { label, tokens, tools };
  });
  return tree.sort((a, b) => b.tokens - a.tokens);
}

// ---- exact usage buckets ----

export interface BucketView {
  total: number;
  messages: number;
  history: number;
  newInput: number;
  response: number;
  system: number | null;
  reasoning: number | null;
}

export function bucketView(b: CtxBuckets | null): BucketView | null {
  if (!b) return null;
  const messages = b.history + b.newInput + b.response;
  return {
    total: messages + (b.system ?? 0) + (b.reasoning ?? 0),
    messages,
    history: b.history,
    newInput: b.newInput,
    response: b.response,
    system: b.system,
    reasoning: b.reasoning,
  };
}

// ---- Bash exec facets (spec 2026-07-10-bash-exec-drilldown) ----

export interface ExecFacetRow { key: string; tokens: number; calls: number }
export interface ExecFacets {
  byType: ExecFacetRow[];
  byExecutable: ExecFacetRow[];
  byCommand: ExecFacetRow[];
}

function facetOf(rows: CtxExecRow[], keyOf: (r: CtxExecRow) => string, total: number): ExecFacetRow[] {
  const groups = new Map<string, { weight: number; calls: number }>();
  for (const r of rows) {
    const k = keyOf(r);
    const g = groups.get(k) ?? { weight: 0, calls: 0 };
    g.weight += r.estTokens;
    g.calls += r.calls;
    groups.set(k, g);
  }
  const alloc = allocateByWeight(
    total,
    [...groups.entries()].map(([key, g]) => ({ key, weight: g.weight })),
  );
  return [...groups.entries()]
    .map(([key, g]) => ({ key, tokens: alloc.get(key) ?? 0, calls: g.calls }))
    .sort((a, b) => b.tokens - a.tokens);
}

// Three parallel views over the same rows; each facet's tokens sum exactly
// to the Bash leaf's allocated total.
export function execFacets(rows: CtxExecRow[], bashTotal: number | null): ExecFacets | null {
  if (bashTotal == null || rows.length === 0) return null;
  return {
    byType: facetOf(rows, (r) => r.kind, bashTotal),
    byExecutable: facetOf(rows, (r) => r.exe, bashTotal),
    byCommand: facetOf(rows, (r) => r.cmd, bashTotal),
  };
}
