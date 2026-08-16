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
  execSignatures,
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

// Adapters often already name the Source (`grok: …`). The Overview still
// prefixes so a bare message is attributable, but must not double it.
function formatSourceScanError(source: string, error: string): string {
  const meta = sourceMeta(source);
  const prefixes = [source, meta.key, meta.label, meta.source]
    .filter((label, index, all) => label && all.indexOf(label) === index)
    .map((label) => `${label}:`);
  let message = error;
  for (const prefix of prefixes) {
    if (message.startsWith(prefix)) {
      message = message.slice(prefix.length).trimStart();
      break;
    }
  }
  return `${meta.source}: ${message}`;
}

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
  // A window-scoped reload is scheduled or in flight, so the fields it owns —
  // summary, the three breakdowns, every ctx row — still describe the PREVIOUS
  // window while range/from/to already name the new one. The headline uses the
  // series-derived total during this gap; other window-scoped figures and any
  // file export wait for this to clear.
  reloading: boolean;
  // Everything on screen predates a settled scan: the launch paint, or a paint
  // kept through a scan that threw. Cleared by the post-scan series refetch,
  // which is one query pair rather than the window fan-out — so this answers
  // "is this figure post-scan truth?" well before the Summary does. The
  // headline's entrance reel waits on it (#14 wants an authoritative figure);
  // the idle gate uses the private field it mirrors.
  provisional: boolean;
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
  'firstIso' | 'lastIso' | 'from' | 'to' | 'loading' | 'reloading' | 'provisional'
>;

const SNAP_KEYS: (keyof OverviewSnapshot)[] = [
  'allPoints', 'hourPoints', 'summary', 'profileSessions', 'modelRows', 'sourceRows', 'projectRows',
  'ctxResources', 'ctxBuckets', 'ctxToolRows', 'ctxSkillRows', 'ctxExecRows',
  'scanSources', 'scanError', 'scanAt', 'fetchError', 'range', 'customFrom', 'customTo', 'selected',
  'firstIso', 'lastIso', 'from', 'to', 'loading', 'reloading', 'provisional',
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
  // Reload results by window (filters + hourly day): returning to a window
  // already fetched since the last scan skips the 10-query fan-out and
  // repatches the exact same references, so the memoized panels don't
  // re-render either. Cleared at EVERY refresh — the idle gate's zero-insert
  // signal cannot stand in for "unchanged", because keep-max adapters upgrade
  // existing Usage Records in place while reporting nothing inserted — and
  // when prices rebuild. Only a response that is STILL CURRENT when it lands is
  // written here (runReload's own `epoch === this.epoch` guard, not land()'s
  // looser one), so a superseded response can never be replayed.
  private reloadCache = new Map<string, ReloadResult>();
  private epoch = 0; // monotonic; supersedes in-flight reload responses
  // The epoch whose reload actually landed. Reusing the counter that already
  // decides which response wins means "still loading" needs no second flag to
  // keep in sync with it: scheduleReload bumps epoch, land() catches this up.
  // 0 is therefore also the sentinel for "no window-scoped figures have landed
  // yet", which land() reads to let the launch's superseded first reload paint.
  // Safe as a sentinel because scheduleReload pre-increments: a scheduled epoch
  // is never 0, so this can only be 0 before the first landing.
  private loadedEpoch = 0;
  // The window the last ISSUED reload was for — issued, not landed, unlike
  // loadedEpoch above. The idle skip compares the current window against it,
  // because "nothing was ingested" does not mean "nothing on screen moved": a
  // range that ends at today names a DIFFERENT window once the day rolls over.
  private issuedKey: string | null = null;
  private reloadTimer: number | null = null; // pending debounce timer
  // The data on screen predates the last settled scan. Set when the first-load
  // paint fires, cleared only once a post-scan series refetch lands. A field,
  // not a local of refresh(): a scan can throw AFTER committing (the rejection
  // is IPC-level; per-source errors travel inside ScanStatus), and the next
  // tick's scan may then honestly report idle — the idle gate must still not
  // mistake the pre-scan paint for post-scan truth.
  private provisional = false;

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
    // Before anything else: a scan is about to run (or just failed partway),
    // so every cached window is suspect — see the cache's comment for why the
    // idle gate below is not a safe substitute.
    this.reloadCache.clear();

    // First load of this store: the Ledger already holds every record the
    // previous run captured, so paint that now instead of sitting on zeros
    // for the whole launch scan (the backend runs reads on a separate WAL
    // connection, so the scan's write hold cannot queue them). The reload is
    // scheduled on both outcomes, exactly as first loads always have: a
    // failure settles the series to [] and the window fetch still proceeds.
    let firstPaint: Promise<void> | null = null;
    if (this.state.allPoints === null) {
      this.provisional = true;
      firstPaint = this.fetchSeries().then(() => this.scheduleReload());
    }

    // One await for both: the scan's verdict may be a rejection, and the paint
    // must have landed before post-scan state is applied over it. fetchSeries
    // never rejects, so only the scan's slot needs inspecting.
    const [scanned] = await Promise.allSettled([this.ledger.scan(), firstPaint]);
    if (scanned.status === 'rejected') {
      this.state.scanError = String(scanned.reason);
      this.publish();
      return; // keep any paint; `provisional` stays set, so the next tick reconciles
    }
    const status = scanned.value;
    this.state.scanSources = status.sources;
    const errs = status.sources
      .filter((s) => s.error)
      .map((s) => formatSourceScanError(s.source, s.error!));
    this.state.scanError = errs.length ? errs.join(' · ') : null;
    // Backend reports epoch seconds (scan.rs as_secs); scanAt is epoch ms.
    this.state.scanAt = status.scannedAt ? status.scannedAt * 1000 : this.clock.now().getTime();
    this.publish();

    if (this.provisional) {
      // Everything on screen predates a settled scan — painted above, kept
      // through an earlier scan throw, or fetched by a mid-scan range click —
      // so it is suspect even when this scan reports idle: the gate's premise
      // ("what's rendered IS the Ledger") only holds for post-scan fetches,
      // and zero-insert ≠ unchanged (keep-max upgrades). Drop whatever got
      // cached, supersede any in-flight reload so nothing pre-scan can be
      // REPLAYED from here on, and refetch. A superseded reload may still
      // paint once (see land) — the launch would otherwise sit on placeholders
      // through figures it had already fetched — but `reloading` stays true and
      // `provisional` stays set until the refetch below lands, so nothing
      // pre-scan is presented as settled. The reload is rescheduled even when
      // the series refetch fails: the epoch bump would otherwise leave
      // `reloading` latched until the next tick.
      this.reloadCache.clear();
      this.epoch++;
      this.publish(); // `reloading` is true from the bump until the refetch lands
      // Do NOT publish between these two lines. fetchSeries publishes before
      // `provisional` clears, so the flip reaches the snapshot on
      // scheduleReload's trailing publish — the same one that carries the epoch
      // bump. That pairing is what the headline's entrance rests on: it reveals
      // on `authoritative` (provisional false) while `reloading` is true, so the
      // figure it rolls is the post-scan SERIES total. An eager publish here
      // would emit provisional false with reloading still false for one render,
      // and the reel would roll the pre-scan Summary instead (#14).
      if (await this.fetchSeries()) this.provisional = false;
      this.scheduleReload();
      return;
    }

    // Idle tick: nothing ingested, no source errored, data already on screen —
    // the Ledger is bit-identical to what's rendered, so skip the series fetch
    // and the 9-query reload. This is the every-30s steady state of an open
    // app; the skip is what lets it sit at ~0 CPU instead of re-rendering the
    // whole Overview each tick. Prices-rebuilt still forces a reload via its
    // own listener.
    // fetchError null required: a failed cycle must retry on the next tick
    // even when the scan reports nothing new.
    const idle = status.sources.every((s) => !s.error && s.eventsInserted === 0);
    if (
      idle &&
      this.state.allPoints !== null &&
      this.state.fetchError === null &&
      !this.windowMoved()
    ) {
      return;
    }

    if (await this.fetchSeries()) this.scheduleReload();
  }

  // Fetch the unbounded daily series + the Profile's Session count, publish,
  // and report whether the series landed. Scheduling the window reload is the
  // caller's decision — the policy differs per call site.
  private async fetchSeries(): Promise<boolean> {
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
      return true;
    } catch (e) {
      this.state.fetchError = String(e);
      // First load settles to [] so loading ends; later failures keep prior data.
      if (this.state.allPoints === null) this.state.allPoints = [];
      this.publish();
      return false;
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
    const unsub = this.ledger.onPricesRebuilt(() => {
      this.reloadCache.clear(); // a rate change reprices every cached window
      this.scheduleReload();
    });
    return () => {
      unsub();
      if (this.reloadTimer !== null) {
        this.clock.clearTimeout(this.reloadTimer);
        this.reloadTimer = null;
      }
      // Supersede any in-flight reload. This no longer stops one landing during
      // a launch (land's exception fires while loadedEpoch is 0), which is inert
      // rather than fixed: unsub() above and useSyncExternalStore's own
      // unsubscribe have both run, so a late patch reaches a snapshot nobody
      // reads and publishes to an empty listener set.
      this.epoch++;
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
      reloading: this.epoch !== this.loadedEpoch,
      provisional: this.provisional,
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

  // What a reload issued right now would fetch, and the identity of that fetch.
  // Derived in ONE place because two readers depend on it — the reload cache
  // keys on it, and windowMoved compares against it. Written twice, a changed
  // key shape would leave windowMoved matching nothing and reloading on every
  // idle tick, voiding the idle budget in docs/performance.md silently, with
  // every test still green.
  private windowNow(now: Date): { filters: Filters; hourDay: string | null; key: string } {
    const d = this.derive(now);
    const filters = rangeToFilters(this.state.range, d.from, d.to);
    const hourDay = hourlyDayOf(this.state.range, d.from, d.to, d.firstIso, d.lastIso, now);
    return { filters, hourDay, key: JSON.stringify([filters, hourDay]) };
  }

  // Whether the window the app would fetch now differs from the one it last
  // fetched. Total is unbounded and a Custom range with both ends set is fixed,
  // so neither moves. Every other window is anchored to today — Day, Week,
  // Month, and a Custom left open-ended, whose missing end falls back to today
  // in derive() — so each names a new window the moment the local day turns
  // over, with nothing ingested to announce it. Without this an idle machine
  // sits on yesterday's figures under a TODAY label for as long as it stays idle.
  private windowMoved(): boolean {
    if (this.issuedKey === null) return false; // nothing fetched yet to be stale
    return this.windowNow(this.clock.now()).key !== this.issuedKey;
  }

  private scheduleReload() {
    if (this.state.allPoints === null) return; // no per-range fetch until first load lands
    if (this.reloadTimer !== null) this.clock.clearTimeout(this.reloadTimer);
    const epoch = ++this.epoch;
    const s = this.state;
    // State can't change between schedule and fire without rescheduling, so
    // capturing filters/hourDay here is equivalent to computing them at fire.
    // The key is recorded now rather than on landing: a failed reload sets
    // fetchError, which the idle skip already refuses to skip on, so the retry
    // is covered either way.
    const { filters, hourDay, key } = this.windowNow(this.clock.now());
    this.issuedKey = key;
    const delay = s.range === 'custom' ? 250 : 0;
    this.reloadTimer = this.clock.setTimeout(() => {
      this.reloadTimer = null;
      this.runReload(epoch, filters, hourDay, key);
    }, delay);
    // The epoch bump above is what makes `reloading` true, and every caller
    // published BEFORE calling here — so without this the new window would be
    // on screen with nothing announcing that its figures have not caught up.
    this.publish();
  }

  // All jobs land as ONE patch/publish: nine individual landings would
  // re-render the dashboard nine times per reload (visible as a long-task
  // burst every refresh tick).
  private runReload(epoch: number, filters: Filters, hourDay: string | null, key: string) {
    // Both the success and the failure path land, so a reload that throws
    // clears `reloading` too — fetchError is how a failure is reported, and
    // leaving the flag set would disable the export button for good.
    // The current epoch always lands. A SUPERSEDED one lands too while no
    // window-scoped figures have landed yet (loadedEpoch 0 — the series has
    // always painted by then, since scheduleReload requires it), because boot
    // supersedes its own first reload before it can answer: the post-scan
    // reconcile bumps the epoch in the microtask that follows the paint (and
    // prices-rebuilt can bump it in the same stretch). Discarding that left the
    // '…' cost and the '—' Context rows on screen until the SECOND fan-out
    // landed — measured at 2.7s on a 95k-event Ledger, after the first had
    // already paid for the same figures. Once a window HAS landed,
    // only the current epoch may overwrite it: a rapid range walk must not
    // flash the figures of windows it passed through.
    const land = (fn: () => void) => {
      if (epoch !== this.epoch && this.loadedEpoch !== 0) return;
      this.loadedEpoch = epoch;
      fn();
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
    const apply = ([summary, modelRows, projectRows, ctxResources, ctxBuckets, ctxToolRows, ctxSkillRows, ctxExecRows, hour, sourceRows]: ReloadResult) =>
      this.patch({
        summary, modelRows, projectRows, ctxResources, ctxBuckets, ctxToolRows, ctxSkillRows, ctxExecRows, sourceRows,
        ...(hour ? { hourPoints: hour } : {}),
        fetchError: null,
      });
    const cached = this.reloadCache.get(key);
    if (cached) {
      land(() => apply(cached));
      return;
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
      .then((result) =>
        land(() => {
          // Only a CURRENT response is worth remembering. A superseded one is
          // painted (above) for the launch's sake, but caching it would let a
          // pre-scan read be replayed as post-scan truth the next time its
          // window comes back — zero-insert ≠ unchanged.
          if (epoch === this.epoch) {
            this.reloadCache.set(key, result);
            // A session can walk arbitrarily many custom windows; drop the
            // oldest entry rather than growing without bound.
            if (this.reloadCache.size > 16) {
              const oldest = this.reloadCache.keys().next();
              if (!oldest.done) this.reloadCache.delete(oldest.value);
            }
          }
          apply(result);
        }),
      )
      // A failure has nothing to paint, so land()'s launch exception must not
      // apply to it: a superseded reload's error would flash a band that the
      // newer reload already in flight clears half a second later. Dropping it
      // leaves `reloading` true, which that newer reload clears when it lands —
      // or latches honestly if it fails too, since its own error IS current.
      .catch((e) => {
        if (epoch !== this.epoch) return;
        land(() => this.patch({ fetchError: String(e) }));
      });
  }
}

// What one reload fetches, in Promise.all order: summary, the model and
// project breakdowns, the five ctx reads, the optional hourly series, and
// last the Source breakdown.
type ReloadResult = [
  Summary,
  BreakdownRow[],
  BreakdownRow[],
  CtxResource[],
  CtxBuckets[],
  CtxToolRow[],
  CtxSkillRow[],
  CtxExecRow[],
  SeriesPoint[] | null,
  BreakdownRow[],
];

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
  headline: { total: number; authoritative: boolean };
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
    // `authoritative` is #14's own word for the figure the entrance reel is
    // allowed to roll: one that descends from a settled scan, so the launch
    // reconcile cannot correct it a moment later with no motion (#12 story 9
    // holds a same-window change still, and the entrance is spent once). The
    // post-scan SERIES earns it, not the Summary: the series is what `total`
    // reads while the window fan-out is in flight, and it lands one query pair
    // after the scan rather than ten. A Summary alone would not do — the
    // launch's provisional one describes a pre-scan Ledger.
    headline: {
      total: s.reloading ? total : s.summary?.totalTokens ?? total,
      authoritative: !s.provisional && s.allPoints !== null,
    },
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
      // SeriesPoint carries no cache-estimated flag, so this block cannot know
      // whether a bucket's cache figures were logged or reconstructed. null
      // writes an empty cell, which is how the file says "unknown" — a `false`
      // here would read as a definite claim beneath a summary row saying true.
      hasUnpriced: false, unattributedTokens: 0, cacheEstimated: null,
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
  // The held hours must be the SAME day, not merely present: runReload clears
  // stale hourPoints, but that runs after the debounce (250ms for a custom
  // range), so a snapshot in that gap carries the new bounds beside the
  // previous day's hours with loading already false. Comparing days is the
  // idiom runReload itself uses.
  const hourDay = hourlyDayOf(s.range, s.from, s.to, s.firstIso, s.lastIso, now);
  const hourly = hourDay !== null && s.hourPoints[0]?.bucket.slice(0, 10) === hourDay;
  const grain: Granularity = hourly ? 'hour' : 'day';

  const ctxCategories: ReportCtxCategory[] = [];
  const ctxTools: ReportInput['ctxTools'] = [];
  const ctxMcp: ReportInput['ctxMcp'] = [];
  const ctxSkills: ReportInput['ctxSkills'] = [];
  const ctxExec: ReportInput['ctxExec'] = [];

  // Every Source in the catalog — toolTotalsOfPoints seeds them all, so a
  // Source absent from the window is walked too and simply contributes nothing:
  // ctxTotals reports null for every category and each helper below returns []
  // for a Source it has no rows for. The file therefore does not depend on which
  // card happened to be selected, and a Source with usage but no Context
  // capability yields no rows rather than a row of zeros.
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
    const tree = toolTree(toolRows, ctx.toolcalls);
    for (const cat of tree) {
      for (const leaf of cat.tools) {
        ctxTools.push({ source: key, category: cat.label, name: leaf.name, estTokens: leaf.tokens, calls: leaf.calls });
      }
    }
    for (const m of mcpBars(toolRows, ctx.mcp)) {
      ctxMcp.push({ source: key, name: m.name, estTokens: m.tokens, calls: m.calls });
    }
    // The raw rows, not skillBars: that helper is card layout — it keeps the
    // top ten and folds the rest into one nameless row, which here would drop
    // skills 11..N from a report whose whole point is completeness and write a
    // total row into a Context block with an empty first cell meaning "unknown".
    for (const sk of s.ctxSkillRows.filter((r) => r.source === key)) {
      ctxSkills.push({ source: key, name: sk.name, estTokens: sk.estTokens, uses: sk.uses });
    }
    // Allocated against the Bash leaf, exactly as the drill-down allocates its
    // facets: the raw ctx_exec weights are content sizes, while the leaf holds
    // that Source's share of the billed Tool-calls total, so writing the raw
    // figures would put this block on a scale no other block in the file — and
    // no panel the file was taken from — is on.
    const bashLeaf = tree.flatMap((c) => c.tools).find((leaf) => leaf.name === 'Bash') ?? null;
    for (const e of execSignatures(s.ctxExecRows.filter((r) => r.source === key), bashLeaf?.tokens ?? null)) {
      ctxExec.push({ source: key, exe: e.exe, cmd: e.cmd, estTokens: e.tokens, calls: e.calls });
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
