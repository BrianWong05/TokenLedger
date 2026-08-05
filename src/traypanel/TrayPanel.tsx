// The Menu Bar Extra panel (design 2b grown, in 2b's idiom — ADR-0007): a small
// frameless webview the tray toggles. Beneath the mock's header sit the Cost
// sparkline, the Source and Model rows, and the stats strip; every figure is a
// display string the view model already decided. Data goes through the same
// ports the app shell uses; the four actions are Tauri glue.
// UI strings are English-only like the native menu it replaced; number and
// currency formatting still follow the app's locale. Dark-only like the mock.
import { useCallback, useEffect, useRef, useState, type MouseEvent as ReactMouseEvent } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window';
import { detectPlatform, type Platform } from '../lib/platform';
import { panelModel, periodWindows, seriesBucket, type PanelModel, type Period } from './panelModel';
import { tauriLedger, type LedgerPort } from '../overview/ledger';
import { tauriSettings, type SettingsPort } from '../settings/settings';
import type { Filters } from '../types';
import './TrayPanel.css';

export interface TrayPanelPorts {
  ledger?: LedgerPort;
  settings?: SettingsPort;
}

const PANEL_WIDTH = 300;

// Sparkline geometry: the model hands over one normalised value per bucket,
// the view decides what that looks like in its 260×44 box. The horizontal
// inset keeps the stroke and the now-dot off the box edges, where half of
// either would be clipped.
const SPARK_W = 260;
const SPARK_H = 44;
const SPARK_PAD = 3;
function sparkXY(points: number[], i: number): [number, number] {
  const last = Math.max(1, points.length - 1);
  return [
    SPARK_PAD + (i / last) * (SPARK_W - SPARK_PAD * 2),
    SPARK_H - 4 - points[i] * (SPARK_H - 10),
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

// Fire-and-forget IPC for the action rows; harmless outside Tauri (tests).
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

// The panel opens wherever the tray delivers a click — macOS and Windows
// (ADR-0010) — and the two spell a modifier differently.
const KEY_HINTS = {
  macos: { rescan: '⇧⌘R', settings: '⌘,', quit: '⌘Q' },
  other: { rescan: 'Ctrl+Shift+R', settings: 'Ctrl+,', quit: 'Ctrl+Q' },
};

export default function TrayPanel({
  ports,
  platform = detectPlatform(),
}: { ports?: TrayPanelPorts; platform?: Platform } = {}) {
  const keys = platform === 'macos' ? KEY_HINTS.macos : KEY_HINTS.other;
  const ledger = ports?.ledger ?? tauriLedger;
  const settings = ports?.settings ?? tauriSettings;
  const [model, setModel] = useState<PanelModel | null>(null);
  const [scanning, setScanning] = useState(false);
  const [loading, setLoading] = useState(true);
  const [period, setPeriod] = useState<Period>('today');
  // The sparkline's hover inspector: which bucket the pointer is nearest, null
  // when the mouse is away.
  const [sparkHover, setSparkHover] = useState<number | null>(null);
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
          ledger.breakdown('project', current),
          ledger.series(current, seriesBucket(periodRef.current)),
          ledger.lastScan(),
        ]),
      ]),
      new Promise((r) => setTimeout(r, showLoading ? minLoadingMs() : 0)),
    ]);
    if (fetched[0].status === 'fulfilled') {
      const [t, y, rows, s, models, projects, series, scannedAt] = fetched[0].value;
      setModel(
        panelModel(t, y, rows, s, s.language === 'zh-Hant' ? 'zh-Hant' : 'en', {
          period: periodRef.current,
          now: new Date(),
          models,
          projects,
          series,
          scannedAt,
        }),
      );
      // A fresh model can hold fewer buckets than the inspected index (period
      // switch, midnight rollover mid-hover); never let the index outlive them.
      setSparkHover(null);
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

  // Initial load + refetch every time the tray shows the panel — both with
  // the loading beat; background refetches (rescan) skip it.
  useEffect(() => {
    void refresh(true);
    let un: (() => void) | undefined;
    Promise.resolve()
      .then(() => listen('panel-shown', () => void refresh(true)))
      .then((f) => { un = f; })
      .catch(() => {});
    return () => un?.();
  }, [refresh]);

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

  // Hit-testing: inverting the svg's x is enough — the svg stretches
  // (preserveAspectRatio none), so viewBox x is proportional to box x.
  const onSparkMove = (e: ReactMouseEvent<SVGSVGElement>) => {
    const spark = model?.spark;
    const rect = e.currentTarget.getBoundingClientRect();
    if (!spark || !rect.width) return;
    const vx = ((e.clientX - rect.left) / rect.width) * SPARK_W;
    const last = Math.max(1, spark.points.length - 1);
    const i = Math.round(((vx - SPARK_PAD) / (SPARK_W - SPARK_PAD * 2)) * last);
    setSparkHover(Math.max(0, Math.min(spark.points.length - 1, i)));
  };
  // The inspected bucket's chart position; null while the mouse is away.
  const hoverXY =
    model?.spark && sparkHover != null ? sparkXY(model.spark.points, sparkHover) : null;

  const pickPeriod = (p: Period) => {
    if (p === periodRef.current) return;
    periodRef.current = p;
    setPeriod(p);
    void refresh(); // no skeleton beat on a switch — it should feel snappy
  };

  const rescan = async () => {
    if (scanning) return; // coalesce double-clicks; scans serialize anyway
    setScanning(true);
    // Same minimum beat as the open-skeleton: a fast no-change scan must
    // still visibly happen (spinner + pulsing figures for ≥1s).
    const minDone = new Promise((r) => setTimeout(r, minLoadingMs()));
    try {
      await ledger.scan();
    } catch {
      /* scan errors surface in the Overview, not here */
    }
    await refresh();
    await minDone;
    setScanning(false);
  };

  return (
    <div className="tp" ref={bodyRef}>
      <div className="tp-header">
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
        {loading ? (
          <div className="tp-figures">
            <div className="tp-cost-row">
              <span className="tp-skel tp-skel-cost" />
            </div>
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
            <span className="tp-sub">
              {model ? `${model.fmtTokens(animTokens)} tok · ${model.requestsText} req` : ''}
            </span>
          </div>
        )}
      </div>

      {loading && (
        <>
          <div className="tp-spark">
            <span className="tp-skel tp-skel-spark" />
          </div>
          {[
            ['src', 2],
            ['model', 3],
          ].map(([kind, n]) => (
            <div key={kind}>
              <div className="tp-sep" />
              {Array.from({ length: n as number }, (_, i) => (
                <div className="tp-row" key={i}>
                  <span className="tp-skel tp-skel-dot" />
                  <span className="tp-skel tp-skel-name" />
                  <span className="tp-spacer" />
                  <span className="tp-skel tp-skel-num" />
                </div>
              ))}
            </div>
          ))}
          <div className="tp-sep" />
          {[0, 1, 2].map((i) => (
            <div className="tp-stat" key={i}>
              <span className="tp-skel tp-skel-name" />
              <span className="tp-skel tp-skel-num" />
            </div>
          ))}
        </>
      )}

      {!loading && !model?.empty && model?.spark && (
        <div className="tp-spark">
          {/* The hover inspector's read-out. The line is always reserved so
              inspecting never shifts the layout (the window is sized to the
              content); idle it hints, hovered it reads the bucket. */}
          <div className={sparkHover != null ? 'tp-spark-read' : 'tp-spark-read idle'}>
            {sparkHover != null ? model.spark.details[sparkHover] : 'hover the chart'}
          </div>
          <svg
            viewBox={`0 0 ${SPARK_W} ${SPARK_H}`}
            width={SPARK_W}
            height={SPARK_H}
            preserveAspectRatio="none"
            aria-hidden="true"
            onMouseMove={onSparkMove}
            onMouseLeave={() => setSparkHover(null)}
          >
            {/* One bucket is a point, not a line: the area would close into a
                triangle spanning the whole box, drawing a fall that never
                happened. The now-dot marks the latest bucket either way. */}
            {model.spark.points.length > 1 && (
              <>
                <path
                  className="tp-spark-area"
                  d={`${sparkPath(model.spark.points)} L${SPARK_W - SPARK_PAD} ${SPARK_H} L${SPARK_PAD} ${SPARK_H} Z`}
                />
                <path className="tp-spark-line" d={sparkPath(model.spark.points)} />
              </>
            )}
            <circle
              className="tp-spark-now"
              cx={sparkXY(model.spark.points, model.spark.points.length - 1)[0]}
              cy={sparkXY(model.spark.points, model.spark.points.length - 1)[1]}
              r="2.5"
            />
            {hoverXY && (
              <>
                <line
                  className="tp-spark-hover-line"
                  x1={hoverXY[0]}
                  x2={hoverXY[0]}
                  y1={2}
                  y2={SPARK_H - 2}
                />
                <circle className="tp-spark-hover-dot" cx={hoverXY[0]} cy={hoverXY[1]} r="2.5" />
              </>
            )}
          </svg>
          {/* The axis: first tick sits at the left edge, last at the right,
              middle between them — space-between puts each where its bucket is. */}
          <div className="tp-spark-cap">
            {model.spark.ticks.map((t) => (
              <span key={t}>{t}</span>
            ))}
          </div>
          <div className="tp-spark-cap tp-spark-peak">{model.spark.peak}</div>
        </div>
      )}

      {!loading && model && model.rows.length > 0 && (
        <div className="tp-sources">
          <div className="tp-sep" />
          {model.rows.map((r) => (
            <div className="tp-row" key={r.key}>
              {r.icon ? <img src={r.icon} alt="" width={13} height={13} /> : <span className="tp-icon-gap" />}
              <span className="tp-row-label">{r.label}</span>
              <span className="tp-spacer" />
              <span className="tp-row-tokens">{r.tokens}</span>
              <span className="tp-row-cost">{r.cost}</span>
            </div>
          ))}
        </div>
      )}

      {!loading && model && !model.empty && model.models.length > 0 && (
        <div className="tp-models">
          <div className="tp-sep" />
          <div className="tp-sec">Models</div>
          {model.models.map((r) => (
            <div className="tp-row" key={r.key}>
              {/* No icon per Model — the dot carries its owning Source instead. */}
              <span className="tp-dot" style={{ background: r.color ?? '#6e6e76' }} />
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
        <div className="tp-stats">
          <div className="tp-sep" />
          <div className="tp-stat">
            <span className="tp-stat-k">Cache hit rate</span>
            <span className="tp-stat-v">{model.stats.cacheHit}</span>
          </div>
          {model.stats.topProject && (
            <div className="tp-stat">
              <span className="tp-stat-k">Top project</span>
              <span className="tp-stat-v">{model.stats.topProject}</span>
            </div>
          )}
          <div className="tp-stat">
            <span className="tp-stat-k">Scanned</span>
            <span className="tp-stat-v">{model.stats.scanned}</span>
          </div>
        </div>
      )}

      <div className="tp-sep" />
      <button className="tp-action" onClick={() => ipc('show_main')}>
        Open TokenLedger
      </button>
      <button className="tp-action" onClick={() => void rescan()} disabled={scanning}>
        Rescan now
        {scanning ? (
          // 1b's refresh glyph, spinning while the scan runs.
          <svg
            className="tp-spin"
            width="12"
            height="12"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-label="scanning"
          >
            <path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8" />
            <path d="M21 3v5h-5" />
            <path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16" />
            <path d="M8 16H3v5" />
          </svg>
        ) : (
          <span className="tp-key">{keys.rescan}</span>
        )}
      </button>
      <div className="tp-sep" />
      <button className="tp-action" onClick={() => ipc('open_settings')}>
        Settings…<span className="tp-key">{keys.settings}</span>
      </button>
      <button className="tp-action" onClick={() => ipc('quit_app')}>
        Quit TokenLedger<span className="tp-key">{keys.quit}</span>
      </button>
    </div>
  );
}
