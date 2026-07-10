// Real-data layer for the Overview: shared design meta plus pure reshaping of
// backend responses (SeriesPoint/BreakdownRow) into the shapes the components
// consume. No fetching here — Overview8b orchestrates IPC calls.
import type { BreakdownRow, Filters, SeriesPoint, DateRange, CtxResourceCount, CtxBuckets, CtxToolRow } from '../types';
import { rangeToBounds, parseLocalDate } from '../lib/dateRange';

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
    let tokens = 0;
    for (const p of byDate.get(iso) ?? []) {
      if (p.source in byTool) byTool[p.source as ToolKey] += p.totalTokens;
      tokens += p.totalTokens;
    }
    const cell = i + startDow;
    days.push({
      index: i, date, iso, weekday: date.getDay(),
      col: Math.floor(cell / 7), row: cell % 7,
      tokens, level: 0, byTool,
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
  if (range === 'custom') {
    // Normalize a reversed range exactly like windowOf does, so server-fetched
    // panels (summary, breakdowns) always agree with the client-sliced ones.
    const lo = customFrom <= customTo ? customFrom : customTo;
    const hi = customFrom <= customTo ? customTo : customFrom;
    return { tools: [], models: [], project: null, ...rangeToBounds({ start: lo, end: hi }) };
  }
  const dr: DateRange =
    range === 'day' ? 'today' : range === 'week' ? '7d' : range === 'month' ? '30d' : 'all';
  return { tools: [], models: [], project: null, ...rangeToBounds(dr) };
}

// ---- trend buckets ----

export type Granularity = 'hour' | 'day' | 'week' | 'month';
export const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

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
): Bucket[] {
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
  // Zero-fill the whole window when its bounds are known; fall back to
  // data-present keys otherwise.
  const keys = fromIso && toIso ? allKeys(per, fromIso, toIso) : [...map.keys()].sort();
  const out = keys.map((k) => {
    const by = map.get(k) ?? emptyByTool();
    return {
      label: labelOf(k),
      byTool: by,
      total: (Object.values(by) as number[]).reduce((a, b) => a + b, 0),
    };
  });
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

const CTX_KINDS: { kind: string; label: string }[] = [
  { kind: 'skill', label: 'skill' },
  { kind: 'mcp_server', label: 'MCP server' },
  { kind: 'agent', label: 'agent' },
  { kind: 'memory_file', label: 'memory file' },
];

export function ctxMeta(res: CtxResourceCount[], tool: ToolKey): string {
  const bits: string[] = [];
  for (const { kind, label } of CTX_KINDS) {
    const n = res.find((r) => r.source === tool && r.kind === kind)?.count ?? 0;
    if (n > 0) bits.push(`${n} ${label}${n === 1 ? '' : 's'}`);
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
