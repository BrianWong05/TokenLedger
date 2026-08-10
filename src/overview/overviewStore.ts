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
  hourlyDayOf,
  trendSlice,
  smallMultiples,
  catTotals,
  ctxTotals,
  dailyTableRows,
  projectTableRows,
  modelBars,
  ctxMeta,
  bucketView,
  toolTree,
  profileView,
  profileSessionFilters,
  type Bucket,
  type Day,
  type Granularity,
  type CatTotals,
  type CtxTotals,
  type ModelBar,
  type ProfileView,
  type TableRow,
  type BucketView,
  type ToolCategory,
  skillBars,
  type SkillBar,
  mcpBars,
  type McpBar,
} from './data';
import type { ReportCtxCategory, ReportInput, ReportUsageRow } from './reportCsv';
import { SOURCES, orderedSourceKeys, sourceMeta, type Range8b, type SourceKey, type SourceMeta } from './meta';
import type { Lang } from '../lib/i18n';
import { fmtIsoDateL, overviewT, RANGE_LONG_KEY } from './localize';
import { parseLocalDate } from '../lib/dateRange';
import { unreadableSourcesIn } from '../lib/tokenCompleteness';
import type {
  Filters,
  ScanStatus,
  SourceStatus,
  SeriesPoint,
  Summary,
  BreakdownRow,
  CtxResource,
  CtxSkillRow,
  CtxBuckets,
  CtxToolRow,
  CtxExecRow,
  Settings,
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
  // Distinct Sessions over the fixed trailing 30 days, for the Profile. Its own
  // fetch because the Profile ignores the range, and null until it lands.
  profileSessions: number | null;
  modelRows: BreakdownRow[];
  // One row per Source over the window. Nothing on screen reads these — the
  // cards derive their totals from the series — but the report needs a Source
  // block with the same Cost honesty the Model and Project blocks carry.
  sourceRows: BreakdownRow[];
  projectRows: BreakdownRow[];
  ctxResources: CtxResource[];
  ctxSkillRows: CtxSkillRow[];
  ctxBuckets: CtxBuckets[];
  ctxToolRows: CtxToolRow[];
  ctxExecRows: CtxExecRow[];
  scanSources: SourceStatus[]; // per-source scan stats (eventsInserted / linesSkipped) for the footer
  scanError: string | null;
  scanAt: number | null; // epoch ms of the last successful scan; drives the toolbar's last-scan label
  fetchError: string | null;
  range: Range8b;
  customFrom: string;
  customTo: string;
  selected: SourceKey;
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
  setSelected(k: SourceKey): void;
  start(): () => void;
}

// Raw state; derived fields live only in the built snapshot.
type State = Omit<
  OverviewSnapshot,
  'firstIso' | 'lastIso' | 'from' | 'to' | 'loading'
>;

const SNAP_KEYS: (keyof OverviewSnapshot)[] = [
  'allPoints', 'hourPoints', 'summary', 'profileSessions', 'modelRows', 'sourceRows', 'projectRows',
  'ctxResources', 'ctxBuckets', 'ctxToolRows', 'ctxSkillRows', 'ctxExecRows',
  'scanSources', 'scanError', 'scanAt', 'fetchError', 'range', 'customFrom', 'customTo', 'selected',
  'firstIso', 'lastIso', 'from', 'to', 'loading',
];

function sameSnapshot(a: OverviewSnapshot, b: OverviewSnapshot): boolean {
  return SNAP_KEYS.every((k) => Object.is(a[k], b[k]));
}

class Store implements OverviewStore {
  private state: State = {
    allPoints: null, hourPoints: [], summary: null, profileSessions: null,
    modelRows: [], sourceRows: [], projectRows: [],
    ctxResources: [], ctxBuckets: [], ctxToolRows: [], ctxSkillRows: [], ctxExecRows: [],
    scanSources: [], scanError: null, scanAt: null, fetchError: null,
    range: 'total', customFrom: '', customTo: '', selected: SOURCES[0].key,
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
    // Backend reports epoch seconds (scan.rs as_secs); scanAt is epoch ms.
    this.state.scanAt = status.scannedAt ? status.scannedAt * 1000 : this.clock.now().getTime();
    this.publish();

    // Idle tick: nothing ingested, no source errored, data already on screen —
    // the Ledger is bit-identical to what's rendered, so skip the series fetch
    // and the 9-query reload. This is the every-30s steady state of an open
    // app; the skip is what lets it sit at ~0 CPU instead of re-rendering the
    // whole dashboard each tick. Prices-rebuilt still forces a reload via its
    // own listener.
    // fetchError null required: a failed cycle must retry on the next tick
    // even when the scan reports nothing new.
    const idle = status.sources.every((s) => !s.error && s.eventsInserted === 0);
    if (idle && this.state.allPoints !== null && this.state.fetchError === null) return;

    try {
      // The Profile's Session count rides with the unbounded series, not with
      // the per-range reload: both describe the Ledger itself, so both refresh
      // exactly when a scan brings something new — and a range click reissues
      // neither. A failure leaves the count unknown ('—') without failing the
      // series it travelled with.
      const [pts, sessions] = await Promise.all([
        this.ledger.series(EMPTY_FILTERS, 'day'),
        this.ledger.summary(profileSessionFilters(this.clock.now())).then(
          (s) => s.convs,
          () => null,
        ),
      ]);
      this.state.allPoints = pts;
      this.state.profileSessions = sessions;
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

  setSelected(k: SourceKey) {
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
    // capturing filters/hourDay here is equivalent to computing them at fire.
    const filters = rangeToFilters(s.range, d.from, d.to);
    const hourDay = hourlyDayOf(s.range, d.from, d.to, d.firstIso, d.lastIso, this.clock.now());
    const delay = s.range === 'custom' ? 250 : 0;
    this.reloadTimer = this.clock.setTimeout(() => {
      this.reloadTimer = null;
      this.runReload(epoch, filters, hourDay);
    }, delay);
  }

  // All jobs land as ONE patch/publish: nine individual landings would
  // re-render the dashboard nine times per reload (visible as a long-task
  // burst every refresh tick).
  private runReload(epoch: number, filters: Filters, hourDay: string | null) {
    const land = (fn: () => void) => {
      if (epoch === this.epoch) fn();
    };
    const L = this.ledger;
    // hourPoints describe exactly one day. Drop them when the window stops being
    // hourly OR moves to a different day: their keys would miss every bucket of
    // the new window, painting 24 empty bars — permanently, if this reload then
    // fails. A same-day refresh keeps them, so a background tick never blinks.
    const held = this.state.hourPoints[0]?.bucket.slice(0, 10) ?? null;
    if (this.state.hourPoints.length && held !== hourDay) {
      this.patch({ hourPoints: [] });
    }
    Promise.all([
      L.summary(filters),
      L.breakdown('model', filters),
      L.breakdown('project', filters),
      L.ctxResources(filters),
      L.ctxBuckets(filters),
      L.ctxTools(filters),
      L.ctxSkills(filters),
      L.ctxExec(filters),
      hourDay ? L.series(filters, 'hour') : null,
      L.breakdown('tool', filters),
    ])
      .then(([summary, modelRows, projectRows, ctxResources, ctxBuckets, ctxToolRows, ctxSkillRows, ctxExecRows, hour, sourceRows]) =>
        land(() =>
          this.patch({
            summary, modelRows, projectRows, ctxResources, ctxBuckets, ctxToolRows, ctxSkillRows, ctxExecRows, sourceRows,
            ...(hour ? { hourPoints: hour } : {}),
            fetchError: null,
          }),
        ),
      )
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
  toolTotals: Record<string, number>;
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
  selSkills: SkillBar[];
  selMcp: McpBar[];
  selModels: ModelBar[];
  tool: SourceMeta;
  headline: { total: number; summaryReady: boolean };
  canOpenCostBreakdown: boolean;
  // Sources whose Unreadable Artifacts could hold usage in this window
  // (ADR-0017) — every token total shown for the window is a floor.
  unreadable: SourceStatus[];
}

// The 365-day heatmap grid depends only on the full series. Callers MUST
// memoize this on s.allPoints identity — the heatmap must not recompute on
// range/selection changes (the store keeps allPoints's reference stable across
// those, see the getSnapshot-identity test).
export function selectDays(s: OverviewSnapshot, now: Date = new Date()): Day[] {
  return seriesToDays(s.allPoints ?? [], now);
}

// The Profile ignores the range and the Source selection, so like the heatmap
// it depends only on the full series (plus its own Session count). Callers MUST
// memoize on those two identities, not on the snapshot.
export function selectProfile(s: OverviewSnapshot, now: Date = new Date()): ProfileView {
  return profileView(s.allPoints ?? [], s.profileSessions, now);
}

export function selectVisibleTools(s: OverviewSnapshot, now: Date = new Date()): SourceMeta[] {
  const totals = toolTotalsOfPoints(pointsIn(s.allPoints ?? [], windowOf(s.range, s.from, s.to, now)));
  return orderedSourceKeys(Object.keys(totals).filter((key) => totals[key] > 0)).map(sourceMeta);
}

export function selectView(s: OverviewSnapshot, now: Date = new Date(), lang: Lang = 'en'): OverviewView {
  const pts = s.allPoints ?? [];
  // Window bounds (not the raw custom inputs) drive granularity + trend; the
  // enlarge shares this exact derivation via trendSlice.
  const { rpts, trend, per, modelTool, total } = trendSlice(
    pts, s.hourPoints, s.range, s.from, s.to, s.firstIso, s.lastIso, now, lang,
  );
  const toolTotals = toolTotalsOfPoints(rpts);
  const ctx = ctxTotals(rpts, s.selected);
  const win = windowOf(s.range, s.from, s.to, now);
  const unreadable = unreadableSourcesIn(
    s.scanSources,
    win.fromIso ? Math.floor(parseLocalDate(win.fromIso).getTime() / 1000) : null,
  );
  const selBuckets = s.ctxBuckets.find((b) => b.source === s.selected) ?? null;
  const selToolRows = s.ctxToolRows.filter((r) => r.source === s.selected);
  return {
    rpts,
    total,
    toolTotals,
    per,
    trend,
    modelTool,
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
    selSkills: skillBars(s.ctxSkillRows, s.selected),
    selMcp: mcpBars(selToolRows, ctx.mcp),
    selModels: modelBars(s.modelRows, s.selected, toolTotals[s.selected]),
    tool: sourceMeta(s.selected),
    headline: { total: s.summary?.totalTokens ?? total, summaryReady: s.summary !== null },
    canOpenCostBreakdown: s.summary !== null && s.modelRows.length > 0,
    unreadable,
  };
}

// ---- the window report's input ----

// A BreakdownRow in the report's shape. `key: null` is the Model breakdown's
// Unattributed Usage row; every other breakdown names its key.
function reportRow(row: BreakdownRow, withSource: boolean): ReportUsageRow {
  return {
    key: row.key ?? 'Unattributed usage',
    ...(withSource ? { source: row.source ?? '' } : {}),
    inputTokens: row.inputTokens,
    outputTokens: row.outputTokens,
    cacheReadTokens: row.cacheReadTokens,
    cacheWriteTokens: row.cacheWriteTokens,
    totalTokens: row.totalTokens,
    requests: row.requests,
    sessions: row.convs,
    cost: row.cost,
    hasUnpriced: row.hasUnpriced,
    unattributedTokens: row.unattributedTokens,
    cacheEstimated: row.cacheEstimated,
  };
}

// The time block: one row per bucket, summed across Sources as the Trend sums it.
//
// SeriesPoint carries `cost: number` with a separate hasUnpriced flag, so
// unlike Summary and BreakdownRow it cannot express "no priced tokens". Rather
// than refetch a summary per bucket — which would break the no-refetch property
// this design rests on — a bucket with unpriced usage and no cost resolves to
// null. The lean is deliberate: a bucket genuinely worth $0 that also holds an
// Unpriced Model reads unavailable, erring toward admitting ignorance rather
// than writing a 0 that means "unknown".
function reportTimeRows(pts: SeriesPoint[]): ReportUsageRow[] {
  const byBucket = new Map<string, ReportUsageRow>();
  for (const p of pts) {
    const row = byBucket.get(p.bucket) ?? {
      key: p.bucket,
      inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0,
      totalTokens: 0, requests: 0, sessions: 0, cost: 0,
      hasUnpriced: false, unattributedTokens: 0, cacheEstimated: false,
    };
    row.inputTokens += p.inputTokens;
    row.outputTokens += p.outputTokens;
    row.cacheReadTokens += p.cacheReadTokens;
    row.cacheWriteTokens += p.cacheWriteTokens;
    row.totalTokens += p.totalTokens;
    row.requests += p.requests;
    row.sessions += p.convs;
    row.cost = (row.cost ?? 0) + p.cost;
    row.hasUnpriced ||= p.hasUnpriced;
    row.unattributedTokens += p.unattributedTokens;
    byBucket.set(p.bucket, row);
  }
  return [...byBucket.values()]
    .map((row) => (row.hasUnpriced && row.cost === 0 ? { ...row, cost: null } : row))
    .sort((a, b) => a.key.localeCompare(b.key));
}

const CTX_EXACT = ['messages', 'system', 'reasoning'] as const;
const CTX_ESTIMATED = ['toolcalls', 'agents', 'mcp', 'skills'] as const;

// `now` and `generatedAt` are two different instants and must stay that way.
// `now` is the instant the view being exported was built from: it fixes the
// window bounds and, through hourlyDayOf, the time block's grain, so a file
// taken from a render describes that render. `generatedAt` is when the file
// was asked for, which is the only honest thing to stamp on it — a tab left
// open all afternoon still renders the window it rendered, but the save is
// happening now. Both are parameters because this selector is pure: it reads
// no clock of its own, exactly like every other selector in this file.
export function selectReportInput(
  s: OverviewSnapshot,
  view: OverviewView,
  settings: Settings,
  now: Date = new Date(),
  generatedAt: Date = now,
): ReportInput {
  const win = windowOf(s.range, s.from, s.to, now);
  const summary = s.summary;
  // A Total window is unbounded, so it reports the Ledger's own extent.
  const fromIso = win.fromIso ?? s.firstIso;
  const toIso = win.toIso ?? s.lastIso;

  // The time block's first column names the grain of the rows beneath it,
  // which is NOT the Trend's display bucket (view.per): the chart aggregates a
  // long window to weeks or months, the file never does. A report carries the
  // finest honest grain it holds and lets the spreadsheet roll it up, so
  // pre-rolling into months here would destroy detail the file otherwise
  // keeps. That leaves two cases — a single-day window, where the store
  // already holds the very hours the screen is showing, and everything else,
  // which is daily. hourlyDayOf is the one place that rule runs, so the report
  // and the store's hourly fetch can never disagree about which window is
  // hourly; hours that have not landed fall back to the daily row rather than
  // writing an empty block. window_grain and this column read the same value
  // by construction.
  const hourly =
    hourlyDayOf(s.range, s.from, s.to, s.firstIso, s.lastIso, now) !== null && s.hourPoints.length > 0;
  const grain: Granularity = hourly ? 'hour' : 'day';

  const ctxCategories: ReportCtxCategory[] = [];
  const ctxTools: ReportInput['ctxTools'] = [];
  const ctxMcp: ReportInput['ctxMcp'] = [];
  const ctxSkills: ReportInput['ctxSkills'] = [];
  const ctxExec: ReportInput['ctxExec'] = [];

  // Every Source present in the window, so the file does not depend on which
  // card happened to be selected. A Source with usage but no Context capability
  // yields no rows rather than a row of zeros.
  for (const key of Object.keys(view.toolTotals).sort()) {
    const ctx = ctxTotals(view.rpts, key);
    for (const category of CTX_EXACT) {
      const v = ctx[category];
      if (v !== null) ctxCategories.push({ source: key, category, estTokens: v, basis: 'exact' });
    }
    for (const category of CTX_ESTIMATED) {
      const v = ctx[category];
      if (v !== null) ctxCategories.push({ source: key, category, estTokens: v, basis: 'estimated' });
    }
    const toolRows = s.ctxToolRows.filter((r) => r.source === key);
    for (const cat of toolTree(toolRows, ctx.toolcalls)) {
      for (const leaf of cat.tools) {
        ctxTools.push({ source: key, category: cat.label, name: leaf.name, estTokens: leaf.tokens, calls: leaf.calls });
      }
    }
    for (const m of mcpBars(toolRows, ctx.mcp)) {
      ctxMcp.push({ source: key, name: m.name, estTokens: m.tokens, calls: m.calls });
    }
    for (const sk of skillBars(s.ctxSkillRows, key)) {
      ctxSkills.push({ source: key, name: sk.name, estTokens: sk.tokens, uses: sk.uses });
    }
    for (const e of s.ctxExecRows.filter((r) => r.source === key)) {
      ctxExec.push({ source: key, exe: e.exe, cmd: e.cmd, estTokens: e.estTokens, calls: e.calls });
    }
  }

  return {
    generatedIso: generatedAt.toISOString(),
    fromIso,
    toIso,
    grain,
    tokensBasis: view.unreadable.length > 0 ? 'floor' : 'exact',
    // USD needs no conversion note; anything else does, and the figures below
    // stay USD either way (CONTEXT.md Display Currency).
    displayCurrency: settings.currency === 'USD' ? null : settings.currency,
    usdRate: settings.currency === 'USD' ? null : settings.usdRate,
    summary: {
      key: `${fromIso} .. ${toIso}`,
      inputTokens: summary?.inputTokens ?? 0,
      outputTokens: summary?.outputTokens ?? 0,
      cacheReadTokens: summary?.cacheReadTokens ?? 0,
      cacheWriteTokens: summary?.cacheWriteTokens ?? 0,
      totalTokens: summary?.totalTokens ?? 0,
      requests: summary?.requests ?? 0,
      sessions: summary?.convs ?? 0,
      cost: summary?.cost ?? null,
      hasUnpriced: summary?.hasUnpriced ?? false,
      unattributedTokens: summary?.unattributedTokens ?? 0,
      cacheEstimated: (summary?.cacheEstimatedModels.length ?? 0) > 0,
    },
    unpricedModels: summary?.unpricedModels ?? [],
    cacheEstimatedModels: summary?.cacheEstimatedModels ?? [],
    time: reportTimeRows(hourly ? s.hourPoints : view.rpts),
    sources: s.sourceRows.map((r) => reportRow(r, false)),
    models: s.modelRows.map((r) => reportRow(r, true)),
    projects: s.projectRows.map((r) => reportRow(r, false)),
    ctxCategories,
    ctxTools,
    ctxMcp,
    ctxSkills,
    ctxExec,
  };
}
