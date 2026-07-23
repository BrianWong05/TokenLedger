import { useMemo, useRef, type RefObject } from 'react';
import type { Summary } from '../types';
import { modelColor, stackModels, type Bucket, type Granularity } from './data';
import { TOOLS } from './meta';
import { fmtTok } from '../lib/format';
import { formatDisplayCost, PER_UNIT_KEY, useOverviewT } from './localize';
import { useChartColors } from '../lib/chartColors';
import { useSettings } from '../settings/SettingsContext';
import { useDialogChrome } from './useDialogChrome';

// Design 1b — the Usage-trend card's full-screen enlarge (shell slice). A
// centered dialog over the dimmed dashboard: a header (title + window
// subtitle + close), the stacked-by-Source chart at full width, and a footer
// of the window's headline figures + legend. The window is the Overview's
// current one; its Cost comes from the same range Summary the page already
// fetched (later slices give the dialog its own window and inspector).

// Chart geometry in viewBox units (full-width, larger than the card).
const VW = 1000;
const VH = 360;
const PL = 52; // left gutter for the widest right-aligned y label
const PR = 16;
const PT = 16;
const BASE = 320;
const LABEL_Y = 344;

export default function TrendModal({
  data,
  per,
  rangeLabel,
  modelTool,
  summary,
  returnFocusRef,
  onClose,
}: {
  data: Bucket[];
  per: Granularity;
  rangeLabel: string;
  modelTool: Record<string, string>;
  summary: Summary | null; // the Overview's range Summary; null while it loads
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

  const maxTotal = Math.max(1, ...data.map((b) => b.total));
  const total = data.reduce((a, b) => a + b.total, 0);
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
