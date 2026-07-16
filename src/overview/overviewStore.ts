// Framework-free data-plane store for the Overview. Owns the fetch
// orchestration the shell used to run in effects (scan + full series,
// per-range reloads, debounce, stale-response guard, selection auto-correct)
// and exposes a useSyncExternalStore-shaped surface. No React here.
import { tauriLedger, type LedgerPort } from './ledger';
import {
  isoOf,
  windowOf,
  pointsIn,
  toolTotalsOfPoints,
  seriesToDays,
  rangeToFilters,
  granularityOf,
  calendarSpan,
  bucketsFromPoints,
  sumPoints,
  modelTools,
  smallMultiples,
  catTotals,
  ctxTotals,
  dailyTableRows,
  projectTableRows,
  modelBars,
  ctxMeta,
  bucketView,
  toolTree,
  type Bucket,
  type Day,
  type Granularity,
  type CatTotals,
  type CtxTotals,
  type ModelBar,
  type TableRow,
  type BucketView,
  type ToolCategory,
} from './data';
import { TOOLS, type Range8b, type ToolKey, type ToolMeta } from './meta';
import type { Lang } from '../lib/i18n';
import { fmtIsoDateL, overviewT, RANGE_LONG_KEY } from './localize';
import type {
  Filters,
  ScanStatus,
  SourceStatus,
  SeriesPoint,
  Summary,
  BreakdownRow,
  CtxResourceCount,
  CtxBuckets,
  CtxToolRow,
  CtxExecRow,
} from '../types';

// ---- clock port (Date + timers), so tests can freeze "now" and drive debounce ----
export interface ClockPort {
  now(): Date;
  setTimeout(fn: () => void, ms: number): number;
  clearTimeout(h: number): void;
}

export const systemClock: ClockPort = {
  now: () => new Date(),
  setTimeout: (fn, ms) => globalThis.setTimeout(fn, ms) as unknown as number,
  clearTimeout: (h) => globalThis.clearTimeout(h),
};

// The one filter the unbounded daily series is fetched with.
const EMPTY_FILTERS: Filters = { tools: [], models: [], project: null };

// ---- snapshot ----

export interface OverviewSnapshot {
  allPoints: SeriesPoint[] | null;
  hourPoints: SeriesPoint[];
  summary: Summary | null;
  modelRows: BreakdownRow[];
  projectRows: BreakdownRow[];
  ctxResources: CtxResourceCount[];
  ctxBuckets: CtxBuckets[];
  ctxToolRows: CtxToolRow[];
  ctxExecRows: CtxExecRow[];
  scanSources: SourceStatus[]; // per-source scan stats (eventsInserted / linesSkipped) for the footer
  scanError: string | null;
  fetchError: string | null;
  range: Range8b;
  customFrom: string;
  customTo: string;
  selected: ToolKey;
  // derived (frozen at each transition; getSnapshot must not recompute per call)
  firstIso: string;
  lastIso: string;
  from: string;
  to: string;
  loading: boolean;
}

export interface OverviewStore {
  subscribe(l: () => void): () => void;
  getSnapshot(): OverviewSnapshot;
  refresh(): Promise<void>;
  setRange(r: Range8b): void;
  setCustomRange(from: string, to: string): void;
  setSelected(k: ToolKey): void;
  start(): () => void;
}

// Raw state; derived fields live only in the built snapshot.
type State = Omit<
  OverviewSnapshot,
  'firstIso' | 'lastIso' | 'from' | 'to' | 'loading'
>;

const SNAP_KEYS: (keyof OverviewSnapshot)[] = [
  'allPoints', 'hourPoints', 'summary', 'modelRows', 'projectRows',
  'ctxResources', 'ctxBuckets', 'ctxToolRows', 'ctxExecRows',
  'scanSources', 'scanError', 'fetchError', 'range', 'customFrom', 'customTo', 'selected',
  'firstIso', 'lastIso', 'from', 'to', 'loading',
];

function sameSnapshot(a: OverviewSnapshot, b: OverviewSnapshot): boolean {
  return SNAP_KEYS.every((k) => Object.is(a[k], b[k]));
}

class Store implements OverviewStore {
  private state: State = {
    allPoints: null, hourPoints: [], summary: null, modelRows: [], projectRows: [],
    ctxResources: [], ctxBuckets: [], ctxToolRows: [], ctxExecRows: [],
    scanSources: [], scanError: null, fetchError: null,
    range: 'total', customFrom: '', customTo: '', selected: 'claude',
  };
  private snapshot: OverviewSnapshot;
  private listeners = new Set<() => void>();
  private epoch = 0; // monotonic; supersedes in-flight reload responses
  private reloadTimer: number | null = null; // pending debounce timer

  constructor(private ledger: LedgerPort, private clock: ClockPort) {
    this.snapshot = this.buildSnapshot(this.clock.now());
  }

  subscribe(l: () => void) {
    this.listeners.add(l);
    return () => {
      this.listeners.delete(l);
    };
  }

  getSnapshot() {
    return this.snapshot;
  }

  async refresh() {
    let status: ScanStatus;
    try {
      status = await this.ledger.scan();
    } catch (e) {
      this.state.scanError = String(e);
      this.publish();
      return; // scan threw: do not proceed to the series reload
    }
    this.state.scanSources = status.sources;
    const errs = status.sources
      .filter((s) => s.error)
      .map((s) => `${s.source}: ${s.error}`);
    this.state.scanError = errs.length ? errs.join(' · ') : null;
    this.publish();

    try {
      const pts = await this.ledger.series(EMPTY_FILTERS, 'day');
      this.state.allPoints = pts;
      this.correctSelection();
      this.publish();
      this.scheduleReload();
    } catch (e) {
      const wasNull = this.state.allPoints === null;
      this.state.fetchError = String(e);
      // First load settles to [] so loading ends; later failures keep prior data.
      if (wasNull) this.state.allPoints = [];
      this.publish();
      if (wasNull) this.scheduleReload();
    }
  }

  setRange(r: Range8b) {
    if (r === this.state.range) return;
    this.state.range = r;
    this.correctSelection();
    this.publish();
    this.scheduleReload();
  }

  setCustomRange(from: string, to: string) {
    const s = this.state;
    if (from === s.customFrom && to === s.customTo) return;
    const d = this.derive(this.clock.now()); // allPoints unchanged → firstIso/lastIso stable
    const prevFrom = s.customFrom || d.firstIso;
    const prevTo = s.customTo || d.lastIso;
    s.customFrom = from;
    s.customTo = to;
    this.correctSelection();
    this.publish();
    // Reload only when the effective window bounds moved (matches the cf/ct
    // effect deps: raw '' → firstIso can change nothing).
    if ((from || d.firstIso) !== prevFrom || (to || d.lastIso) !== prevTo) {
      this.scheduleReload();
    }
  }

  setSelected(k: ToolKey) {
    if (k === this.state.selected) return;
    this.state.selected = k;
    this.publish();
  }

  start() {
    const unsub = this.ledger.onPricesRebuilt(() => this.scheduleReload());
    return () => {
      unsub();
      if (this.reloadTimer !== null) {
        this.clock.clearTimeout(this.reloadTimer);
        this.reloadTimer = null;
      }
      this.epoch++; // invalidate any in-flight reload so nothing lands post-dispose
    };
  }

  // ---- internals ----

  private derive(now: Date) {
    const s = this.state;
    const firstIso = s.allPoints && s.allPoints.length
      ? s.allPoints.reduce((a, p) => (p.bucket < a ? p.bucket : a), s.allPoints[0].bucket)
      : isoOf(now);
    const lastIso = isoOf(now);
    return { firstIso, lastIso, from: s.customFrom || firstIso, to: s.customTo || lastIso };
  }

  private buildSnapshot(now: Date): OverviewSnapshot {
    const s = this.state;
    const d = this.derive(now);
    return {
      ...s,
      firstIso: d.firstIso,
      lastIso: d.lastIso,
      from: d.from,
      to: d.to,
      loading: s.allPoints === null,
    };
  }

  // Rebuild + emit only when a field actually moved, so getSnapshot keeps a
  // stable reference across no-op transitions (useSyncExternalStore contract).
  private publish() {
    const next = this.buildSnapshot(this.clock.now());
    if (sameSnapshot(next, this.snapshot)) return;
    this.snapshot = next;
    for (const l of [...this.listeners]) l();
  }

  private patch(partial: Partial<State>) {
    Object.assign(this.state, partial);
    this.publish();
  }

  // Keep the selection on a tool that has usage in the current window.
  private correctSelection() {
    const now = this.clock.now();
    const visible = selectVisibleTools(this.buildSnapshot(now), now);
    if (visible.length && !visible.some((t) => t.key === this.state.selected)) {
      this.state.selected = visible[0].key;
    }
  }

  private scheduleReload() {
    if (this.state.allPoints === null) return; // no per-range fetch until first load lands
    if (this.reloadTimer !== null) this.clock.clearTimeout(this.reloadTimer);
    const epoch = ++this.epoch;
    const s = this.state;
    const d = this.derive(this.clock.now());
    // State can't change between schedule and fire without rescheduling, so
    // capturing filters/isDay here is equivalent to computing them at fire.
    const filters = rangeToFilters(s.range, d.from, d.to);
    const isDay = s.range === 'day';
    const delay = s.range === 'custom' ? 250 : 0;
    this.reloadTimer = this.clock.setTimeout(() => {
      this.reloadTimer = null;
      this.runReload(epoch, filters, isDay);
    }, delay);
  }

  private runReload(epoch: number, filters: Filters, isDay: boolean) {
    const land = (fn: () => void) => {
      if (epoch === this.epoch) fn();
    };
    const L = this.ledger;
    const jobs: Promise<unknown>[] = [
      L.summary(filters).then((v) => land(() => this.patch({ summary: v }))),
      L.breakdown('model', filters).then((v) => land(() => this.patch({ modelRows: v }))),
      L.breakdown('project', filters).then((v) => land(() => this.patch({ projectRows: v }))),
      L.ctxResources(filters).then((v) => land(() => this.patch({ ctxResources: v }))),
      L.ctxBuckets(filters).then((v) => land(() => this.patch({ ctxBuckets: v }))),
      L.ctxTools(filters).then((v) => land(() => this.patch({ ctxToolRows: v }))),
      L.ctxExec(filters).then((v) => land(() => this.patch({ ctxExecRows: v }))),
    ];
    if (isDay) {
      jobs.push(L.series(filters, 'hour').then((v) => land(() => this.patch({ hourPoints: v }))));
    } else if (this.state.hourPoints.length) {
      this.patch({ hourPoints: [] }); // leaving Day: drop the hourly series
    }
    Promise.all(jobs)
      .then(() => land(() => this.patch({ fetchError: null })))
      .catch((e) => land(() => this.patch({ fetchError: String(e) })));
  }
}

export function createOverviewStore(ports?: {
  ledger?: LedgerPort;
  clock?: ClockPort;
}): OverviewStore {
  return new Store(ports?.ledger ?? tauriLedger, ports?.clock ?? systemClock);
}

// ---- pure selectors over a snapshot (compose ./data; no ports, no Date.now) ----

export interface OverviewView {
  rpts: SeriesPoint[];
  total: number;
  toolTotals: Record<ToolKey, number>;
  per: Granularity;
  trend: Bucket[];
  modelTool: Record<string, string>;
  sparks: ReturnType<typeof smallMultiples>;
  cats: CatTotals;
  ctx: CtxTotals;
  dailyRows: TableRow[];
  projectRows: TableRow[];
  rangeLabel: string;
  grand: number;
  ctxView: BucketView | null;
  ctxTree: ToolCategory[];
  selExecRows: CtxExecRow[];
  selMeta: string;
  selModels: ModelBar[];
  tool: ToolMeta;
  headline: { total: number; summaryReady: boolean };
  canOpenCostBreakdown: boolean;
}

// The 365-day heatmap grid depends only on the full series. Callers MUST
// memoize this on s.allPoints identity — the heatmap must not recompute on
// range/selection changes (the store keeps allPoints's reference stable across
// those, see the getSnapshot-identity test).
export function selectDays(s: OverviewSnapshot, now: Date = new Date()): Day[] {
  return seriesToDays(s.allPoints ?? [], now);
}

export function selectVisibleTools(s: OverviewSnapshot, now: Date = new Date()): ToolMeta[] {
  const totals = toolTotalsOfPoints(pointsIn(s.allPoints ?? [], windowOf(s.range, s.from, s.to, now)));
  return TOOLS.filter((t) => totals[t.key] > 0);
}

export function selectView(s: OverviewSnapshot, now: Date = new Date(), lang: Lang = 'en'): OverviewView {
  const pts = s.allPoints ?? [];
  const win = windowOf(s.range, s.from, s.to, now);
  const rpts = pointsIn(pts, win);
  // Window bounds (not the raw custom inputs) drive granularity + trend.
  const winFrom = win.fromIso ?? s.firstIso;
  const winTo = win.toIso ?? s.lastIso;
  const per = granularityOf(s.range, calendarSpan(winFrom, winTo));
  const trend =
    per === 'hour'
      ? bucketsFromPoints(s.hourPoints, 'hour', winFrom, winTo, lang)
      : bucketsFromPoints(rpts, per, winFrom, winTo, lang);
  const total = sumPoints(rpts);
  const toolTotals = toolTotalsOfPoints(rpts);
  const ctx = ctxTotals(rpts, s.selected);
  const selBuckets = s.ctxBuckets.find((b) => b.source === s.selected) ?? null;
  const selToolRows = s.ctxToolRows.filter((r) => r.source === s.selected);
  return {
    rpts,
    total,
    toolTotals,
    per,
    trend,
    modelTool: modelTools(rpts),
    sparks: smallMultiples(trend),
    cats: catTotals(rpts, s.selected),
    ctx,
    dailyRows: dailyTableRows(rpts),
    projectRows: projectTableRows(s.projectRows),
    // Custom shows the entered bounds (s.from/s.to), not the normalized window.
    rangeLabel:
      s.range === 'custom'
        ? `${fmtIsoDateL(s.from, lang)} – ${fmtIsoDateL(s.to, lang)}`
        : overviewT(lang, RANGE_LONG_KEY[s.range]),
    grand: total || 1,
    // Context drill-down derivations, memoized per snapshot here instead of per
    // render in ContextBreakdown.
    ctxView: bucketView(selBuckets),
    ctxTree: toolTree(selToolRows, ctx.toolcalls),
    selExecRows: s.ctxExecRows.filter((r) => r.source === s.selected),
    selMeta: ctxMeta(s.ctxResources, s.selected, lang),
    selModels: modelBars(s.modelRows, s.selected, toolTotals[s.selected]),
    tool: TOOLS.find((t) => t.key === s.selected)!,
    headline: { total: s.summary?.totalTokens ?? total, summaryReady: s.summary !== null },
    canOpenCostBreakdown: s.summary !== null && s.modelRows.length > 0,
  };
}
