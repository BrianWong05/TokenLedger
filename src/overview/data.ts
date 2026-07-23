// Real-data layer for the Overview: pure reshaping of backend responses
// (SeriesPoint/BreakdownRow) into the shapes the components consume. No fetching
// here — overviewStore orchestrates the Ledger reads. Design meta (TOOLS,
// CATEGORIES, themes, ranges, types) lives in ./meta.
import type { BreakdownRow, Filters, SeriesPoint, CtxResourceCount, CtxBuckets, CtxToolRow, CtxExecRow } from '../types';
import { parseLocalDate } from '../lib/dateRange';
import { TOOLS, CATEGORIES, emptyByTool, type ToolKey, type ToolMeta, type Range8b } from './meta';
import type { Lang } from '../lib/i18n';
import { monthShortL, overviewT, type OverviewKey } from './localize';

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

// A model's stack color: the brand accent of its owning tool (grey fallback for
// an unknown source). Shared by the trend card and its enlarge.
export function modelColor(modelTool: Record<string, string>, m: string): string {
  return TOOLS.find((t) => t.key === modelTool[m])?.color ?? '#5f6880';
}

// Models ordered for a stacked bar: grouped by owning tool (in TOOLS order),
// largest-first within each tool. The stable sort over rankModels keeps the
// largest-first order inside each tool block, so bars read as contiguous tool
// blocks. Shared by the trend card and its enlarge.
export function stackModels(bks: Bucket[], modelTool: Record<string, string>): string[] {
  const toolIdx = (m: string) => {
    const i = TOOLS.findIndex((t) => t.key === modelTool[m]);
    return i < 0 ? TOOLS.length : i;
  };
  return rankModels(bks).sort((a, b) => toolIdx(a) - toolIdx(b));
}

// Model -> owning tool, from the raw points. Models don't span sources in
// practice, so last-write-wins is fine.
export function modelTools(pts: SeriesPoint[]): Record<string, string> {
  const out: Record<string, string> = {};
  for (const p of pts) for (const m of Object.keys(p.byModel)) out[m] = p.source;
  return out;
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
  byTool: Record<ToolKey, number>;
  byModel: Record<string, number>;
}

export interface Bucket {
  key: string;  // bucket key: iso day, 'YYYY-MM-DD HH:00', week-start iso, or 'YYYY-MM'
  label: string;
  byTool: Record<ToolKey, number>;
  byModel: Record<string, number>;
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
    const byTool = emptyByTool();
    const byModel: Record<string, number> = {};
    let tokens = 0;
    for (const p of byDate.get(iso) ?? []) {
      if (p.source in byTool) byTool[p.source as ToolKey] += p.totalTokens;
      for (const [m, v] of Object.entries(p.byModel)) byModel[m] = (byModel[m] ?? 0) + v;
      tokens += p.totalTokens;
    }
    const cell = i + startDow;
    days.push({
      index: i, date, iso, weekday: date.getDay(),
      col: Math.floor(cell / 7), row: cell % 7,
      tokens, level: 0, byTool, byModel,
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

// ---- trend buckets ----

export type Granularity = 'hour' | 'day' | 'week' | 'month';

// Adaptive granularity: hourly for a single day, daily up to a month,
// weekly up to ~a quarter, monthly beyond.
export function granularityOf(range: Range8b, spanDays: number): Granularity {
  if (range === 'day') return 'hour';
  if (range === 'week' || range === 'month') return 'day';
  return spanDays <= 31 ? 'day' : spanDays <= 120 ? 'week' : 'month';
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
  const map = new Map<string, { byTool: Record<ToolKey, number>; byModel: Record<string, number> }>();
  for (const p of pts) {
    const k = keyOf(p);
    const g = map.get(k) ?? { byTool: emptyByTool(), byModel: {} };
    if (p.source in g.byTool) g.byTool[p.source as ToolKey] += p.totalTokens;
    for (const [m, v] of Object.entries(p.byModel)) g.byModel[m] = (g.byModel[m] ?? 0) + v;
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
    const g = map.get(k) ?? { byTool: emptyByTool(), byModel: {} };
    return {
      key: k,
      label: labelOf(k),
      byTool: g.byTool,
      byModel: g.byModel,
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
): TrendSlice {
  const win = windowOf(range, from, to, now);
  const rpts = pointsIn(allPoints, win);
  const winFrom = win.fromIso ?? firstIso;
  const winTo = win.toIso ?? lastIso;
  const per = granularityOf(range, calendarSpan(winFrom, winTo));
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

export function toolTotalsOfPoints(pts: SeriesPoint[]): Record<ToolKey, number> {
  const out = emptyByTool();
  for (const p of pts) if (p.source in out) out[p.source as ToolKey] += p.totalTokens;
  return out;
}

export interface SmallMultipleItem extends ToolMeta {
  total: number;
  share: number;
  series: number[];
}

// Per-tool sparkline series over the buckets (small multiples).
export function smallMultiples(bks: Bucket[]): SmallMultipleItem[] {
  const totals = emptyByTool();
  for (const b of bks) for (const t of TOOLS) totals[t.key] += b.byTool[t.key];
  const grand = (Object.values(totals) as number[]).reduce((a, b) => a + b, 0) || 1;
  return TOOLS.filter((t) => totals[t.key] > 0).map((t) => ({
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

export function ctxTotals(pts: SeriesPoint[], tool: ToolKey): CtxTotals {
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

export function ctxMeta(res: CtxResourceCount[], tool: ToolKey, lang: Lang = 'en'): string {
  const bits: string[] = [];
  for (const { kind, key } of CTX_KINDS) {
    const n = res.find((r) => r.source === tool && r.kind === kind)?.count ?? 0;
    // English pluralises the kind word; Chinese has no plural.
    if (n > 0) bits.push(`${n} ${overviewT(lang, key)}${lang === 'zh-Hant' ? '' : n === 1 ? '' : 's'}`);
  }
  return bits.join(' · ');
}

// ---- tool drill-down (spec 2026-07-10-context-drilldown) ----

// Category map mirrors TokenTracker's, trimmed to names seen in our logs.
export function categorizeTool(name: string): string {
  if (name === 'Task' || name === 'Agent') return 'Agent';
  if (/^Task(Create|Update|Get|List|Output|Stop)$/.test(name) || name.startsWith('Todo')) return 'Task Mgmt';
  if (/^(Read|Write|Edit|Glob)$/.test(name)) return 'File Ops';
  if (name === 'Grep') return 'Search';
  if (name === 'Bash') return 'Execution';
  if (/^Web(Fetch|Search)$/.test(name)) return 'Web';
  if (name.startsWith('mcp__')) {
    const server = name.split('__')[1] ?? 'unknown';
    return `MCP: ${server}`;
  }
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
