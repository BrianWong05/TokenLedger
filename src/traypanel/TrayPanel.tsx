// The Menu Bar Extra panel (ADR-0007), in the "Glance + Models · bars" design:
// period tabs top-left double as the window's label, Rescan is the top-right
// refresh icon, and beneath the hero Cost sit the stacked source bar with its
// legend, the Cost bar chart, the Model rows (led by their Source's mark), the
// stat tiles, and the three icon actions. Every figure is a display string the
// view model already decided. Data goes through the same ports the app shell
// uses; the four actions are Tauri glue.
// UI strings are English-only like the native menu it replaced; number and
// currency formatting still follow the app's locale. Dark-only like the mock.
import { useCallback, useEffect, useRef, useState, type MouseEvent as ReactMouseEvent } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window';
import { detectPlatform, type Platform } from '../lib/platform';
import { hotkeyHint, isHotkey } from '../lib/hotkeys';
import { panelModel, periodWindows, seriesBucket, type PanelModel, type Period } from './panelModel';
import { tauriLedger, type LedgerPort } from '../overview/ledger';
import { tauriSettings, type SettingsPort } from '../settings/settings';
import { unreadableSourcesIn } from '../lib/tokenCompleteness';
import { sourceMeta } from '../overview/meta';
import { countLabel } from '../overview/localize';
import type { Filters } from '../types';
import './TrayPanel.css';

export interface TrayPanelPorts {
  ledger?: LedgerPort;
  settings?: SettingsPort;
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

// Fire-and-forget IPC for the actions; harmless outside Tauri (tests).
function ipc(cmd: string) {
  Promise.resolve()
    .then(() => invoke(cmd))
    .catch(() => {});
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
  const [model, setModel] = useState<PanelModel | null>(null);
  // The selected window's token figure is a floor (≥) when an Unreadable
  // Artifact could hold usage in it (ADR-0017) — same rules as everywhere.
  // The reason rides along as the marker's hover text.
  const [tokensFloor, setTokensFloor] = useState<{ marked: boolean; reason: string }>({
    marked: false,
    reason: '',
  });
  const [scanning, setScanning] = useState(false);
  // The gate rescan() reads, mirroring useAutoRefresh's busyRef: `scanning`
  // is for the view, and a memoized rescan() would close over a stale copy of
  // it forever — while listing it as a dep would re-fire the open effect.
  const scanningRef = useRef(false);
  const [loading, setLoading] = useState(true);
  const [period, setPeriod] = useState<Period>('today');
  // The bar chart's hover inspector: which bucket the pointer is over, null
  // when the mouse is away.
  const [chartHover, setChartHover] = useState<number | null>(null);
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

  const refresh = useCallback(async (showLoading = false) => {
    if (showLoading) setLoading(true);
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
      const unread = unreadableSourcesIn(sources, w.start);
      setTokensFloor({
        marked: unread.length > 0,
        reason: unread
          .map((u) => `${sourceMeta(u.source).label}: ${countLabel(u.artifactsUnreadable, 'overview.unreadableSessionOne', 'overview.unreadableSessionMany', lang)}`)
          .join(' · '),
      });
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
  }, [ledger, settings]);

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
  }, [ledger, refresh]);

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

  const barSlices = model?.rows.filter((r) => (r.share ?? 0) > 0) ?? [];

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
          <span className="tp-sub" title={tokensFloor.marked ? tokensFloor.reason : undefined}>
            {/* Tokens only — the requests figure lives in its stat tile, and
                saying it twice two lines apart bought nothing. */}
            {model ? `${tokensFloor.marked ? '≥ ' : ''}${model.fmtTokens(animTokens)} tokens` : ''}
          </span>
        </div>
      )}

      {loading && (
        <>
          <span className="tp-skel tp-skel-bar" />
          <span className="tp-skel tp-skel-chart" />
          <div className="tp-tiles">
            {[0, 1, 2].map((i) => (
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
            {model.rows.map((r) => (
              <div className="tp-leg" key={r.key}>
                <span className="tp-leg-name">
                  {r.icon && <img src={r.icon} alt="" width={12} height={12} />}
                  {r.label}
                </span>
                <span className="tp-leg-cost">{r.cost}</span>
              </div>
            ))}
          </div>
        </div>
      )}

      {!loading && !model?.empty && model?.chart && (
        <div className="tp-chart">
          {/* The hover inspector's read-out row. Reserved so inspecting never
              shifts the layout (the window is sized to the content); the peak
              caption holds the left, the inspected bucket reads on the right. */}
          <div className="tp-chart-cap-row">
            <span className="tp-chart-peak">{model.chart.peak}</span>
            <span className="tp-chart-read">
              {chartHover != null ? model.chart.details[chartHover] : ''}
            </span>
          </div>
          <svg
            viewBox={`0 0 ${CHART_W} ${CHART_H}`}
            width={CHART_W}
            height={CHART_H}
            preserveAspectRatio="none"
            aria-hidden="true"
            onMouseMove={onChartMove}
            onMouseLeave={() => setChartHover(null)}
          >
            {chartHover != null && (
              <path className="tp-chart-hover" d={slotRect(model.chart.points, chartHover)} />
            )}
            {/* The peak bucket wears the brighter fill — the same bucket the
                model's peak caption names. */}
            <path className="tp-bars" d={barsPath(model.chart.points, model.chart.peakIndex)} />
            <path className="tp-bar-peak" d={barRect(model.chart.points, model.chart.peakIndex)} />
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

      {!loading && !model?.empty && model?.stats && (
        <div className="tp-tiles">
          <div className="tp-tile">
            <div className="tp-tile-v">{model.stats.cacheHit}</div>
            <div className="tp-tile-k">cache hit</div>
          </div>
          <div className="tp-tile">
            <div className="tp-tile-v">{model.requestsText}</div>
            <div className="tp-tile-k">requests</div>
          </div>
          <div className="tp-tile">
            <div className="tp-tile-v">{model.stats.scanned}</div>
            <div className="tp-tile-k">scanned</div>
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
