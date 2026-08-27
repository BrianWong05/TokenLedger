// The Menu Bar Extra panel (ADR-0007), in the "Glance + Models · bars" design
// (TOKL-19 added the Limits, and the ADR's second amendment capped the panel):
// period tabs top-left double as the window's label, the last-scan time and
// Rescan sit top-right, and beneath the hero Cost sit the stacked source bar
// with its legend, the Cost-per-bucket drawing (columns by default, a
// line-and-peak sparkline behind a toggle on the caption row), the Model
// rows (led by their Source's mark), one Limits card per live Source
// (collapsed to its Session + Weekly meters, expandable to every window),
// the stat tiles, and the three icon actions. The lists are capped and
// say what they hid, because the window hugs its content and the app holds the
// full story — see panelModel's caps. Every figure is a display string the
// view model already decided. Data goes through the same ports the app shell
// uses; the four actions are Tauri glue.
// UI strings are English-only like the native menu it replaced; number and
// currency formatting still follow the app's locale. Dark-only like the mock.
import { useCallback, useEffect, useMemo, useRef, useState, type MouseEvent as ReactMouseEvent } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window';
import { detectPlatform, type Platform } from '../lib/platform';
import { hotkeyHint, isHotkey } from '../lib/hotkeys';
import { panelModel, periodWindows, seriesBucket, type PanelModel, type Period } from './panelModel';
import { tauriLedger, type LedgerPort } from '../overview/ledger';
import { tauriSettings, type SettingsPort } from '../settings/settings';
import { tauriLimits, MODE_KEY, PANEL_CARDS_KEY, PANEL_WINDOWS_KEY, type LimitsPort } from '../limits/limits';
import { runDueLiveChecks, storedFailures, type LiveFailure } from '../limits/limits.live';
import {
  cards, durationParts, panelCardOrder, panelWindows, parsePanelCardPick,
  parsePanelPicks, planLabel, windowLabel,
  type Mode as LimitsMode, type ParsedWindow, type WindowView,
} from '../limits/limits.derive';
import { limits as limitStrings } from '../lib/strings/limits';
import { windowText, type LimitT } from '../limits/limitLabel';
import { fill } from '../lib/format';
import { sourceIcon } from '../overview/icons';
import { NO_FLOOR, tokenFloor, markedTokenFigure, type TokenFloor } from '../overview/localize';
import type { Filters, SourceLimits } from '../types';
import './TrayPanel.css';

export interface TrayPanelPorts {
  ledger?: LedgerPort;
  settings?: SettingsPort;
  limits?: LimitsPort;
}

const PANEL_WIDTH = 320;

// Bar-chart geometry: the model hands over one normalised value per bucket,
// the view splits its 288×56 box into one column per bucket. A bucket with
// usage never rounds below a visible sliver, an idle bucket draws nothing,
// and a young period's lone bucket stays a column rather than flooding the
// box (the cap only ever binds when buckets are few). Exported for its test.
const CHART_W = 288;
const CHART_H = 56;
export function barRect(points: number[], i: number): string {
  const slot = CHART_W / points.length;
  const w = Math.max(2, Math.min(slot - 2, 28));
  const x = i * slot + (slot - w) / 2;
  const h = points[i] > 0 ? Math.max(1.2, points[i] * (CHART_H - 6)) : 0;
  return h > 0
    ? `M${x.toFixed(1)} ${CHART_H - 2} v-${h.toFixed(1)} h${w.toFixed(1)} v${h.toFixed(1)} Z`
    : '';
}
function barsPath(points: number[], skipIdx: number): string {
  return points
    .map((_, i) => (i === skipIdx ? '' : barRect(points, i)))
    .filter(Boolean)
    .join(' ');
}
// The hover indicator is the bucket's whole slot, drawn behind the columns:
// an idle bucket has no column to brighten, but its slot still lights up
// while the read-out says its zero.
function slotRect(points: number[], i: number): string {
  const slot = CHART_W / points.length;
  return `M${(i * slot).toFixed(1)} 0 h${slot.toFixed(1)} v${CHART_H} h-${slot.toFixed(1)} Z`;
}

// Line geometry lives in the same 288×56 box as the columns, so switching
// drawings never resizes the window. The inset keeps the stroke and the
// now-dot off the box edges. A lone bucket stays on the left pad — the
// sparkline this restores parked it there; last=max(1, n-1) is what does it.
const SPARK_PAD = 3;
export function sparkXY(points: number[], i: number): [number, number] {
  const last = Math.max(1, points.length - 1);
  return [
    SPARK_PAD + (i / last) * (CHART_W - SPARK_PAD * 2),
    CHART_H - 4 - points[i] * (CHART_H - 10),
  ];
}
function sparkPath(points: number[]): string {
  return points
    .map((_, i) => {
      const [x, y] = sparkXY(points, i);
      return `${i ? 'L' : 'M'}${x.toFixed(1)} ${y.toFixed(1)}`;
    })
    .join(' ');
}

// Columns vs the sparkline the redesign replaced. Stored so the choice
// survives the panel being destroyed on dismiss (ADR-0007).
type CostDrawing = 'columns' | 'line';
export const PANEL_COST_DRAWING_KEY = 'tokenledger.panelCostDrawing';
function loadCostDrawing(): CostDrawing {
  try {
    return localStorage.getItem(PANEL_COST_DRAWING_KEY) === 'line' ? 'line' : 'columns';
  } catch {
    return 'columns';
  }
}
function saveCostDrawing(drawing: CostDrawing) {
  try {
    localStorage.setItem(PANEL_COST_DRAWING_KEY, drawing);
  } catch {
    /* storage disabled: the choice does not survive dismiss */
  }
}

function CostDrawingToggle({
  value,
  onChange,
}: {
  value: CostDrawing;
  onChange: (d: CostDrawing) => void;
}) {
  return (
    <div className="tp-chart-styles" role="radiogroup" aria-label="Cost per bucket drawing">
      <button
        type="button"
        role="radio"
        aria-checked={value === 'columns'}
        aria-label="Columns"
        title="Columns"
        className={value === 'columns' ? 'tp-chart-style on' : 'tp-chart-style'}
        onClick={() => onChange('columns')}
      >
        <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
          <rect x="1" y="7" width="2.5" height="4" rx="0.5" fill="currentColor" />
          <rect x="4.75" y="2" width="2.5" height="9" rx="0.5" fill="currentColor" />
          <rect x="8.5" y="5" width="2.5" height="6" rx="0.5" fill="currentColor" />
        </svg>
      </button>
      <button
        type="button"
        role="radio"
        aria-checked={value === 'line'}
        aria-label="Line"
        title="Line"
        className={value === 'line' ? 'tp-chart-style on' : 'tp-chart-style'}
        onClick={() => onChange('line')}
      >
        <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
          <polyline
            points="1,9 4,5 7,7 11,2"
            stroke="currentColor"
            strokeWidth="1.4"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
          <circle cx="11" cy="2" r="1.3" fill="currentColor" />
        </svg>
      </button>
    </div>
  );
}

// Fire-and-forget IPC for the actions; harmless outside Tauri (tests).
function ipc(cmd: string) {
  Promise.resolve()
    .then(() => invoke(cmd))
    .catch(() => {});
}

// The panel is English-only, but its Limits vocabulary is not its own: labels,
// pool names and duration units read straight off the dictionary's `en` table
// — the same keys the Limits page renders through `t()` — so the words have
// one home and cannot drift.
const LIMITS_EN = limitStrings.en;
// The panel's translator: the dictionary read straight, since this window ships
// English only (see above).
const EN: LimitT = (key) => LIMITS_EN[key];

// "1d 6h" — durationParts' largest two units, through the dictionary's own
// unit templates (the same `as` the page's fmtDuration uses, for the same
// reason: both keys stay checked against the dictionary).
function limitDuration(minutes: number): string {
  return durationParts(minutes)
    .map((p) => fill(LIMITS_EN[`limits.t.${p.unit}` as 'limits.t.d'], { n: p.n }))
    .join(' ');
}

// One limit bar, shared by the Source lines and the expanded rows: the
// framed fill plus the time tick. The tick is TIME (where "now" sits in the
// window), so it stays neutral — never tinted by the scarcity tones around it.
function LimitBar({ w, mode }: { w: WindowView; mode: LimitsMode }) {
  return (
    <div
      className="tp-limbar"
      role="img"
      aria-label={`${w.pctShown}% ${mode === 'left' ? 'left' : 'used'}`}
    >
      <span className={'fill ' + w.tone} style={{ width: `${w.pct}%` }} />
      {w.tickPct !== null && <span className="tick" style={{ left: `${w.tickPct}%` }} />}
    </div>
  );
}

// Tween toward `target` with an ease-out, so the header figures count up
// instead of jumping. Snaps instantly under prefers-reduced-motion — and in
// jsdom, which has no matchMedia — keeping tests deterministic and motion
// accessible.
function useCountUp(target: number, duration = 600): number {
  const [value, setValue] = useState(target);
  const shown = useRef(target);
  useEffect(() => {
    const reduce =
      typeof window.matchMedia !== 'function' ||
      window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    const from = shown.current;
    if (reduce || from === target) {
      shown.current = target;
      setValue(target);
      return;
    }
    let raf: number;
    const t0 = performance.now();
    const step = (t: number) => {
      const p = Math.min(1, (t - t0) / duration);
      const eased = 1 - (1 - p) ** 3;
      shown.current = from + (target - from) * eased;
      setValue(shown.current);
      if (p < 1) raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
  }, [target, duration]);
  return value;
}

export default function TrayPanel({
  ports,
  platform = detectPlatform(),
}: { ports?: TrayPanelPorts; platform?: Platform } = {}) {
  // The panel opens wherever the tray delivers a click — macOS and Windows
  // (ADR-0010) — and the two spell a modifier differently. Hint and chord come
  // from one table, so what is printed is what fires.
  const keys = {
    rescan: hotkeyHint('rescan', platform),
    settings: hotkeyHint('settings', platform),
    quit: hotkeyHint('quit', platform),
  };
  const ledger = ports?.ledger ?? tauriLedger;
  const settings = ports?.settings ?? tauriSettings;
  const limits = ports?.limits ?? tauriLimits;
  const [model, setModel] = useState<PanelModel | null>(null);
  // The Limit cards are *now*, not the selected period (same rule as the
  // Limits page): stored Readings plus the clock they were read at, and which
  // Sources the user has flipped open. The panel is destroyed on dismissal
  // (ADR-0007), so the open set naturally resets per open.
  const [limitsStored, setLimitsStored] = useState<SourceLimits[]>([]);
  const [limitsFailures, setLimitsFailures] = useState<Record<string, LiveFailure>>({});
  const [limitsNow, setLimitsNow] = useState(() => Math.floor(Date.now() / 1000));
  const [limitsOpen, setLimitsOpen] = useState<Record<string, boolean>>({});
  // The selected window's token figure is a floor (≥) when an Unreadable
  // Artifact could hold usage in it (ADR-0017) — the same tokenFloor shape
  // every ≥ surface carries; reason rides along as the marker's hover text.
  const [tokensFloor, setTokensFloor] = useState<TokenFloor>(NO_FLOOR);
  const [scanning, setScanning] = useState(false);
  // The gate rescan() reads, mirroring useAutoRefresh's busyRef: `scanning`
  // is for the view, and a memoized rescan() would close over a stale copy of
  // it forever — while listing it as a dep would re-fire the open effect.
  const scanningRef = useRef(false);
  const [loading, setLoading] = useState(true);
  const [period, setPeriod] = useState<Period>('today');
  // The chart's hover inspector: which bucket the pointer is over, null
  // when the mouse is away. Same hit-test for both drawings.
  const [chartHover, setChartHover] = useState<number | null>(null);
  // Columns vs line — loaded once; the panel is destroyed on dismiss, so a
  // later open re-reads storage. Invalid or missing values stay columns.
  const [drawing, setDrawing] = useState<CostDrawing>(loadCostDrawing);
  // refresh() reads the ref so its identity doesn't churn on period change
  // (the mount effect re-registering listeners on every switch would be
  // wasteful); pickPeriod keeps ref and state in step.
  const periodRef = useRef<Period>('today');
  const bodyRef = useRef<HTMLDivElement>(null);
  // Count-up for the two headline figures; unpriced (null) doesn't animate.
  const animCost = useCountUp(model?.costValue ?? 0);
  const animTokens = useCountUp(model?.tokensValue ?? 0);

  // On panel open the skeleton stays up at least this long, so the load
  // reads as a deliberate beat instead of a flash. Zero under
  // reduced-motion (and in jsdom) — the wait is pure show.
  const minLoadingMs = () =>
    typeof window.matchMedia !== 'function' ||
    window.matchMedia('(prefers-reduced-motion: reduce)').matches
      ? 0
      : 1000;

  // The stored-Readings read, re-issued by every refresh and by a scan
  // elsewhere landing relevant changes. Promise.resolve guards a port whose
  // IPC throws synchronously when no Tauri runtime is present.
  const loadLimits = useCallback(() => {
    setLimitsNow(Math.floor(Date.now() / 1000));
    // The verdicts the floor still holds ride along with every read, so a
    // signed-out or erroring Source keeps its card off the panel (its
    // trouble state renders on the app's Limits page instead).
    setLimitsFailures(storedFailures(limits, Date.now()));
    try {
      // The Array guard covers an IPC that answers but answers nothing (the
      // tests' recording invoke mock does exactly that) — an absent runtime
      // must read as "no Readings", never crash the derivation.
      Promise.resolve(limits.list())
        .then((s) => setLimitsStored(Array.isArray(s) ? s : []))
        .catch(() => {});
    } catch { /* no runtime */ }
  }, [limits]);

  const refresh = useCallback(async (showLoading = false) => {
    if (showLoading) setLoading(true);
    loadLimits();
    const w = periodWindows(periodRef.current, new Date());
    const current: Filters = { tools: [], models: [], project: null, startTs: w.start, endTs: w.end };
    const prev: Filters = { tools: [], models: [], project: null, startTs: w.prevStart, endTs: w.prevEnd };
    const [fetched] = await Promise.all([
      Promise.allSettled([
        Promise.all([
          ledger.summary(current),
          ledger.summary(prev),
          ledger.breakdown('tool', current),
          settings.get(),
          ledger.breakdown('model', current),
          ledger.series(current, seriesBucket(periodRef.current)),
          ledger.lastScan(),
          ledger.unreadableArtifacts(),
        ]),
      ]),
      new Promise((r) => setTimeout(r, showLoading ? minLoadingMs() : 0)),
    ]);
    if (fetched[0].status === 'fulfilled') {
      const [t, y, rows, s, models, series, scannedAt, sources] = fetched[0].value;
      const lang = s.language === 'zh-Hant' ? 'zh-Hant' : 'en';
      // Today runs to now, so the window has no end to rule anything out.
      setTokensFloor(tokenFloor(sources, w.start, lang));
      setModel(
        panelModel(t, y, rows, s, lang, {
          period: periodRef.current,
          now: new Date(),
          models,
          series,
          scannedAt,
        }),
      );
      // A fresh model can hold fewer buckets than the inspected index (period
      // switch, midnight rollover mid-hover); never let the index outlive them.
      setChartHover(null);
    }
    // Ledger unavailable (e.g. mid-restart): keep the last model.
    if (showLoading) setLoading(false);
  }, [ledger, settings, loadLimits]);

  // Mark the document so TrayPanel.css can force the window transparent
  // over index.css's app background, and so the translucent card is scoped
  // to the one platform that paints a material behind it (see the css
  // header comment).
  useEffect(() => {
    document.body.classList.add('tp-window', `tp-${platform}`);
    return () => document.body.classList.remove('tp-window', `tp-${platform}`);
  }, [platform]);

  // Scan, then re-read what it added. The refresh button's action, and the
  // second half of every open. Only the scan is gated — coalescing the read
  // as well would leave an open that lands mid-scan showing the figures from
  // the open before it. The release is in `finally` because a throw out of
  // `refresh` would otherwise strand the gate: a permanently disabled Rescan
  // and every later open swallowed (the bug useAutoRefresh's gate records).
  const rescan = useCallback(async () => {
    if (scanningRef.current) return;
    scanningRef.current = true;
    setScanning(true);
    // Live limits ride the same "a person asked" moments as the Limits page:
    // panel open (open() rescans) and the Rescan press. runDueLiveChecks
    // gates on the page's opt-in and the per-Source floor; fresh Readings
    // land in the store, so the settle re-reads it. Never rejects, and
    // deliberately not awaited — a slow vendor must not hold the scan
    // spinner. If this window is destroyed mid-check the verdict goes
    // unwritten; the floor bounds that window (see runDueLiveChecks).
    runDueLiveChecks(limits, Date.now())?.then(loadLimits);
    // A fast no-change scan must still visibly happen: hold the spinner and
    // the pulsing figures for ≥1s.
    const minDone = new Promise((r) => setTimeout(r, minLoadingMs()));
    try {
      // Scan errors surface in the Overview, not here — but the re-read still
      // runs, so a failed scan doesn't also freeze the figures.
      await ledger.scan().catch(() => {});
      await refresh();
      await minDone;
    } finally {
      scanningRef.current = false;
      setScanning(false);
    }
  }, [ledger, refresh, limits, loadLimits]);

  // What an open is: paint what the Ledger already holds, then bring it
  // current, so nobody has to press Rescan for today's figures. The read does
  // not wait on the scan — reads have their own connection (lib.rs's read_db)
  // precisely so a scan cannot queue a paint — and the scan's own re-read
  // repaints when it lands, including the "scanned" read-out.
  const open = useCallback(async () => {
    await refresh(true);
    await rescan();
  }, [refresh, rescan]);

  // Initial load + every time the tray shows the panel. The panel is created
  // on demand and destroyed on dismissal (ADR-0007), so mount is the usual
  // open; panel-shown covers a reused window.
  useEffect(() => {
    void open();
    let un: (() => void) | undefined;
    Promise.resolve()
      .then(() => listen('panel-shown', () => void open()))
      .then((f) => { un = f; })
      .catch(() => {});
    return () => un?.();
  }, [open]);

  // A scan elsewhere — the resident cadence, another window's Scan now — that
  // landed relevant Reading changes re-issues the stored read. No vendor is
  // called: a scan is not a person asking.
  useEffect(() => limits.onLimitsChanged(loadLimits), [limits, loadLimits]);

  // A shortcut the panel prints beside an action is a working promise
  // (CONTEXT.md): each key fires the very handler its button does — exact
  // modifiers only, the modifier spelt per platform like KEY_HINTS. On the
  // document, because the panel has no focused element to key off.
  // ⌘Q is the one macOS never delivers here: Tauri installs the default
  // application menu (which is what makes ⌘C/⌘V work), and a menu key
  // equivalent is resolved before the webview sees the key — so there the menu
  // quits and this branch is the Windows one, where no such menu exists.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Escape unqualified, as the app's dialogs take it (useDialogChrome).
      if (e.key === 'Escape') {
        e.preventDefault();
        ipc('close_panel'); // dismissed exactly like focus loss: destroyed, not hidden
        return;
      }
      if (isHotkey(e, 'rescan', platform)) {
        e.preventDefault();
        void rescan(); // gated inside like the button: a scan in flight swallows it
      } else if (isHotkey(e, 'settings', platform)) {
        e.preventDefault();
        ipc('open_settings'); // focus moves to main; the blur path closes the panel
      } else if (isHotkey(e, 'quit', platform)) {
        e.preventDefault();
        ipc('quit_app');
      }
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [platform, rescan]);

  // Size the window to the rendered content on every height change —
  // skeleton → figures, period switches, row counts. Measuring on a state
  // dep is not enough: the model lands while the skeleton still shows, and
  // the taller real content would never get re-measured (the scroll bug).
  // The resize goes through the resize_panel command — the Rust side is
  // authoritative — with the JS window API as belt-and-braces. jsdom has no
  // ResizeObserver; window sizing is Tauri glue anyway.
  useEffect(() => {
    const el = bodyRef.current;
    if (!el || typeof ResizeObserver !== 'function') return;
    const ro = new ResizeObserver(() => {
      const h = el.offsetHeight;
      if (!h) return;
      Promise.resolve()
        .then(() => invoke('resize_panel', { height: h }))
        .catch(() => {});
      Promise.resolve()
        .then(() => getCurrentWindow().setSize(new LogicalSize(PANEL_WIDTH, h)))
        .catch(() => {});
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Hit-testing: the svg stretches (preserveAspectRatio none), so box x is
  // proportional to viewBox x, and each bucket owns an equal slot of it.
  const onChartMove = (e: ReactMouseEvent<SVGSVGElement>) => {
    const chart = model?.chart;
    const rect = e.currentTarget.getBoundingClientRect();
    if (!chart || !rect.width) return;
    const vx = ((e.clientX - rect.left) / rect.width) * CHART_W;
    const i = Math.floor(vx / (CHART_W / chart.points.length));
    setChartHover(Math.max(0, Math.min(chart.points.length - 1, i)));
  };

  const pickPeriod = (p: Period) => {
    if (p === periodRef.current) return;
    periodRef.current = p;
    setPeriod(p);
    void refresh(); // no skeleton beat on a switch — it should feel snappy
  };

  const pickCostDrawing = (d: CostDrawing) => {
    if (d === drawing) return;
    setDrawing(d);
    saveCostDrawing(d);
  };

  // The inspected bucket and the latest bucket's line positions; null while
  // that drawing is off or (for hover) the pointer is away. The sparkline
  // this restores marks "now" (the last bucket), not the peak — the peak
  // lives in the caption.
  const lineHoverXY =
    drawing === 'line' && model?.chart && chartHover != null
      ? sparkXY(model.chart.points, chartHover)
      : null;
  const lineNowXY =
    drawing === 'line' && model?.chart
      ? sparkXY(model.chart.points, model.chart.points.length - 1)
      : null;

  const barSlices = model?.rows.filter((r) => (r.share ?? 0) > 0) ?? [];

  // Only Sources whose card still carries windows render one: live cards, and
  // error cards holding earlier Readings (a refused check must not make a
  // Source vanish from the panel — the failure's text stays on the app's
  // Limits page, the figures stay here). Signed-out and nothing-recorded cards
  // carry none, so the stored verdicts still take those cards down.
  // Framing follows the page's stored Left/Used choice — the panel adds no
  // second toggle.
  const limitsMode: LimitsMode = limits.read(MODE_KEY) === 'used' ? 'used' : 'left';
  // Which windows each Source shows collapsed, and in which order the cards
  // themselves stack — both chosen in Settings. Read on every render like the
  // framing above, so a change made in the app's window lands the next time
  // this panel draws — the two webviews share the store but not its events.
  const panelPicks = parsePanelPicks(limits.read(PANEL_WINDOWS_KEY));
  const panelCardPick = parsePanelCardPick(limits.read(PANEL_CARDS_KEY));
  const catalogCards = useMemo(
    () =>
      cards(limitsStored, limitsNow, limitsMode, limitsFailures).filter(
        (c) => c.windows.length > 0,
      ),
    [limitsStored, limitsNow, limitsMode, limitsFailures],
  );
  const limitCards = panelCardOrder(
    catalogCards.map((c) => c.source),
    panelCardPick,
  ).flatMap((key) => catalogCards.filter((c) => c.source === key));

  return (
    <div className="tp" ref={bodyRef}>
      <div className="tp-top">
        <div className="tp-seg" role="tablist">
          {(
            [
              ['today', 'Today'],
              ['yesterday', 'Yesterday'],
              ['days30', '30 days'],
            ] as [Period, string][]
          ).map(([p, label]) => (
            <button
              key={p}
              role="tab"
              aria-selected={period === p}
              className={period === p ? 'tp-seg-btn active' : 'tp-seg-btn'}
              onClick={() => pickPeriod(p)}
            >
              {label}
            </button>
          ))}
        </div>
        <div className="tp-top-right">
          {/* The last-scan time, beside the button that changes it. */}
          {model?.stats && <span className="tp-scanned">{model.stats.scanned}</span>}
          {/* aria-disabled, not disabled: a disabled control shows no tooltip,
              and the hotkey hint must not vanish exactly while the icon spins.
              rescan() gates itself, so the click is already inert mid-scan. */}
          <button
            className="tp-refresh"
            title={`Rescan now (${keys.rescan})`}
            aria-label="Rescan now"
            aria-disabled={scanning || undefined}
            aria-busy={scanning || undefined}
            onClick={() => void rescan()}
          >
            {/* 1b's refresh glyph, spinning while the scan runs. */}
            <svg
              className={scanning ? 'tp-spin' : undefined}
              width="13"
              height="13"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
              aria-hidden="true"
            >
              <path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8" />
              <path d="M21 3v5h-5" />
              <path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16" />
              <path d="M8 16H3v5" />
            </svg>
          </button>
        </div>
      </div>

      {loading ? (
        <div className="tp-figures">
          <span className="tp-skel tp-skel-cost" />
          <span className="tp-skel tp-skel-sub" />
        </div>
      ) : (
        <div className={scanning ? 'tp-figures tp-pulse' : 'tp-figures'}>
          <div className="tp-cost-row">
            <span className="tp-cost">
              {model ? (model.costValue === null ? model.cost : model.fmtCost(animCost)) : '…'}
            </span>
            {model?.delta && (
              <span className={model.deltaUp ? 'tp-delta up' : 'tp-delta down'}>{model.delta}</span>
            )}
          </div>
          <span className="tp-sub" title={tokensFloor.reason || undefined}>
            {/* Tokens only — the requests figure lives in its stat tile, and
                saying it twice two lines apart bought nothing. */}
            {model ? `${markedTokenFigure(model.fmtTokens(animTokens), tokensFloor)} tokens` : ''}
          </span>
        </div>
      )}

      {loading && (
        <>
          <span className="tp-skel tp-skel-bar" />
          <span className="tp-skel tp-skel-chart" />
          <div className="tp-tiles">
            {[0, 1].map((i) => (
              <span className="tp-skel tp-skel-tile" key={i} />
            ))}
          </div>
        </>
      )}

      {!loading && model && model.rows.length > 0 && (
        <div className="tp-sources">
          {/* The bar splits the period's priced Cost; a period with none
              (all-Unpriced, all-Unattributed) keeps the legend alone. */}
          {barSlices.length > 0 && (
            <div className="tp-srcbar">
              {barSlices.map((r) => (
                <span key={r.key} style={{ width: `${(r.share ?? 0) * 100}%`, background: r.color }} />
              ))}
            </div>
          )}
          <div className="tp-legend">
            {model.legend.map((r) => (
              <div className="tp-leg" key={r.key}>
                <span className="tp-leg-name">
                  {r.icon && <img src={r.icon} alt="" width={12} height={12} />}
                  {r.label}
                </span>
                <span className="tp-leg-cost">{r.cost}</span>
              </div>
            ))}
          </div>
          {model.legendOverflow > 0 && (
            <div className="tp-more">+{model.legendOverflow} more Sources in the app</div>
          )}
        </div>
      )}

      {!loading && !model?.empty && model?.chart && (
        <div className="tp-chart">
          {/* The hover inspector's read-out row. Reserved so inspecting never
              shifts the layout (the window is sized to the content); the peak
              caption holds the row when idle and yields it to the inspected
              bucket while hovering — side by side they squeezed the read-out
              into an ellipsis. The Columns/Line toggle stays on the right. */}
          <div className="tp-chart-cap-row">
            {chartHover == null && <span className="tp-chart-peak">{model.chart.peak}</span>}
            <span className="tp-chart-read">
              {chartHover != null ? model.chart.details[chartHover] : ''}
            </span>
            <CostDrawingToggle value={drawing} onChange={pickCostDrawing} />
          </div>
          <svg
            className="tp-chart-plot"
            viewBox={`0 0 ${CHART_W} ${CHART_H}`}
            width={CHART_W}
            height={CHART_H}
            preserveAspectRatio="none"
            aria-hidden="true"
            onMouseMove={onChartMove}
            onMouseLeave={() => setChartHover(null)}
          >
            {drawing === 'columns' && (
              <>
                {chartHover != null && (
                  <path className="tp-chart-hover" d={slotRect(model.chart.points, chartHover)} />
                )}
                {/* The peak bucket wears the brighter fill — the same bucket the
                    model's peak caption names. */}
                <path className="tp-bars" d={barsPath(model.chart.points, model.chart.peakIndex)} />
                <path className="tp-bar-peak" d={barRect(model.chart.points, model.chart.peakIndex)} />
              </>
            )}
            {drawing === 'line' && (
              <>
                {model.chart.points.length > 1 && (
                  <>
                    <path
                      className="tp-line-area"
                      d={`${sparkPath(model.chart.points)} L${CHART_W - SPARK_PAD} ${CHART_H} L${SPARK_PAD} ${CHART_H} Z`}
                    />
                    <path
                      className="tp-line"
                      d={sparkPath(model.chart.points)}
                      vectorEffect="non-scaling-stroke"
                    />
                  </>
                )}
                {lineNowXY && (
                  <circle className="tp-line-now" cx={lineNowXY[0]} cy={lineNowXY[1]} r="2.5" />
                )}
                {lineHoverXY && (
                  <>
                    <line
                      className="tp-line-hover"
                      x1={lineHoverXY[0]}
                      x2={lineHoverXY[0]}
                      y1={2}
                      y2={CHART_H - 2}
                      vectorEffect="non-scaling-stroke"
                    />
                    <circle
                      className="tp-line-hover-dot"
                      cx={lineHoverXY[0]}
                      cy={lineHoverXY[1]}
                      r="2.5"
                    />
                  </>
                )}
              </>
            )}
          </svg>
          {/* The axis: first tick sits at the left edge, last at the right,
              middle between them — space-between puts each where its bucket is. */}
          <div className="tp-chart-cap">
            {model.chart.ticks.map((t) => (
              <span key={t}>{t}</span>
            ))}
          </div>
        </div>
      )}

      {!loading && model && !model.empty && model.models.length > 0 && (
        <div className="tp-models">
          <div className="tp-sec">Models</div>
          {model.models.map((r) => (
            <div className="tp-row" key={r.key}>
              {/* The owning Source's mark leads the row (the redesign traded
                  the colour dot for it). */}
              {r.icon && <img src={r.icon} alt="" width={12} height={12} />}
              <span className="tp-row-label tp-model" title={r.label}>
                {r.label}
              </span>
              <span className="tp-spacer" />
              <span className="tp-row-tokens">{r.tokens}</span>
              <span className="tp-row-cost">{r.cost}</span>
            </div>
          ))}
          {model.modelsOverflow > 0 && (
            <div className="tp-more">+{model.modelsOverflow} more in the app</div>
          )}
        </div>
      )}

      {/* One card per live Source (TOKL-19): collapsed to its Session + Weekly
          meters with reset countdowns, the whole header a disclosure to every
          window — per-model rows included. Stacked in Settings' card order
          (catalog order until someone moves one). The one-line form this
          replaced was shorter, and the ADR's third amendment records why the
          card came back. Not gated on model.empty: Limits are about now, and
          an idle day does not blank them. */}
      {!loading && limitCards.length > 0 && (
        <div className="tp-limsrcs">
          {limitCards.map((card) => {
            const open = !!limitsOpen[card.source];
            const plan = planLabel(card.plan);
            const parsed: ParsedWindow[] = card.windows.map((w) => ({
              w,
              label: windowLabel(w.key, card.source),
            }));
            return (
              <div className="tp-limcard" key={card.source}>
                <button
                  className="tp-limcard-head"
                  aria-expanded={open}
                  onClick={() =>
                    setLimitsOpen((o) => ({ ...o, [card.source]: !o[card.source] }))
                  }
                >
                  <img src={sourceIcon(card.meta.icon)} alt="" width={12} height={12} />
                  <span className="tp-limcard-name">{card.meta.label}</span>
                  {plan && <span className="tp-limcard-plan">{plan}</span>}
                  <span className="tp-spacer" />
                  <svg
                    className={open ? 'tp-limcard-chev open' : 'tp-limcard-chev'}
                    width="12"
                    height="12"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    strokeWidth="2"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    aria-hidden="true"
                  >
                    <path d="M9 6l6 6-6 6" />
                  </svg>
                </button>
                {open ? (
                  parsed.map(({ w, label }) => {
                    const l = windowText(EN, label);
                    return (
                      <div className="tp-limwin" key={w.key}>
                        <div className="tp-limwin-labels">
                          <span className="tp-limwin-k">
                            {l.text}
                            {l.sub && <span className="sub">{l.sub}</span>}
                          </span>
                          <span className={'tp-limwin-pct tp-t-' + w.tone}>{w.pctShown}%</span>
                          <span className="tp-limwin-resets">
                            {w.resetsInMin !== null &&
                              `resets in ${limitDuration(w.resetsInMin)}`}
                          </span>
                        </div>
                        <LimitBar w={w} mode={limitsMode} />
                      </div>
                    );
                  })
                ) : (
                  <div className="tp-limmeters">
                    {panelWindows(parsed, panelPicks[card.source]).map(({ w, label }) => (
                      <div className="tp-limmeter" key={w.key}>
                        <div className="tp-limmeter-row">
                          <span className="tp-limmeter-k">{windowText(EN, label).text}</span>
                          <span>
                            <span className={'tp-limmeter-v tp-t-' + w.tone}>{w.pctShown}%</span>
                            {w.resetsInMin !== null && (
                              <span className="tp-limmeter-t">· {limitDuration(w.resetsInMin)}</span>
                            )}
                          </span>
                        </div>
                        <LimitBar w={w} mode={limitsMode} />
                      </div>
                    ))}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}

      {!loading && !model?.empty && model?.stats && (
        <div className="tp-tiles">
          <div className="tp-tile">
            <div className="tp-tile-v">{model.stats.cacheHit}</div>
            <div className="tp-tile-k">cache hit</div>
          </div>
          <div className="tp-tile">
            {/* The same floor the tokens carry: an unreadable Session hides
                the Requests in it, and an Unbooked Request is one the Source
                made that no Usage Record could count (TOKL-25). */}
            <div className="tp-tile-v" title={tokensFloor.reason || undefined}>
              {markedTokenFigure(model.requestsText, tokensFloor)}
            </div>
            <div className="tp-tile-k">requests</div>
          </div>
        </div>
      )}

      <div className="tp-actions">
        <button className="tp-action" title="Open TokenLedger" onClick={() => ipc('show_main')}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M7 17L17 7" />
            <path d="M9 7h8v8" />
          </svg>
          <span className="tp-act-k">Open</span>
        </button>
        <button className="tp-action" title={`Settings (${keys.settings})`} onClick={() => ipc('open_settings')}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <line x1="4" y1="8" x2="20" y2="8" />
            <circle cx="10" cy="8" r="2.5" />
            <line x1="4" y1="16" x2="20" y2="16" />
            <circle cx="15" cy="16" r="2.5" />
          </svg>
          <span className="tp-act-k">Settings</span>
        </button>
        <button className="tp-action" title={`Quit TokenLedger (${keys.quit})`} onClick={() => ipc('quit_app')}>
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M12 2v10" />
            <path d="M18.4 6.6a9 9 0 1 1-12.77.04" />
          </svg>
          <span className="tp-act-k">Quit</span>
        </button>
      </div>
    </div>
  );
}
