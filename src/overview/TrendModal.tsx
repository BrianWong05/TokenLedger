import { useEffect, useMemo, useRef, useState, type RefObject } from 'react';
import type { SeriesPoint, SourceUnreadable, Summary } from '../types';
import { bucketCsv, bucketFilters, csvFilename, hourlyDayOf, modelOwner, rangeToFilters, rankedModels, stackModels, trendSlice, UNATTRIBUTED_COLOR, type Bucket, type Granularity } from './data';
import { orderedSourceKeys, sourceMeta, type Range8b } from './meta';
import type { LedgerPort } from './ledger';
import type { ExportPort } from './export';
import { fmtPct, fmtTok } from '../lib/format';
import {
  fmtIsoDateL,
  formatSummaryCost,
  INTERVAL_LABEL_KEY,
  markedTokenFigure,
  monthShortL,
  NO_FLOOR,
  PER_UNIT_KEY,
  RANGE_LONG_KEY,
  SEL_HEADING_KEY,
  tokenFloor,
  useOverviewT,
} from './localize';
import { useChartColors, CHART_LIGHT } from '../lib/chartColors';
import { useSettings } from '../settings/SettingsContext';
import { useDialogChrome } from './useDialogChrome';
import { RangeSegments } from './RangePicker';

// Design 1b — the Usage-trend card's full-screen enlarge. A centered dialog
// over the dimmed dashboard with a window of its own: the Day/Week/Month/Total/
// Custom selector, initialized from the Overview's window and discarded on
// close (the dashboard behind never moves), plus a bar-interval selector
// (Auto · Daily · Weekly · Monthly) pinning the bucket size in place of the
// automatic fit. The stacked-by-tool chart, footer
// figures and Est. cost all describe the dialog's local window — buckets from
// the shared trendSlice (its own hourly fetch for a Day window), Cost from a
// per-window Summary fetch the dialog owns (epoch-guarded, like the Activity
// enlarge). The window's own per-Model split sits under the footer stats it
// belongs with. Exactly one bucket is always selected — the window's peak until
// a bar is hovered — and the right-hand inspector reads it out (rank, delta vs
// the window average, per-model split). The inspector's per-bucket Cost and CSV
// export land in later slices.

// Bar-interval options: Auto (the automatic fit) or an explicit bucket size.
// No Hour override — a Day window's automatic hourly view already covers it.
const INTERVALS = ['auto', 'day', 'week', 'month'] as const;
type BarInterval = (typeof INTERVALS)[number];

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
  unreadable = [],
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
  // Sources holding Unreadable Artifacts (ADR-0017), unfiltered — the dialog
  // applies the floor rule to its own window and to the inspected bucket.
  unreadable?: SourceUnreadable[];
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
  // Bar interval, also dialog-local: opens on Auto, survives window changes
  // within one open (an independent axis: the window says which slice, the
  // interval says how finely it's bucketed), forgotten on close.
  const [barInterval, setBarInterval] = useState<BarInterval>('auto');
  // Effective bounds — the raw custom inputs fall back to the data extent,
  // exactly as the store derives from/to (never the empty string).
  const from = customFrom || firstIso;
  const to = customTo || lastIso;

  // Per-window fetches the dialog owns: a Summary for the footer Cost, and the
  // hourly series a single-day window needs (the page only holds it while it is
  // itself on one). Epoch-guarded so a stale response can't land on a new window.
  const [summary, setSummary] = useState<Summary | null>(null);
  const [hourPoints, setHourPoints] = useState<SeriesPoint[]>([]);
  const fetchEpoch = useRef(0);
  useEffect(() => {
    const epoch = ++fetchEpoch.current;
    setSummary(null);
    setHourPoints([]); // the old window's hours key off its own day — never this one's
    const filters = rangeToFilters(range, from, to);
    ledger.summary(filters).then(
      (s) => {
        if (fetchEpoch.current === epoch) setSummary(s);
      },
      () => {},
    );
    if (hourlyDayOf(range, from, to, firstIso, lastIso)) {
      ledger.series(filters, 'hour').then(
        (pts) => {
          if (fetchEpoch.current === epoch) setHourPoints(pts);
        },
        () => {},
      );
    }
  }, [range, from, to, firstIso, lastIso, ledger]);

  const override: Exclude<Granularity, 'hour'> | undefined = barInterval === 'auto' ? undefined : barInterval;
  const { trend: data, per, modelTool, total } = useMemo(
    () => trendSlice(allPoints, hourPoints, range, from, to, firstIso, lastIso, new Date(), lang, override),
    [allPoints, hourPoints, range, from, to, firstIso, lastIso, lang, override],
  );
  const activeTools = useMemo(
    () => orderedSourceKeys(data.flatMap((bucket) => Object.keys(bucket.byTool).filter((key) => bucket.byTool[key] > 0))).map(sourceMeta),
    [data],
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

  // Segments are per model but colored by the Source that produced them in that
  // bucket; grouping the stack by tool keeps each bar reading as contiguous tool
  // blocks (shared with the card via stackModels/modelOwner).
  const models = useMemo(() => stackModels(data, modelTool), [data, modelTool]);
  const segsOf = (b: Bucket) => [
    ...models.map((m) => ({ key: m, color: modelOwner(b.modelSources, modelTool, m).color, val: b.byModel[m] ?? 0 })),
    ...(b.unattributedTokens > 0
      ? [{ key: 'unattributed-usage', color: UNATTRIBUTED_COLOR, val: b.unattributedTokens }]
      : []),
  ];

  const costLabel = summary === null ? '…' : formatSummaryCost(summary, settings, lang);

  // Exactly one bucket is always selected — moved by hovering a bar. The
  // selection is keyed by bucket key AND its window AND the effective
  // granularity: a window or interval change (new winId) drops the old key back
  // to the peak, while a background refresh (same window+interval) keeps the
  // key if it survives.
  const winId = `${range}|${from}|${to}|${per}`;
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

  // The floor rule (ADR-0017) against the dialog's own window and against the
  // inspected bucket: a figure is marked when unreadable content could fall in
  // it, i.e. its start precedes some Unreadable Artifact's last write.
  const windowFloor = tokenFloor(unreadable, rangeToFilters(range, from, to).startTs ?? null, lang);
  const selFloor = selKey
    ? tokenFloor(unreadable, bucketFilters(selKey, per).startTs ?? null, lang)
    : NO_FLOOR;

  // Inspector read-outs for the selected bucket.
  const selRank = selBucket ? 1 + data.filter((b) => b.total > selBucket.total).length : 0;
  const selDeltaPct = selBucket && avg > 0 ? Math.round((selBucket.total / avg - 1) * 100) : 0;
  const selDate = (b: Bucket) =>
    per === 'hour'
      ? b.key.slice(11, 16)
      : per === 'month'
        ? `${monthShortL(parseInt(b.key.slice(5, 7), 10) - 1, lang)} ${b.key.slice(0, 4)}`
        : fmtIsoDateL(b.key, lang);
  // Top-N models, largest first; the rest fold into one muted remainder row
  // (color '' → grey). Shared by the bucket read-out in the inspector (top-6)
  // and the window-scoped breakdown under the footer stats (top-5, so the
  // section stays short and the chart keeps its height on small windows).
  type InspRow = { key: string; name: string; source?: string; val: number; color: string; more: boolean };
  const buildRows = (
    byModel: Record<string, number>,
    modelSources: Record<string, string[]> | undefined,
    unattributedTokens: number,
    top: number,
  ): InspRow[] => {
    const rows: InspRow[] = [];
    const ranked = rankedModels(byModel);
    for (const [m, v] of ranked.slice(0, top)) {
      // Owner resolved from THIS record: a Model name shared by two Sources
      // would otherwise take whichever the window-wide map happened to pick.
      const owner = modelOwner(modelSources, modelTool, m);
      rows.push({ key: m, name: m, source: owner.label, val: v, color: owner.color, more: false });
    }
    const rest = ranked.slice(top);
    if (rest.length) {
      rows.push({
        key: '__more__',
        name: `${rest.length} ${t('overview.trend.moreModels')}`,
        val: rest.reduce((a, [, v]) => a + v, 0),
        color: '',
        more: true,
      });
    }
    if (unattributedTokens > 0) {
      rows.push({
        key: 'unattributed-usage',
        name: t('overview.unattributedUsage'),
        val: unattributedTokens,
        color: UNATTRIBUTED_COLOR,
        more: false,
      });
    }
    return rows;
  };
  const selRows: InspRow[] = selBucket
    ? buildRows(selBucket.byModel, selBucket.modelSources, selBucket.unattributedTokens, 6)
    : [];
  // The whole window's model split, aggregated across its buckets (byModel
  // summed; modelSources unioned so a Model name two Sources share reads
  // 'Codex + pi' rather than claiming one Source).
  const win = useMemo(() => {
    const byModel: Record<string, number> = {};
    const modelSources: Record<string, string[]> = {};
    let unattributedTokens = 0;
    for (const b of data) {
      for (const [m, v] of Object.entries(b.byModel)) byModel[m] = (byModel[m] ?? 0) + v;
      for (const [m, srcs] of Object.entries(b.modelSources ?? {})) {
        const owners = (modelSources[m] ??= []);
        for (const s of srcs) if (!owners.includes(s)) owners.push(s);
      }
      unattributedTokens += b.unattributedTokens;
    }
    return { byModel, modelSources, unattributedTokens };
  }, [data]);
  const winRows: InspRow[] = buildRows(win.byModel, win.modelSources, win.unattributedTokens, 5);
  const selTotal = selBucket?.total ?? 0;
  // WebKit can't resolve var() in an SVG stroke; pick the outline per theme.
  const outline = colors === CHART_LIGHT ? '#12151b' : '#e8ecf4';

  const rowEl = (r: InspRow, base: number) => (
    <div className={'tt-trend-insp-row' + (r.more ? ' more' : '')} key={r.key}>
      <div className="lab">
        <span className="dot" style={r.more ? undefined : { background: r.color }} />
        <span className="name">
          {r.name}
          {r.source && <em className="src">{r.source}</em>}
        </span>
        <span className="num">
          {fmtTok(r.val)} <span className="pct">{fmtPct(r.val / (base || 1))}</span>
        </span>
      </div>
      <div className="track">
        <div className="fill" style={{ width: (r.val / (base || 1)) * 100 + '%', background: r.more ? undefined : r.color }} />
      </div>
    </div>
  );
  const rowsList = (rows: InspRow[], base: number) => (
    <div className="tt-trend-insp-rows">
      {rows.map((r) => rowEl(r, base))}
      {rows.length === 0 && <div className="tt-trend-insp-empty">{t('overview.noActivity')}</div>}
    </div>
  );

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
      <span className="tl-modal-dragstrip" aria-hidden="true" data-tauri-drag-region />
      <section
        ref={modalRef}
        className="tt-trend-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="tt-trend-modal-title"
        tabIndex={-1}
      >
        {/* the backdrop covers the shell's drag handles, so the dialog's own
            header is one — "deep" so the whole strip drags, its buttons still
            click (Tauri exempts clickable elements) */}
        <header className="tt-trend-modal-head" data-tauri-drag-region="deep">
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
            {INTERVALS.map((iv) => (
              <button key={iv} className={barInterval === iv ? 'active' : ''} onClick={() => setBarInterval(iv)}>
                {t(INTERVAL_LABEL_KEY[iv])}
              </button>
            ))}
          </div>
          {/* The dialog's window is its own, so its picker writes to the local
              custom dates — never to the page's, and never to the remembered
              range (only useOverview saves). */}
          <RangeSegments
            range={range}
            onRange={setRange}
            from={from}
            to={to}
            firstIso={firstIso}
            lastIso={lastIso}
            points={allPoints}
            onPick={(fromIso, toIso) => { setCustomFrom(fromIso); setCustomTo(toIso); }}
          />
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
                {/* Hover targets hug the drawn bar (its width + height from the
                    baseline), so the empty space above a short bar doesn't
                    select it. A zero bar has no target. */}
                {data.map((b, i) => {
                  const bh = h(b.total);
                  return (
                    <rect key={'hit' + i} x={PL + i * slot + (slot - barW) / 2} y={BASE - bh} width={barW} height={bh} fill="transparent" onMouseOver={() => setSel({ win: winId, key: b.key })} />
                  );
                })}
              </svg>
            </div>

            <div className="tt-trend-modal-foot">
              <div className="tt-trend-modal-stats">
                <div className="stat" title={windowFloor.reason || undefined}>
                  <b>{markedTokenFigure(fmtTok(total), windowFloor)}</b>
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
                  {summary && summary.unattributedTokens > 0 && (
                    <span className="mark">{t('overview.unattributedUsage')}</span>
                  )}
                </div>
              </div>
              <div className="tt-legend">
                {activeTools.map((tl) => (
                  <span className="item" key={tl.key}>
                    <span className="sw" style={{ background: tl.color }} />
                    {tl.label}
                  </span>
                ))}
              </div>
            </div>
            {winRows.length > 0 && (
              // The window's own model split, under the footer stats it
              // describes — the inspector on the right stays per-bucket.
              <div className="tt-trend-models">
                <div className="head">{t('overview.trend.byModel')} · {rangeLabel}</div>
                <div className="grid">{winRows.map((r) => rowEl(r, total))}</div>
              </div>
            )}
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
                <div className="tt-trend-insp-tok" title={selFloor.reason || undefined}>
                  <span className="val">{markedTokenFigure(fmtTok(selTotal), selFloor)}</span>
                  <span className="unit">{t('overview.tokens')}</span>
                </div>
                <div className={'tt-trend-insp-delta ' + (selDeltaPct >= 0 ? 'up' : 'down')}>
                  {selDeltaPct >= 0 ? '+' : '−'}{Math.abs(selDeltaPct)}% {t('overview.trend.vsAvg')}
                </div>
                <div className="tt-trend-insp-div" />
                <div className="eyebrow">{t('overview.trend.byModel')}</div>
                {rowsList(selRows, selTotal)}
                <div className="tt-trend-insp-cost">
                  <span>{t('overview.estCost')}</span>
                  <span className="amt">
                    {selCost === null ? '…' : formatSummaryCost(selCost, settings, lang)}
                    {selCost?.hasUnpriced && (
                      <span className="mark" title={selCost.unpricedModels.join(', ')}>
                        {' '}· {selCost.unpricedModels.length} {t('overview.unpricedMarker')}
                      </span>
                    )}
                    {selCost && selCost.unattributedTokens > 0 && (
                      <span className="mark"> {' '}· {t('overview.unattributedUsage')}</span>
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
