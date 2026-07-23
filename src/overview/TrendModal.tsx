import { useEffect, useMemo, useRef, useState, type RefObject } from 'react';
import type { SeriesPoint, Summary } from '../types';
import { modelColor, rangeToFilters, stackModels, trendSlice, type Bucket } from './data';
import { TOOLS, RANGES_8B, type Range8b } from './meta';
import type { LedgerPort } from './ledger';
import { fmtTok } from '../lib/format';
import {
  fmtIsoDateL,
  formatDisplayCost,
  PER_UNIT_KEY,
  RANGE_LABEL_KEY,
  RANGE_LONG_KEY,
  useOverviewT,
} from './localize';
import { useChartColors } from '../lib/chartColors';
import { useSettings } from '../settings/SettingsContext';
import { useDialogChrome } from './useDialogChrome';

// Design 1b — the Usage-trend card's full-screen enlarge. A centered dialog
// over the dimmed dashboard with a window of its own: the Day/Week/Month/Total/
// Custom selector, initialized from the Overview's window and discarded on
// close (the dashboard behind never moves). The stacked-by-tool chart, footer
// figures and Est. cost all describe the dialog's local window — buckets from
// the shared trendSlice (its own hourly fetch for a Day window), Cost from a
// per-window Summary fetch the dialog owns (epoch-guarded, like the Activity
// enlarge). The bucket inspector + per-bucket cost + CSV land in later slices.

// Chart geometry in viewBox units (full-width, larger than the card).
const VW = 1000;
const VH = 360;
const PL = 52; // left gutter for the widest right-aligned y label
const PR = 16;
const PT = 16;
const BASE = 320;
const LABEL_Y = 344;

export default function TrendModal({
  allPoints,
  firstIso,
  lastIso,
  initialRange,
  initialCustomFrom,
  initialCustomTo,
  ledger,
  returnFocusRef,
  onClose,
}: {
  allPoints: SeriesPoint[]; // the full unbounded daily series
  firstIso: string;
  lastIso: string;
  initialRange: Range8b;
  initialCustomFrom: string;
  initialCustomTo: string;
  ledger: LedgerPort;
  returnFocusRef: RefObject<HTMLElement | null>;
  onClose: () => void;
}) {
  const { t, lang } = useOverviewT();
  const { settings } = useSettings();
  const colors = useChartColors();

  const modalRef = useRef<HTMLElement>(null);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  useDialogChrome({ modalRef, initialFocusRef: closeButtonRef, returnFocusRef, onClose });
  // A drag/click that begins inside the panel but releases on the dimmed
  // margin makes the backdrop the click target; only close when the press
  // also began on the backdrop (mirrors the Activity enlarge).
  const backdropArmed = useRef(false);

  // Window state local to the dialog: seeded from the Overview's on open,
  // forgotten on close (this component only mounts while open).
  const [range, setRange] = useState<Range8b>(initialRange);
  const [customFrom, setCustomFrom] = useState(initialCustomFrom);
  const [customTo, setCustomTo] = useState(initialCustomTo);
  // Effective bounds — the raw custom inputs fall back to the data extent,
  // exactly as the store derives from/to (never the empty string).
  const from = customFrom || firstIso;
  const to = customTo || lastIso;

  // Per-window fetches the dialog owns: a Summary for the footer Cost, and the
  // hourly series a Day window needs (the page only holds it while itself on
  // Day). Epoch-guarded so a stale response from a superseded window can't land.
  const [summary, setSummary] = useState<Summary | null>(null);
  const [hourPoints, setHourPoints] = useState<SeriesPoint[]>([]);
  const fetchEpoch = useRef(0);
  useEffect(() => {
    const epoch = ++fetchEpoch.current;
    setSummary(null);
    const filters = rangeToFilters(range, from, to);
    ledger.summary(filters).then(
      (s) => {
        if (fetchEpoch.current === epoch) setSummary(s);
      },
      () => {},
    );
    if (range === 'day') {
      ledger.series(filters, 'hour').then(
        (pts) => {
          if (fetchEpoch.current === epoch) setHourPoints(pts);
        },
        () => {},
      );
    }
  }, [range, from, to, ledger]);

  const { trend: data, per, modelTool, total } = useMemo(
    () => trendSlice(allPoints, hourPoints, range, from, to, firstIso, lastIso, new Date(), lang),
    [allPoints, hourPoints, range, from, to, firstIso, lastIso, lang],
  );

  const rangeLabel =
    range === 'custom'
      ? `${fmtIsoDateL(from, lang)} – ${fmtIsoDateL(to, lang)}`
      : t(RANGE_LONG_KEY[range]);

  const maxTotal = Math.max(1, ...data.map((b) => b.total));
  const avg = total / (data.length || 1);
  const plotW = VW - PL - PR;
  const slot = plotW / (data.length || 1);
  const barW = Math.min(48, slot * 0.7);
  const h = (v: number) => (v / maxTotal) * (BASE - PT);
  const grid = [0, 1, 2, 3, 4].map((i) => ({ y: BASE - (i / 4) * (BASE - PT), label: fmtTok((maxTotal * i) / 4) }));
  // One label per ~14 bars, so 24 hourly / 30 daily buckets stay legible.
  const labelStep = Math.max(1, Math.ceil(data.length / 14));

  // Segments are per model but colored by the model's owning tool; grouping the
  // stack by tool keeps each bar reading as contiguous tool blocks (shared with
  // the card via stackModels/modelColor).
  const models = useMemo(() => stackModels(data, modelTool), [data, modelTool]);
  const segsOf = (b: Bucket) => models.map((m) => ({ key: m, color: modelColor(modelTool, m), val: b.byModel[m] ?? 0 }));

  const costLabel =
    summary === null ? '…' : formatDisplayCost(summary.cost, summary.hasUnpriced, settings, lang);

  return (
    <div
      className="tt-trend-modal-backdrop"
      onMouseDown={(event) => {
        backdropArmed.current = event.target === event.currentTarget;
      }}
      onClick={(event) => {
        if (event.target === event.currentTarget && backdropArmed.current) onClose();
        backdropArmed.current = false;
      }}
    >
      <section
        ref={modalRef}
        className="tt-trend-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="tt-trend-modal-title"
        tabIndex={-1}
      >
        <header className="tt-trend-modal-head">
          <div>
            <div className="tt-trend-modal-title" id="tt-trend-modal-title">
              {t('overview.usageTrend')}
            </div>
            <div className="tt-trend-modal-sub">
              {t('overview.stackedByTool')} · {rangeLabel}
            </div>
          </div>
          <span className="tt-trend-modal-spacer" />
          <div className="tt-seg">
            {RANGES_8B.map((r) => (
              <button key={r.key} className={range === r.key ? 'active' : ''} onClick={() => setRange(r.key)}>
                {t(RANGE_LABEL_KEY[r.key])}
              </button>
            ))}
          </div>
          <button
            ref={closeButtonRef}
            type="button"
            className="tt-trend-modal-close"
            onClick={onClose}
            aria-label={t('overview.close')}
          >
            <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M18 6 6 18" />
              <path d="m6 6 12 12" />
            </svg>
          </button>
        </header>

        {range === 'custom' && (
          <div className="tt-custom-row tt-trend-modal-custom">
            <span className="lbl">{t('overview.customRange')}</span>
            <input
              type="date"
              value={from}
              min={firstIso}
              max={to}
              onChange={(e) => e.target.value && setCustomFrom(e.target.value)}
            />
            <span className="to">{t('overview.to')}</span>
            <input
              type="date"
              value={to}
              min={from}
              max={lastIso}
              onChange={(e) => e.target.value && setCustomTo(e.target.value)}
            />
          </div>
        )}

        <div className="tt-trend-modal-chart">
          <svg viewBox={`0 0 ${VW} ${VH}`} preserveAspectRatio="xMidYMid meet" style={{ width: '100%', height: '100%', display: 'block' }}>
            {grid.map((g, i) => (
              <g key={i}>
                <line x1={PL} y1={g.y} x2={VW - PR} y2={g.y} stroke={colors.grid} strokeWidth={1} />
                <text x={PL - 6} y={g.y} dy={3.4} fontSize={11} textAnchor="end" style={{ fill: 'var(--text-tertiary)' }}>
                  {g.label}
                </text>
              </g>
            ))}
            {data.map((b, i) => {
              const x = PL + i * slot + (slot - barW) / 2;
              let y = BASE;
              return (
                <g key={i}>
                  {segsOf(b).map((s) => {
                    const seg = h(s.val);
                    y -= seg;
                    return <rect key={s.key} x={x} y={y} width={barW} height={Math.max(0, seg)} fill={s.color} />;
                  })}
                </g>
              );
            })}
            {data.map((b, i) => (
              <text key={'x' + i} x={PL + i * slot + slot / 2} y={LABEL_Y} fontSize={11} textAnchor="middle" style={{ fill: 'var(--text-tertiary)' }}>
                {i % labelStep ? '' : b.label}
              </text>
            ))}
          </svg>
        </div>

        <div className="tt-trend-modal-foot">
          <div className="tt-trend-modal-stats">
            <div className="stat">
              <b>{fmtTok(total)}</b>
              <span>{t('overview.total')} · {rangeLabel}</span>
            </div>
            <div className="stat">
              <b>{fmtTok(avg)}</b>
              <span>{t('overview.avg')} / {t(PER_UNIT_KEY[per])}</span>
            </div>
            <div className="stat">
              <b className="cost">{costLabel}</b>
              <span>{t('overview.estCost')}</span>
              {summary?.hasUnpriced && (
                <span className="mark" title={summary.unpricedModels.join(', ')}>
                  {summary.unpricedModels.length} {t('overview.unpricedMarker')}
                </span>
              )}
            </div>
          </div>
          <div className="tt-legend">
            {TOOLS.filter((tl) => data.some((b) => b.byTool[tl.key] > 0)).map((tl) => (
              <span className="item" key={tl.key}>
                <span className="sw" style={{ background: tl.color }} />
                {tl.label}
              </span>
            ))}
          </div>
        </div>
      </section>
    </div>
  );
}
