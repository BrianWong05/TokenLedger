import { useEffect, useMemo, useRef, useState, type RefObject } from 'react';
import type { SeriesPoint, Summary } from '../types';
import { bucketCsv, bucketFilters, csvFilename, modelColor, rangeToFilters, stackModels, trendSlice, type Bucket } from './data';
import { TOOLS, RANGES_8B, type Range8b } from './meta';
import type { LedgerPort } from './ledger';
import type { ExportPort } from './export';
import { fmtPct, fmtTok } from '../lib/format';
import {
  fmtIsoDateL,
  formatDisplayCost,
  monthShortL,
  PER_UNIT_KEY,
  RANGE_LABEL_KEY,
  RANGE_LONG_KEY,
  SEL_HEADING_KEY,
  useOverviewT,
} from './localize';
import { useChartColors, CHART_LIGHT } from '../lib/chartColors';
import { useSettings } from '../settings/SettingsContext';
import { useDialogChrome } from './useDialogChrome';

// Design 1b — the Usage-trend card's full-screen enlarge. A centered dialog
// over the dimmed dashboard with a window of its own: the Day/Week/Month/Total/
// Custom selector, initialized from the Overview's window and discarded on
// close (the dashboard behind never moves). The stacked-by-tool chart, footer
// figures and Est. cost all describe the dialog's local window — buckets from
// the shared trendSlice (its own hourly fetch for a Day window), Cost from a
// per-window Summary fetch the dialog owns (epoch-guarded, like the Activity
// enlarge). Exactly one bucket is always selected — the window's peak until a
// bar is hovered — and the right-hand inspector reads it out (rank, delta vs
// the window average, per-model split). The inspector's per-bucket Cost and CSV
// export land in later slices.

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
  exporter,
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
  exporter: ExportPort;
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

  // Exactly one bucket is always selected — moved by hovering a bar. The
  // selection is keyed by bucket key AND its window: a window change (new winId)
  // drops the old key back to the peak, while a background refresh (same window)
  // keeps the key if it survives.
  const winId = `${range}|${from}|${to}`;
  const [sel, setSel] = useState<{ win: string; key: string } | null>(null);
  const peak = data.reduce<Bucket | undefined>((a, b) => (a && a.total >= b.total ? a : b), undefined);
  const activeKey = sel && sel.win === winId ? sel.key : null;
  const selBucket = data.find((b) => b.key === activeKey) ?? peak;
  const selIndex = selBucket ? data.indexOf(selBucket) : -1;

  // The selected bucket's exact Cost: a Summary scoped to just that bucket's
  // time bounds, refetched per selection (or granularity) and epoch-guarded so
  // a superseded bucket's Cost can't land after a newer pick. Its own rules —
  // ≥ Partial Cost, unpriced never $0, Display Currency — all via
  // formatDisplayCost. Keyed by bucket key + granularity, so a background
  // refresh that keeps the selection does not refetch.
  const selKey = selBucket?.key;
  const [selCost, setSelCost] = useState<Summary | null>(null);
  const costEpoch = useRef(0);
  useEffect(() => {
    if (!selKey) return;
    const epoch = ++costEpoch.current;
    setSelCost(null);
    ledger.summary(bucketFilters(selKey, per)).then(
      (s) => {
        if (costEpoch.current === epoch) setSelCost(s);
      },
      () => {},
    );
  }, [selKey, per, ledger]);

  // Inspector read-outs for the selected bucket.
  const selRank = selBucket ? 1 + data.filter((b) => b.total > selBucket.total).length : 0;
  const selDeltaPct = selBucket && avg > 0 ? Math.round((selBucket.total / avg - 1) * 100) : 0;
  const selDate = (b: Bucket) =>
    per === 'hour'
      ? b.key.slice(11, 16)
      : per === 'month'
        ? `${monthShortL(parseInt(b.key.slice(5, 7), 10) - 1, lang)} ${b.key.slice(0, 4)}`
        : fmtIsoDateL(b.key, lang);
  // Top-6 models in the selected bucket, largest first; the rest fold into one
  // muted remainder row (color '' → grey).
  const selRows: { key: string; name: string; val: number; color: string; more: boolean }[] = [];
  if (selBucket) {
    const ranked = Object.entries(selBucket.byModel)
      .filter(([, v]) => v > 0)
      .sort((a, b) => b[1] - a[1]);
    for (const [m, v] of ranked.slice(0, 6)) selRows.push({ key: m, name: m, val: v, color: modelColor(modelTool, m), more: false });
    const rest = ranked.slice(6);
    if (rest.length) {
      selRows.push({
        key: '__more__',
        name: `${rest.length} ${t('overview.trend.moreModels')}`,
        val: rest.reduce((a, [, v]) => a + v, 0),
        color: '',
        more: true,
      });
    }
  }
  const selTotal = selBucket?.total ?? 0;
  // WebKit can't resolve var() in an SVG stroke; pick the outline per theme.
  const outline = colors === CHART_LIGHT ? '#12151b' : '#e8ecf4';

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

        <div className="tt-trend-modal-body">
          <div className="tt-trend-modal-main">
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
                    <g key={i} opacity={selIndex === i ? 1 : 0.42} style={{ transition: 'opacity .15s' }}>
                      {segsOf(b).map((s) => {
                        const seg = h(s.val);
                        y -= seg;
                        return <rect key={s.key} x={x} y={y} width={barW} height={Math.max(0, seg)} fill={s.color} />;
                      })}
                    </g>
                  );
                })}
                {selIndex >= 0 && (
                  <rect
                    x={PL + selIndex * slot + (slot - barW) / 2 - 2.5}
                    y={BASE - h(selTotal) - 2.5}
                    width={barW + 5}
                    height={h(selTotal) + 5}
                    rx={3}
                    fill="none"
                    stroke={outline}
                    strokeWidth={1.5}
                    style={{ pointerEvents: 'none' }}
                  />
                )}
                {data.map((b, i) => (
                  <text key={'x' + i} x={PL + i * slot + slot / 2} y={LABEL_Y} fontSize={11} textAnchor="middle" style={{ fill: 'var(--text-tertiary)' }}>
                    {i % labelStep ? '' : b.label}
                  </text>
                ))}
                {data.map((b, i) => (
                  <rect key={'hit' + i} x={PL + i * slot} y={PT} width={slot} height={BASE - PT} fill="transparent" onMouseOver={() => setSel({ win: winId, key: b.key })} />
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
          </div>

          <aside className="tt-trend-insp">
            {selBucket && (
              <>
                <div className="tt-trend-insp-head">
                  <div>
                    <div className="eyebrow">{t(SEL_HEADING_KEY[per])}</div>
                    <div className="date">{selDate(selBucket)}</div>
                  </div>
                  <span className="rank">#{selRank} / {data.length}</span>
                </div>
                <div className="tt-trend-insp-tok">
                  <span className="val">{fmtTok(selTotal)}</span>
                  <span className="unit">{t('overview.tokens')}</span>
                </div>
                <div className={'tt-trend-insp-delta ' + (selDeltaPct >= 0 ? 'up' : 'down')}>
                  {selDeltaPct >= 0 ? '+' : '−'}{Math.abs(selDeltaPct)}% {t('overview.trend.vsAvg')}
                </div>
                <div className="tt-trend-insp-div" />
                <div className="eyebrow">{t('overview.trend.byModel')}</div>
                <div className="tt-trend-insp-rows">
                  {selRows.map((r) => (
                    <div className={'tt-trend-insp-row' + (r.more ? ' more' : '')} key={r.key}>
                      <div className="lab">
                        <span className="dot" style={r.more ? undefined : { background: r.color }} />
                        <span className="name">{r.name}</span>
                        <span className="num">
                          {fmtTok(r.val)} <span className="pct">{fmtPct(r.val / (selTotal || 1))}</span>
                        </span>
                      </div>
                      <div className="track">
                        <div className="fill" style={{ width: (r.val / (selTotal || 1)) * 100 + '%', background: r.more ? undefined : r.color }} />
                      </div>
                    </div>
                  ))}
                  {selRows.length === 0 && <div className="tt-trend-insp-empty">{t('overview.noActivity')}</div>}
                </div>
                <div className="tt-trend-insp-cost">
                  <span>{t('overview.estCost')}</span>
                  <span className="amt">
                    {selCost === null ? '…' : formatDisplayCost(selCost.cost, selCost.hasUnpriced, settings, lang)}
                    {selCost?.hasUnpriced && (
                      <span className="mark" title={selCost.unpricedModels.join(', ')}>
                        {' '}· {selCost.unpricedModels.length} {t('overview.unpricedMarker')}
                      </span>
                    )}
                  </span>
                </div>
                <button
                  type="button"
                  className="tt-trend-insp-export"
                  disabled={selTotal === 0}
                  onClick={() => {
                    // ponytail: fire-and-forget. Cancel is a no-op; a rare
                    // post-confirm write error is swallowed (no toast infra yet).
                    // Add user feedback if writes ever fail in practice.
                    void exporter.saveCsv(csvFilename(selBucket.key), bucketCsv(selBucket, modelTool)).catch(() => {});
                  }}
                >
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                    <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
                    <path d="M7 10l5 5 5-5" />
                    <path d="M12 15V3" />
                  </svg>
                  {t('overview.trend.exportCsv')}
                </button>
              </>
            )}
          </aside>
        </div>
      </section>
    </div>
  );
}
