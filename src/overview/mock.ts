// Mock data for the "App · Overview" design (canvas 8a).
// Deterministic (seeded PRNG) so the dashboard is stable across renders.
// This is fake data purely to fill out the design — nothing here reads the
// real Ledger. Category names follow CONTEXT.md's ubiquitous language.

import { TOOLS, CATEGORIES, MONTHS, isoOf, type Day, type ToolKey, type Bucket } from './data';

// Re-export only what the 8a views actually consume via './mock'.
export { TOOLS, type ToolKey, type ToolMeta } from './data';
export { fmtTok, fmtUSD, fmtPct } from '../lib/format';

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
      level: levelOf(tokens),
      byTool: splitTools(tokens),
    });
  }
  return days;
}

export const DAYS = buildDays();

export const TOTAL_TOKENS = DAYS.reduce((a, d) => a + d.tokens, 0);

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
  grok: [1, 0, 0, 0], // grok logs expose only a context-size counter → all input
  antigravity: [0.12, 0.01, 0.87, 0], // cache-read dominated (real-data shape)
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
  grok: { skills: 0, mcp: 0, agents: 0, memory: 0 },
  antigravity: { skills: 0, mcp: 1, agents: 0, memory: 0 },
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
  grok: [{ name: 'grok-4.5', share: 1 }],
  antigravity: [
    { name: 'gemini-3-flash-a', share: 0.75 },
    { name: 'gemini-default', share: 0.25 },
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
  return { claude: 0, codex: 0, gemini: 0, hermes: 0, grok: 0, antigravity: 0 };
}
function addInto(dst: Record<ToolKey, number>, src: Record<ToolKey, number>) {
  for (const t of TOOLS) dst[t.key] += src[t.key];
}
function finalize(label: string, by: Record<ToolKey, number>): Bucket {
  return { key: label, label, byTool: by, byModel: {}, total: TOOLS.reduce((s, t) => s + by[t.key], 0) };
}

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

// ---- cost helpers ----

export function costOf(tokens: number): number {
  return (tokens / 1e6) * BLENDED_USD_PER_MTOK;
}
export const TOTAL_COST = costOf(TOTAL_TOKENS);

// Adapter: mock fractions reshaped into the real ContextBreakdown props
// (CtxTotals + meta string), so the unmounted 8a FocusPanel keeps compiling.
export function mockCtxTotals(tool: ToolKey) {
  const c = contextBreakdown(tool);
  const grab = (arr: { key: string; tokens: number }[], key: string) =>
    arr.find((x) => x.key === key)?.tokens ?? null;
  return {
    ctx: {
      billed: c.input,
      reused: c.reused,
      messages: grab([...c.primary], 'messages'),
      system: grab([...c.primary], 'system'),
      reasoning: grab([...c.primary], 'reasoning'),
      toolcalls: grab([...c.secondary], 'toolcalls'),
      agents: grab([...c.secondary], 'agents'),
      mcp: grab([...c.secondary], 'mcp'),
      skills: grab([...c.secondary], 'skills'),
    },
    buckets: {
      source: tool,
      history: Math.round(c.reused * 0.98),
      newInput: Math.round(c.input * 0.004),
      system: Math.round(c.input * 0.003),
      response: Math.round(c.input * 0.002),
      reasoning: null,
    },
    toolRows: [
      { source: tool, name: 'Bash', estTokens: 300, calls: 12 },
      { source: tool, name: 'Read', estTokens: 200, calls: 9 },
    ],
    execRows: [],
    meta: c.meta,
  };
}

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
