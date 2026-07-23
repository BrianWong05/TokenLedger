// The Menu Bar Extra panel's view model (design 2b, pixel-faithful — ADR-0007).
// Pure: Summaries + tool breakdown in, display strings out. These rules moved
// here from the retired native-menu tray model in src-tauri/tray.rs; the Rust
// side now only computes the bar title.
import { formatCompactTokenTotal } from '../lib/format';
import { formatCost } from '../lib/currency';
import type { Lang } from '../lib/i18n';
import { TOOLS } from '../overview/meta';
import { TOOL_ICONS } from '../overview/icons';
import type { BreakdownRow, Settings, Summary } from '../types';
import { isAllUnattributedCost, isPartialCost, type CostCompleteness } from '../lib/costCompleteness';

export interface PanelRow {
  key: string;
  label: string;
  icon?: string; // asset URL; unknown sources have none but are never dropped
  tokens: string;
  cost: string;
}

export interface PanelModel {
  cost: string; // "$12.84" | "≥ $12.84" | "unpriced" | "unavailable"
  delta: string | null; // "+12.4%", null when yesterday-so-far has no Cost
  deltaUp: boolean;
  sub: string; // "3.4M tok · 1,912 req"
  rows: PanelRow[];
  empty: boolean; // no usage today → panel shows its empty state
  // Raw values + per-frame formatters for the header count-up animation:
  // the view tweens the numbers and formats each frame with the same rules
  // the static strings above use (≥ marker and Display Currency included).
  costValue: number | null; // USD; null = unavailable or unpriced, not animatable
  tokensValue: number;
  requestsText: string; // "1,912" — not animated, appended to the sub line
  fmtCost(v: number): string;
  fmtTokens(v: number): string;
}

type CostSettings = Pick<Settings, 'currency' | 'usdRate'>;

// Cost per the glossary: Display Currency at render time, "≥ " marks a
// Partial Cost, and missing Cost distinguishes Unpriced Models from an
// all-Unattributed selection — never $0.
function cost(value: CostCompleteness, s: CostSettings, lang: Lang): string {
  if (isAllUnattributedCost(value)) return 'unavailable';
  if (value.cost === null) return 'unpriced';
  const base = formatCost(value.cost, s, lang);
  return isPartialCost(value) ? `≥ ${base}` : base;
}

export function panelModel(
  today: Summary,
  yesterdaySoFar: Summary,
  tools: BreakdownRow[],
  settings: CostSettings,
  lang: Lang,
): PanelModel {
  // Pace vs the same time yesterday; hidden when either side has no Cost,
  // and on an empty today — "No usage yet" beside "-100.0%" helps nobody.
  let delta: string | null = null;
  let deltaUp = true;
  const y = yesterdaySoFar.cost;
  if (today.totalTokens > 0 && today.cost !== null && y !== null && y > 0) {
    const pct = (today.cost / y - 1) * 100;
    delta = `${pct >= 0 ? '+' : ''}${pct.toFixed(1)}%`;
    deltaUp = pct >= 0;
  }

  const used = tools.filter((r) => r.totalTokens > 0);
  // Cost desc; all-Unpriced Sources (cost null) last, by tokens desc.
  used.sort((a, b) => {
    if (a.cost !== null && b.cost !== null) return b.cost - a.cost;
    if (a.cost !== null) return -1;
    if (b.cost !== null) return 1;
    return b.totalTokens - a.totalTokens;
  });

  const requestsText = today.requests.toLocaleString('en-US');
  return {
    cost: cost(today, settings, lang),
    delta,
    deltaUp,
    sub: `${formatCompactTokenTotal(today.totalTokens)} tok · ${requestsText} req`,
    costValue: today.cost,
    tokensValue: today.totalTokens,
    requestsText,
    fmtCost: (v: number) => cost({ ...today, cost: v }, settings, lang),
    fmtTokens: formatCompactTokenTotal,
    rows: used.map((r) => {
      const key = r.key ?? 'unknown';
      const meta = TOOLS.find((t) => t.key === key);
      return {
        key,
        label: meta?.label ?? key,
        icon: meta ? TOOL_ICONS[meta.key] : undefined,
        tokens: formatCompactTokenTotal(r.totalTokens),
        cost: cost(r, settings, lang),
      };
    }),
    empty: today.totalTokens === 0,
  };
}

// The panel's period selector (design 2b's Today / Yesterday / 30 days).
export type Period = 'today' | 'yesterday' | 'days30';

// [start, end) plus the comparison window [prevStart, prevEnd) for the pace
// delta, all local-calendar-day aligned (same semantics as the Overview's
// day buckets and the bar title's day_window in Rust) and end-exclusive.
// Epoch seconds. Today compares so-far vs yesterday clamped to now − 24h;
// the completed periods compare full window vs the full window before it.
export function periodWindows(
  period: Period,
  now: Date,
): { start: number; end: number; prevStart: number; prevEnd: number } {
  const mid = (offsetDays: number) =>
    new Date(now.getFullYear(), now.getMonth(), now.getDate() + offsetDays);
  const sec = (d: Date) => Math.floor(d.getTime() / 1000);
  switch (period) {
    case 'today':
      return {
        start: sec(mid(0)),
        end: sec(mid(1)),
        prevStart: sec(mid(-1)),
        prevEnd: Math.floor(now.getTime() / 1000) - 86_400,
      };
    case 'yesterday':
      return {
        start: sec(mid(-1)),
        end: sec(mid(0)),
        prevStart: sec(mid(-2)),
        prevEnd: sec(mid(-1)),
      };
    case 'days30':
      return {
        start: sec(mid(-29)),
        end: sec(mid(1)),
        prevStart: sec(mid(-59)),
        prevEnd: sec(mid(-29)),
      };
  }
}
