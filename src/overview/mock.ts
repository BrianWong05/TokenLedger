// Mock data for the "App · Overview" design (canvas 8a).
// Deterministic (seeded PRNG) so the dashboard is stable across renders.
// This is fake data purely to fill out the design — nothing here reads the
// real Ledger. Category names follow CONTEXT.md's ubiquitous language.

import {
  TOOLS, CATEGORIES,
  type Day, type ToolKey, type Bucket, type TableRow, type Range8b,
} from './data';

export {
  TOOLS, CATEGORIES, THEMES, THEME_OPTIONS, RANGES_8B,
  type ToolKey, type ToolMeta, type Day, type Bucket, type TableRow, type Range8b,
} from './data';
export { fmtTok, fmtUSD, fmtPct, fmtDate, fmtIsoDate } from '../lib/format';

const YEAR = 2025;
const BLENDED_USD_PER_MTOK = 2.75; // est. blended list price, $/M tokens

function mulberry32(seed: number) {
  let a = seed;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
const rng = mulberry32(20250109);

function isoOf(d: Date): string {
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return `${d.getFullYear()}-${m}-${day}`;
}

function levelOf(t: number): 0 | 1 | 2 | 3 | 4 {
  if (t <= 0) return 0;
  if (t < 250_000) return 1;
  if (t < 550_000) return 2;
  if (t < 950_000) return 3;
  return 4;
}

function splitTools(tokens: number): Record<ToolKey, number> {
  const weights = TOOLS.map(() => rng() ** 1.8);
  const sum = weights.reduce((a, b) => a + b, 0) || 1;
  const out = {} as Record<ToolKey, number>;
  TOOLS.forEach((t, i) => (out[t.key] = Math.round((weights[i] / sum) * tokens)));
  return out;
}

function buildDays(): Day[] {
  const startDow = new Date(YEAR, 0, 1).getDay();
  const days: Day[] = [];
  for (let i = 0; i < 365; i++) {
    const date = new Date(YEAR, 0, 1 + i);
    const weekday = date.getDay();
    const cell = i + startDow;

    // usage shape: weekday-heavy, gentle seasonal ramp, off-days and spikes
    const season = 0.55 + 0.45 * Math.sin((i / 365) * Math.PI * 1.4);
    const weekendDamp = weekday === 0 || weekday === 6 ? 0.32 : 1;
    let tokens = 0;
    if (rng() > 0.12) {
      const spike = rng() > 0.94 ? 2.4 : 1;
      tokens = Math.round((120_000 + rng() * 1_050_000) * season * weekendDamp * spike);
    }

    days.push({
      index: i,
      date,
      iso: isoOf(date),
      weekday,
      col: Math.floor(cell / 7),
      row: cell % 7,
      tokens,
      cost: costOf(tokens),
      level: levelOf(tokens),
      byTool: splitTools(tokens),
    });
  }
  return days;
}

export const DAYS = buildDays();
export const COLS = Math.max(...DAYS.map((d) => d.col)) + 1;

export const TOTAL_TOKENS = DAYS.reduce((a, d) => a + d.tokens, 0);
export const ACTIVE_DAYS = DAYS.filter((d) => d.tokens > 0).length;
export const BEST_DAY = DAYS.reduce((a, d) => (d.tokens > a.tokens ? d : a), DAYS[0]);

export const LONGEST_STREAK = (() => {
  let best = 0;
  let run = 0;
  for (const d of DAYS) {
    if (d.tokens > 0) {
      run += 1;
      best = Math.max(best, run);
    } else run = 0;
  }
  return best;
})();

export const TOOL_TOTALS = TOOLS.reduce((acc, t) => {
  acc[t.key] = DAYS.reduce((s, d) => s + d.byTool[t.key], 0);
  return acc;
}, {} as Record<ToolKey, number>);

// Per-tool category mix (input, output, cacheRead, cacheWrite).
const CAT_MIX: Record<ToolKey, [number, number, number, number]> = {
  claude: [0.18, 0.14, 0.55, 0.13],
  codex: [0.3, 0.2, 0.42, 0.08],
  gemini: [0.26, 0.17, 0.49, 0.08],
  hermes: [0.4, 0.3, 0.22, 0.08],
};

export function categorySplit(tool: ToolKey, tokens: number) {
  const mix = CAT_MIX[tool];
  return CATEGORIES.map((c, i) => ({ ...c, tokens: Math.round(tokens * mix[i]) }));
}

// Context-window breakdown (Context tab): what actually occupies the model's
// context. Primary rows are the token-heavy contents (sum ≈ input); secondary
// rows are supplementary context sources shown as raw volumes.
const CTX_PRIMARY = [
  { key: 'messages', label: 'Messages', frac: 0.96, expand: true },
  { key: 'system', label: 'System prompt', frac: 0.027, info: true },
  { key: 'reasoning', label: 'Reasoning', frac: 0.013 },
] as const;

const CTX_SECONDARY = [
  { key: 'toolcalls', label: 'Tool calls', frac: 0.041 },
  { key: 'agents', label: 'Custom agents', frac: 0.006 },
  { key: 'mcp', label: 'MCP servers', frac: 0.011 },
  { key: 'skills', label: 'Skills', frac: 0.0004 },
] as const;

// Configured resources per source (drives the footer + which secondary rows show).
const CTX_RESOURCES: Record<ToolKey, { skills: number; mcp: number; agents: number; memory: number }> = {
  claude: { skills: 32, mcp: 2, agents: 1, memory: 1 },
  codex: { skills: 0, mcp: 1, agents: 0, memory: 1 },
  gemini: { skills: 0, mcp: 1, agents: 0, memory: 0 },
  hermes: { skills: 4, mcp: 0, agents: 2, memory: 1 },
};

function plural(n: number, one: string): string {
  return `${n} ${n === 1 ? one : one + 's'}`;
}

export function contextBreakdown(tool: ToolKey, toolTokens: number = TOOL_TOTALS[tool]) {
  const [fresh, , cacheRead] = categorySplit(tool, toolTokens).map((c) => c.tokens);
  const input = fresh + cacheRead; // total context in the window
  const reused = cacheRead;
  const r = CTX_RESOURCES[tool];
  const present: Record<string, boolean> = {
    toolcalls: true,
    agents: r.agents > 0,
    mcp: r.mcp > 0,
    skills: r.skills > 0,
  };
  const metaBits: string[] = [];
  if (r.skills) metaBits.push(plural(r.skills, 'skill'));
  if (r.mcp) metaBits.push(plural(r.mcp, 'MCP server'));
  if (r.agents) metaBits.push(plural(r.agents, 'agent'));
  if (r.memory) metaBits.push(plural(r.memory, 'memory file'));
  return {
    input,
    reused,
    cacheHit: input ? reused / input : 0,
    primary: CTX_PRIMARY.map((p) => {
      const tokens = Math.round(input * p.frac);
      return { ...p, tokens, pct: tokens / input };
    }),
    secondary: CTX_SECONDARY.filter((s) => present[s.key]).map((s) => ({
      ...s,
      tokens: Math.round(input * s.frac),
    })),
    meta: metaBits.join(' · '),
  };
}

export const MODELS: Record<ToolKey, { name: string; share: number }[]> = {
  claude: [
    { name: 'claude-opus-4-8', share: 0.52 },
    { name: 'claude-sonnet-5', share: 0.34 },
    { name: 'claude-haiku-4-5', share: 0.14 },
  ],
  codex: [
    { name: 'gpt-5.4', share: 0.68 },
    { name: 'gpt-5.4-mini', share: 0.32 },
  ],
  gemini: [
    { name: 'gemini-2.5-pro', share: 0.6 },
    { name: 'gemini-2.5-flash', share: 0.4 },
  ],
  hermes: [
    { name: 'llama-3.3-70b', share: 0.57 },
    { name: 'mixtral-8x22b', share: 0.43 },
  ],
};

// ---- usage-trend buckets, stacked by tool ----

export type Interval = 'D' | 'W' | 'M' | 'Q';
export const INTERVALS: { key: Interval; label: string; per: string }[] = [
  { key: 'D', label: 'Day', per: 'day' },
  { key: 'W', label: 'Week', per: 'week' },
  { key: 'M', label: 'Month', per: 'month' },
  { key: 'Q', label: 'Quarter', per: 'quarter' },
];

function emptyByTool(): Record<ToolKey, number> {
  return { claude: 0, codex: 0, gemini: 0, hermes: 0 };
}
function addInto(dst: Record<ToolKey, number>, src: Record<ToolKey, number>) {
  for (const t of TOOLS) dst[t.key] += src[t.key];
}
function finalize(label: string, by: Record<ToolKey, number>): Bucket {
  return { label, byTool: by, total: TOOLS.reduce((s, t) => s + by[t.key], 0) };
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

export function buckets(interval: Interval): Bucket[] {
  if (interval === 'D') {
    return DAYS.slice(-14).map((d) => finalize(String(d.date.getDate()), { ...d.byTool }));
  }
  if (interval === 'M') {
    const arr = Array.from({ length: 12 }, emptyByTool);
    for (const d of DAYS) addInto(arr[d.date.getMonth()], d.byTool);
    return arr.map((by, i) => finalize(MONTHS[i], by));
  }
  if (interval === 'Q') {
    const arr = Array.from({ length: 4 }, emptyByTool);
    for (const d of DAYS) addInto(arr[Math.floor(d.date.getMonth() / 3)], d.byTool);
    return arr.map((by, i) => finalize('Q' + (i + 1), by));
  }
  // weekly: group by heatmap column (week index), show the last 10
  const byWeek = new Map<number, Record<ToolKey, number>>();
  for (const d of DAYS) {
    if (!byWeek.has(d.col)) byWeek.set(d.col, emptyByTool());
    addInto(byWeek.get(d.col)!, d.byTool);
  }
  return [...byWeek.entries()]
    .sort((a, b) => a[0] - b[0])
    .slice(-10)
    .map(([col, by]) => finalize('W' + (col + 1), by));
}

// ---- date ranges (8b overview) ----

const RANGE_LEN: Record<Range8b, number> = { day: 1, week: 7, month: 30, total: 365, custom: 0 };

// "today" = the most recent day with activity, so the Day view is never empty.
export const TODAY: Day = [...DAYS].reverse().find((d) => d.tokens > 0) ?? DAYS[DAYS.length - 1];

export function sliceDays(range: Range8b): Day[] {
  if (range === 'day') return [TODAY];
  return DAYS.slice(Math.max(0, DAYS.length - RANGE_LEN[range]));
}

export function daysBetween(fromIso: string, toIso: string): Day[] {
  const lo = fromIso <= toIso ? fromIso : toIso;
  const hi = fromIso <= toIso ? toIso : fromIso;
  return DAYS.filter((d) => d.iso >= lo && d.iso <= hi);
}

export const FIRST_ISO = DAYS[0].iso;
export const LAST_ISO = DAYS[DAYS.length - 1].iso;
export function sumTokens(days: Day[]): number {
  return days.reduce((a, d) => a + d.tokens, 0);
}
export function toolTotalsOf(days: Day[]): Record<ToolKey, number> {
  const out = emptyByTool();
  for (const d of days) addInto(out, d.byTool);
  return out;
}

function dailyBuckets(days: Day[]): Bucket[] {
  return days.map((d) => finalize(String(d.date.getDate()), { ...d.byTool }));
}
function weeklyBuckets(days: Day[]): Bucket[] {
  const byWeek = new Map<number, Record<ToolKey, number>>();
  for (const d of days) {
    if (!byWeek.has(d.col)) byWeek.set(d.col, emptyByTool());
    addInto(byWeek.get(d.col)!, d.byTool);
  }
  return [...byWeek.entries()].sort((a, b) => a[0] - b[0]).map(([col, by]) => finalize('W' + (col + 1), by));
}
function monthlyBuckets(days: Day[]): Bucket[] {
  const byMonth = new Map<number, Record<ToolKey, number>>();
  for (const d of days) {
    const m = d.date.getMonth();
    if (!byMonth.has(m)) byMonth.set(m, emptyByTool());
    addInto(byMonth.get(m)!, d.byTool);
  }
  return [...byMonth.entries()].sort((a, b) => a[0] - b[0]).map(([m, by]) => finalize(MONTHS[m], by));
}

// diurnal shape for the single-day (hourly) view
const DIURNAL = [2, 1, 1, 1, 2, 3, 6, 12, 18, 22, 24, 23, 25, 26, 24, 22, 20, 16, 12, 9, 7, 5, 4, 3];
function hourlyBuckets(day: Day): Bucket[] {
  const wsum = DIURNAL.reduce((a, b) => a + b, 0);
  return DIURNAL.map((w, h) => {
    const by = emptyByTool();
    for (const t of TOOLS) by[t.key] = Math.round((day.byTool[t.key] * w) / wsum);
    return finalize(String(h), by);
  });
}

// stacked-by-tool buckets; granularity follows the range (hourly → monthly)
export function bucketsOf(days: Day[], range: Range8b): Bucket[] {
  if (range === 'day') return hourlyBuckets(days[0] ?? TODAY);
  if (range === 'week' || range === 'month') return dailyBuckets(days);
  if (range === 'total') return monthlyBuckets(days);
  // custom: pick granularity by span
  if (days.length <= 31) return dailyBuckets(days);
  if (days.length <= 120) return weeklyBuckets(days);
  return monthlyBuckets(days);
}

export function perOf(range: Range8b, count: number): string {
  if (range === 'day') return 'hour';
  if (range === 'week' || range === 'month') return 'day';
  if (range === 'total') return 'month';
  return count <= 31 ? 'day' : count <= 120 ? 'week' : 'month';
}

// per-tool sparkline series over the range's buckets (small multiples)
export function smallMultiples(days: Day[], range: Range8b) {
  const bks = bucketsOf(days, range);
  const totals = toolTotalsOf(days);
  const grand = sumTokens(days) || 1;
  return TOOLS.map((t) => ({
    ...t,
    total: totals[t.key],
    share: totals[t.key] / grand,
    series: bks.map((b) => b.byTool[t.key]),
  }));
}

// ---- daily / project breakdown table (8b) ----

// Projects (working directories) the usage rolls up to. Weights are relative;
// project totals are apportioned from the range total so they stay coherent.
const PROJECTS: { name: string; weight: number }[] = [
  { name: 'token-ledger-web', weight: 26 },
  { name: 'agent-runtime', weight: 18.5 },
  { name: 'infra-terraform', weight: 13.5 },
  { name: 'ml-eval-harness', weight: 9.6 },
  { name: 'design-system', weight: 8.4 },
  { name: 'docs-portal', weight: 5.9 },
  { name: 'mobile-client', weight: 3.8 },
  { name: 'data-pipeline', weight: 4.3 },
];

// Split a total into the table's component columns. Columns don't fully sum to
// total (cache-write is omitted), matching the design.
function tableCats(total: number): Omit<TableRow, 'label'> {
  return {
    total,
    input: Math.round(total * 0.15),
    output: Math.round(total * 0.1),
    cached: Math.round(total * 0.72),
    reasoning: Math.round(total * 0.015),
    convs: Math.max(1, Math.round(total / 550_000)),
  };
}

export function dailyTableRows(days: Day[]): TableRow[] {
  return days.filter((d) => d.tokens > 0).map((d) => ({ label: d.iso, ...tableCats(d.tokens) }));
}

export function projectTableRows(total: number): TableRow[] {
  const sumW = PROJECTS.reduce((a, p) => a + p.weight, 0);
  return PROJECTS.map((p) => ({ label: p.name, ...tableCats(Math.round((total * p.weight) / sumW)) }));
}

// ---- cost helpers ----

export function costOf(tokens: number): number {
  return (tokens / 1e6) * BLENDED_USD_PER_MTOK;
}
export const TOTAL_COST = costOf(TOTAL_TOKENS);

// 8a adapter: fake ModelBar rows from the static MODELS shares.
export function mockModelBars(tool: ToolKey, toolTokens: number) {
  return MODELS[tool].map((m) => {
    const tokens = Math.round(toolTokens * m.share);
    const segs = categorySplit(tool, tokens);
    const segTotal = Math.max(1, segs.reduce((a, c) => a + c.tokens, 0));
    return {
      name: m.name,
      tokens,
      cost: costOf(tokens),
      share: m.share,
      segs: segs.map((c) => ({ key: c.key, color: c.color, frac: c.tokens / segTotal })),
      cacheEstimated: false,
    };
  });
}
