// The Menu Bar Extra panel's view model (design 2b grown — ADR-0007 for why the
// surface is a webview at all). Pure: the period's reads in, display strings
// out. These rules moved here from the retired native-menu tray model in
// src-tauri/tray.rs; the Rust side now only computes the bar title.
import { formatCompactTokenTotal } from '../lib/format';
import { formatCost } from '../lib/currency';
import type { Lang } from '../lib/i18n';
import { isoOf } from '../overview/data';
import { sourceMeta } from '../overview/meta';
import { sourceIcon } from '../overview/icons';
import type { BreakdownRow, SeriesPoint, Settings, Summary } from '../types';
import { isAllUnattributedCost, isPartialCost, type CostCompleteness } from '../lib/costCompleteness';

export interface PanelRow {
  key: string;
  label: string;
  icon?: string; // asset URL; Source rows and Model rows both carry one
  color?: string; // the owning Source's brand colour
  share?: number; // Source rows only: this row's slice of the period's priced Cost, 0..1
  tokens: string;
  cost: string;
}

// The period's Cost per bucket, normalised for the view: geometry (viewBox,
// stroke) stays in the panel, the shape and the read-out are decided here.
export interface PanelChart {
  points: number[]; // 0..1 of the period's peak, one per bucket, gaps zero-filled
  ticks: string[]; // sparse x labels — first, middle, last — spread across the points
  peak: string; // "peak 14:00 · $101.15"
  peakIndex: number; // which bucket the peak caption names — the view brightens it
  details: string[]; // hover read-out per bucket — "14:00 · $3.12 · 1.2M tok"
}

export interface PanelStats {
  cacheHit: string;
  scanned: string; // "just now" | "8 min ago" | "2h ago" | "—"
}

// The reads beyond the 2b panel's own: the Models section, the stats strip and
// the chart. Optional as a whole — without them the panel is exactly the
// surface it was.
export interface PanelExtras {
  period: Period;
  now: Date;
  models: BreakdownRow[];
  series: SeriesPoint[]; // per (bucket, Source) over the period
  scannedAt: number; // epoch seconds; 0 = no scan yet this launch
}

export interface PanelModel {
  cost: string; // "$12.84" | "≥ $12.84" | "unpriced" | "unavailable"
  delta: string | null; // "+12.4%", null when yesterday-so-far has no Cost
  deltaUp: boolean;
  rows: PanelRow[]; // every Source with usage — the stacked bar splits them all
  legend: PanelRow[]; // the head of `rows` the legend names, capped
  legendOverflow: number; // Sources the cap hid, 0 when none
  empty: boolean; // no usage in the window → the figures read $0.00 · 0 tok, but
  // the sections below them stay away rather than fabricating a 0.0% cache hit
  // Raw values + per-frame formatters for the header count-up animation:
  // the view tweens the numbers and formats each frame with the same rules
  // the static strings above use (≥ marker and Display Currency included).
  costValue: number | null; // USD; null = unavailable or unpriced, not animatable
  tokensValue: number;
  requestsText: string; // "1,912" — not animated, appended to the sub line
  fmtCost(v: number): string;
  fmtTokens(v: number): string;
  chart: PanelChart | null; // null hides the chart
  models: PanelRow[];
  modelsOverflow: number; // Models the cap hid, 0 when none
  stats: PanelStats | null;
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

// Cost desc; all-Unpriced rows (cost null) last, by tokens desc. The one
// ordering rule for every list on the panel — Sources, Models, Projects.
function byCost(a: BreakdownRow, b: BreakdownRow): number {
  if (a.cost !== null && b.cost !== null) return b.cost - a.cost;
  if (a.cost !== null) return -1;
  if (b.cost !== null) return 1;
  return b.totalTokens - a.totalTokens;
}

// Both lists are capped, and the caps are why the panel still fits a laptop.
// The window hugs its content (resize_panel), and an anchored panel taller
// than the monitor's work area has nowhere left to clamp to (tray.rs), so it
// simply hangs off the bottom of the screen — which an 8-Source, 20-Model
// month did. Three of each is the tallest either list gets; the app holds the
// rest and the overflow lines say how much.
const MODEL_CAP = 3;
const SOURCE_CAP = 3;

const pad = (n: number) => String(n).padStart(2, '0');

// Which bucket size the period's series is read at — the panel fetches with
// this and the chart is built from it, so the two cannot drift.
export function seriesBucket(period: Period): 'hour' | 'day' {
  return period === 'days30' ? 'day' : 'hour';
}

// Every bucket key the period covers, in the local-calendar form the backend's
// strftime emits ("2026-06-15 14:00" / "2026-06-15"). Walked on the calendar
// rather than by adding seconds, so a DST day yields the hours it really had. A
// period in progress stops at the bucket in progress: hours that have not
// happened yet are not idle hours.
function bucketKeys(period: Period, now: Date): string[] {
  const w = periodWindows(period, now);
  const start = new Date(w.start * 1000);
  const last = new Date(Math.min(w.end * 1000 - 1, now.getTime()));
  const keys: string[] = [];
  if (seriesBucket(period) === 'hour') {
    for (
      let d = new Date(start.getFullYear(), start.getMonth(), start.getDate(), start.getHours());
      d <= last;
      d = new Date(d.getFullYear(), d.getMonth(), d.getDate(), d.getHours() + 1)
    ) {
      keys.push(`${isoOf(d)} ${pad(d.getHours())}:00`);
    }
  } else {
    for (
      let d = new Date(start.getFullYear(), start.getMonth(), start.getDate());
      d <= last;
      d = new Date(d.getFullYear(), d.getMonth(), d.getDate() + 1)
    ) {
      keys.push(isoOf(d));
    }
  }
  return keys;
}

interface Cell extends CostCompleteness {
  cost: number; // series Cost is never null — Unpriced usage contributes 0
  totalTokens: number;
}

const emptyCell = (): Cell => ({ cost: 0, totalTokens: 0, hasUnpriced: false, unattributedTokens: 0 });

// A bucket key as the axis says it: "2026-06-15 14:00" → "14:00"; "2026-06-15" → "06-15".
const tickLabel = (key: string) => (key.includes(' ') ? key.slice(11) : key.slice(5));

// The period's Cost per bucket. Series points arrive per (bucket, Source), so
// a bucket's figure is the sum across the Sources that ran in it.
function costChart(
  current: Summary,
  extras: PanelExtras,
  settings: CostSettings,
  lang: Lang,
): PanelChart | null {
  // No Cost for the period at all (all-Unattributed, or every Model Unpriced):
  // a flat line along zero would assert usage was free.
  if (current.cost === null) return null;

  const cells = new Map<string, Cell>();
  for (const p of extras.series) {
    const c = cells.get(p.bucket) ?? emptyCell();
    c.cost += p.cost;
    c.totalTokens += p.totalTokens;
    c.hasUnpriced ||= p.hasUnpriced;
    c.unattributedTokens += p.unattributedTokens;
    cells.set(p.bucket, c);
  }

  const keys = bucketKeys(extras.period, extras.now);
  const row = keys.map((k) => cells.get(k) ?? emptyCell());
  // Usage somewhere is enough to draw: a day 40 minutes old has one hour slot,
  // and hiding its chart reads as breakage rather than as a young day. The
  // view draws a lone bucket as one column. A period whose whole Cost is zero
  // still has no shape to normalise against.
  if (!row.some((c) => c.totalTokens > 0)) return null;
  const peak = row.reduce((a, b) => (b.cost > a.cost ? b : a), row[0]);
  if (peak.cost <= 0) return null;

  // Three labels at most — ends and middle. They read as the axis, and they
  // say the bucket size on their own ("14:00" is an hour, "06-15" a day), so
  // no separate hourly/daily caption is needed. A short window collapses to
  // the labels it actually has.
  const at = [...new Set([0, Math.floor((keys.length - 1) / 2), keys.length - 1])];
  const peakIndex = row.indexOf(peak);
  return {
    points: row.map((c) => c.cost / peak.cost),
    ticks: at.map((i) => tickLabel(keys[i])),
    peak: `peak ${tickLabel(keys[peakIndex])} · ${cost(peak, settings, lang)}`,
    peakIndex,
    // One read-out per bucket for the hover inspector, preformatted like the
    // peak line so the view stays display-only. An idle bucket honestly reads
    // $0.00 · 0 tok — the period as a whole is priced or chart would be null.
    details: row.map((c, i) => {
      // Series Cost is a sum of priced usage, so a bucket holding only
      // Unpriced or Unattributed usage sums to 0 — which cost() must read as
      // the absence it is ("unpriced"/"unavailable"), never as $0.
      const cell = c.cost === 0 && (c.hasUnpriced || c.unattributedTokens > 0) ? { ...c, cost: null } : c;
      return `${tickLabel(keys[i])} · ${cost(cell, settings, lang)} · ${formatCompactTokenTotal(c.totalTokens)} tok`;
    }),
  };
}

// "8 min ago" since the last scan. Computed at render from the timestamp the
// panel fetched: it refetches on every open and after a Rescan, which is
// exactly when this string can change, so no ticking timer is needed.
function since(ts: number, now: Date): string {
  if (ts <= 0) return '—'; // no scan yet this launch — never a wrong "just now"
  const s = Math.max(0, Math.floor(now.getTime() / 1000) - ts);
  if (s < 60) return 'just now';
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  return `${Math.floor(s / 3600)}h ago`;
}

export function panelModel(
  today: Summary,
  yesterdaySoFar: Summary,
  tools: BreakdownRow[],
  settings: CostSettings,
  lang: Lang,
  extras?: PanelExtras,
): PanelModel {
  // Pace vs the same time yesterday; hidden when either side has no Cost,
  // and on an empty today — "-100.0%" beside a $0.00 helps nobody.
  let delta: string | null = null;
  let deltaUp = true;
  const y = yesterdaySoFar.cost;
  if (today.totalTokens > 0 && today.cost !== null && y !== null && y > 0) {
    const pct = (today.cost / y - 1) * 100;
    delta = `${pct >= 0 ? '+' : ''}${pct.toFixed(1)}%`;
    deltaUp = pct >= 0;
  }

  const used = tools.filter((r) => r.totalTokens > 0).sort(byCost);

  // Models: same rows as the Sources above, named by their raw logged name and
  // keyed by Source too — two Sources can run the same Model, and each one's
  // usage is its own row.
  const usedModels = extras?.models.filter((r) => r.totalTokens > 0).sort(byCost) ?? [];
  const models = usedModels.slice(0, MODEL_CAP).map((r) => ({
    key: `${r.source ?? 'unknown'}/${r.key ?? 'unknown'}`,
    label: r.key ?? 'unknown',
    icon: sourceIcon(sourceMeta(r.source ?? 'unknown').icon),
    tokens: formatCompactTokenTotal(r.totalTokens),
    cost: cost(r, settings, lang),
  }));

  // The stacked source bar splits the period's priced Cost; a row whose Cost
  // is null (all-Unpriced or all-Unattributed) has no slice to claim.
  const pricedTotal = used.reduce((a, r) => a + (r.cost ?? 0), 0);

  const requestsText = today.requests.toLocaleString('en-US');
  const rows: PanelRow[] = used.map((r) => {
    const key = r.key ?? 'unknown';
    const meta = sourceMeta(key);
    return {
      key,
      label: meta.label,
      icon: sourceIcon(meta.icon),
      color: meta.color,
      share: pricedTotal > 0 ? (r.cost ?? 0) / pricedTotal : 0,
      tokens: formatCompactTokenTotal(r.totalTokens),
      cost: cost(r, settings, lang),
    };
  });
  return {
    cost: cost(today, settings, lang),
    delta,
    deltaUp,
    costValue: today.cost,
    tokensValue: today.totalTokens,
    requestsText,
    fmtCost: (v: number) => cost({ ...today, cost: v }, settings, lang),
    fmtTokens: formatCompactTokenTotal,
    // The bar keeps every slice — it splits the whole priced Cost, and a
    // capped bar would misstate the split. Only the naming is capped.
    rows,
    legend: rows.slice(0, SOURCE_CAP),
    legendOverflow: Math.max(0, rows.length - SOURCE_CAP),
    empty: today.totalTokens === 0,
    chart: extras ? costChart(today, extras, settings, lang) : null,
    models,
    modelsOverflow: Math.max(0, usedModels.length - models.length),
    stats: extras
      ? {
          cacheHit: `${(today.cacheHitRate * 100).toFixed(1)}%`,
          scanned: since(extras.scannedAt, extras.now),
        }
      : null,
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
