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

export interface PanelRow {
  key: string;
  label: string;
  icon?: string; // asset URL; unknown sources have none but are never dropped
  tokens: string;
  cost: string;
}

export interface PanelModel {
  cost: string; // "$12.84" | "≥ $12.84" | "unpriced"
  delta: string | null; // "+12.4%", null when yesterday-so-far has no Cost
  deltaUp: boolean;
  sub: string; // "3.4M tok · 1,912 req"
  rows: PanelRow[];
  empty: boolean; // no usage today → panel shows its empty state
}

type CostSettings = Pick<Settings, 'currency' | 'usdRate'>;

// Cost per the glossary: Display Currency at render time, "≥ " marks a
// Partial Cost, and Unpriced is worded — never $0.
function cost(c: number | null, hasUnpriced: boolean, s: CostSettings, lang: Lang): string {
  if (c === null) return 'unpriced';
  const base = formatCost(c, s, lang);
  return hasUnpriced ? `≥ ${base}` : base;
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

  return {
    cost: cost(today.cost, today.hasUnpriced, settings, lang),
    delta,
    deltaUp,
    sub: `${formatCompactTokenTotal(today.totalTokens)} tok · ${today.requests.toLocaleString('en-US')} req`,
    rows: used.map((r) => {
      const meta = TOOLS.find((t) => t.key === r.key);
      return {
        key: r.key,
        label: meta?.label ?? r.key,
        icon: meta ? TOOL_ICONS[meta.key] : undefined,
        tokens: formatCompactTokenTotal(r.totalTokens),
        cost: cost(r.cost, r.hasUnpriced, settings, lang),
      };
    }),
    empty: today.totalTokens === 0,
  };
}

// [local midnight, next local midnight) for today, plus yesterday clamped to
// now − 24h — same semantics as the Overview's local-day buckets and the bar
// title's day_window in Rust, so the two surfaces can never disagree.
// Epoch seconds, ends exclusive.
export function dayWindows(now: Date): {
  todayStart: number;
  todayEnd: number;
  yStart: number;
  yEnd: number;
} {
  const mid = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate());
  const sec = (d: Date) => Math.floor(d.getTime() / 1000);
  const todayStart = mid(now);
  const tomorrow = new Date(todayStart.getFullYear(), todayStart.getMonth(), todayStart.getDate() + 1);
  const yesterday = new Date(now.getTime() - 86_400_000);
  return {
    todayStart: sec(todayStart),
    todayEnd: sec(tomorrow),
    yStart: sec(mid(yesterday)),
    yEnd: sec(now) - 86_400,
  };
}
