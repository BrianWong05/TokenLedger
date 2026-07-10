import { useEffect, useMemo, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import './overview.css';
import Heatmap from './Heatmap';
import ContextBreakdown from './ContextBreakdown';
import TokenBreakdown from './TokenBreakdown';
import ModelsList from './ModelsList';
import BreakdownTable from './BreakdownTable';
import { scan, fetchSeries, fetchSummary, fetchBreakdown, fetchCtxResources, fetchCtxBuckets, fetchCtxTools, fetchCtxExec } from '../api';
import type { BreakdownRow, SeriesPoint, Summary, CtxResourceCount, CtxBuckets, CtxToolRow, CtxExecRow } from '../types';
import {
  TOOLS,
  RANGES_8B,
  isoOf,
  calendarSpan,
  emptyByTool,
  seriesToDays,
  windowOf,
  pointsIn,
  granularityOf,
  bucketsFromPoints,
  smallMultiples,
  toolTotalsOfPoints,
  sumPoints,
  catTotals,
  dailyTableRows,
  projectTableRows,
  modelBars,
  ctxTotals,
  ctxMeta,
  rangeToFilters,
  type Range8b,
  type ToolKey,
  type Bucket,
} from './data';
import { fmtTok, fmtPct, fmtIsoDate, formatCost } from '../lib/format';

const NAV = ['Overview', 'Insights', 'Models', 'Settings'];
const EMPTY_FILTERS = { tools: [], models: [], project: null };

// Design 8b — "App · Overview", wired to the real Ledger. One unbounded daily
// series powers heatmap/trends/tables via client-side slicing; summary and
// breakdowns re-fetch per range; an hourly series serves the Day view.
export default function Overview8b() {
  const [nav, setNav] = useState('Overview');
  const [range, setRange] = useState<Range8b>('total');
  const [sel, setSel] = useState<ToolKey>('claude');
  const [customFrom, setCustomFrom] = useState('');
  const [customTo, setCustomTo] = useState('');

  const [allPoints, setAllPoints] = useState<SeriesPoint[] | null>(null);
  const [hourPoints, setHourPoints] = useState<SeriesPoint[]>([]);
  const [sum, setSum] = useState<Summary | null>(null);
  const [modelRows, setModelRows] = useState<BreakdownRow[]>([]);
  const [projRows, setProjRows] = useState<BreakdownRow[]>([]);
  const [ctxRes, setCtxRes] = useState<CtxResourceCount[]>([]);
  const [ctxBuckets, setCtxBuckets] = useState<CtxBuckets[]>([]);
  const [ctxToolRows, setCtxToolRows] = useState<CtxToolRow[]>([]);
  const [ctxExecRows, setCtxExecRows] = useState<CtxExecRow[]>([]);
  // Scan problems persist until the next scan; fetch problems clear on the
  // next successful fetch cycle — one transient failure must not stick.
  const [scanError, setScanError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pricesVersion, setPricesVersion] = useState(0);

  // Mount: scan the logs, then load the whole ledger's daily series once.
  useEffect(() => {
    (async () => {
      try {
        const status = await scan();
        const errs = status.sources.filter((s) => s.error).map((s) => `${s.source}: ${s.error}`);
        setScanError(errs.length ? errs.join(' · ') : null);
      } catch (e) {
        setScanError(String(e));
      }
      try {
        setAllPoints(await fetchSeries(EMPTY_FILTERS, 'day'));
      } catch (e) {
        setError(String(e));
        setAllPoints([]);
      }
    })();
  }, []);

  // The backend rebuilds prices off-thread at startup; when they land, re-run
  // the per-range fetches so a fresh install doesn't render 'unpriced' until
  // the user happens to change range.
  useEffect(() => {
    const un = listen('prices-rebuilt', () => setPricesVersion((v) => v + 1));
    return () => {
      un.then((f) => f());
    };
  }, []);

  const firstIso = allPoints?.length
    ? allPoints.reduce((a, p) => (p.bucket < a ? p.bucket : a), allPoints[0].bucket)
    : isoOf(new Date());
  const lastIso = isoOf(new Date());
  const cf = customFrom || firstIso;
  const ct = customTo || lastIso;

  // Per-range data: authoritative cost + right column + project table (+ hourly
  // on Day). The cleanup marks in-flight responses stale so a slow previous
  // range can never overwrite a newer one; custom-range keystrokes debounce.
  useEffect(() => {
    if (allPoints === null) return;
    let stale = false;
    const run = () => {
      const filters = rangeToFilters(range, cf, ct);
      const jobs: Promise<unknown>[] = [
        fetchSummary(filters).then((v) => { if (!stale) setSum(v); }),
        fetchBreakdown('model', filters).then((v) => { if (!stale) setModelRows(v); }),
        fetchBreakdown('project', filters).then((v) => { if (!stale) setProjRows(v); }),
        fetchCtxResources(filters).then((v) => { if (!stale) setCtxRes(v); }),
        fetchCtxBuckets(filters).then((v) => { if (!stale) setCtxBuckets(v); }),
        fetchCtxTools(filters).then((v) => { if (!stale) setCtxToolRows(v); }),
        fetchCtxExec(filters).then((v) => { if (!stale) setCtxExecRows(v); }),
      ];
      if (range === 'day') {
        jobs.push(fetchSeries(filters, 'hour').then((v) => { if (!stale) setHourPoints(v); }));
      } else {
        setHourPoints((prev) => (prev.length ? [] : prev));
      }
      Promise.all(jobs)
        .then(() => { if (!stale) setError(null); })
        .catch((e) => { if (!stale) setError(String(e)); });
    };
    const t = window.setTimeout(run, range === 'custom' ? 250 : 0);
    return () => {
      stale = true;
      window.clearTimeout(t);
    };
  }, [allPoints, range, cf, ct, pricesVersion]);

  // The 365-day heatmap grid only depends on the full series — not on
  // range/tool selection — so it gets its own memo.
  const days = useMemo(() => seriesToDays(allPoints ?? []), [allPoints]);

  const view = useMemo(() => {
    const pts = allPoints ?? [];
    const win = windowOf(range, cf, ct);
    const rpts = pointsIn(pts, win);
    // Calendar span of the window (not active-day count) drives granularity;
    // 'total' spans the whole ledger.
    const from = win.fromIso ?? firstIso;
    const to = win.toIso ?? lastIso;
    const per = granularityOf(range, calendarSpan(from, to));
    const trend =
      per === 'hour'
        ? bucketsFromPoints(hourPoints, 'hour', from, to)
        : bucketsFromPoints(rpts, per, from, to);
    return {
      rpts,
      total: sumPoints(rpts),
      toolTotals: toolTotalsOfPoints(rpts),
      per,
      trend,
      sparks: smallMultiples(trend),
      cats: catTotals(rpts, sel),
      ctx: ctxTotals(rpts, sel),
      dailyRows: dailyTableRows(rpts),
    };
  }, [allPoints, hourPoints, range, cf, ct, sel, firstIso, lastIso]);

  const rangeLabel =
    range === 'custom' ? `${fmtIsoDate(cf)} – ${fmtIsoDate(ct)}` : RANGES_8B.find((r) => r.key === range)!.long;
  const grand = view.total || 1;
  const tool = TOOLS.find((t) => t.key === sel)!;
  const loading = allPoints === null;

  return (
    <div className="tt">
      <div className={'tt-app' + (loading ? ' tt-loading' : '')}>
        <div className="tt-top">
          <div className="tt-brand">
            <div className="tt-logo">
              <i>T</i>
              <b>tokentracker</b>
            </div>
            <div className="tt-nav">
              {NAV.map((n) => (
                <button key={n} className={n === nav ? 'active' : ''} onClick={() => setNav(n)}>
                  {n}
                </button>
              ))}
            </div>
          </div>
          <div className="tt-top-right">
            <div className="tt-seg">
              {RANGES_8B.map((r) => (
                <button key={r.key} className={range === r.key ? 'active' : ''} onClick={() => setRange(r.key)}>
                  {r.label}
                </button>
              ))}
            </div>
            <span className="tt-avatar">BW</span>
          </div>
        </div>

        {(scanError || error) && (
          <div className="tt-error">{[scanError, error].filter(Boolean).join(' · ')}</div>
        )}

        {range === 'custom' && (
          <div className="tt-custom-row">
            <span className="lbl">Custom range</span>
            <input
              type="date"
              value={cf}
              min={firstIso}
              max={ct}
              onChange={(e) => e.target.value && setCustomFrom(e.target.value)}
            />
            <span className="to">to</span>
            <input
              type="date"
              value={ct}
              min={cf}
              max={lastIso}
              onChange={(e) => e.target.value && setCustomTo(e.target.value)}
            />
          </div>
        )}

        <div className="tt-b8-body">
          <div className="tt-b8-head">
            <div className="tt-eyebrow">Total tokens · {rangeLabel}</div>
            <div className="tt-b8-total">{fmtTok(sum?.totalTokens ?? view.total)}</div>
            <div className="tt-b8-cost">
              {sum ? formatCost(sum.cost, sum.hasUnpriced) : '…'} est.
              {sum?.hasUnpriced && (
                <span title={sum.unpricedModels.join(', ')}> · {sum.unpricedModels.length} unpriced</span>
              )}
              {sum && sum.cacheEstimatedModels.length > 0 && (
                <span title={sum.cacheEstimatedModels.join(', ')}>
                  {' '}· {sum.cacheEstimatedModels.length} cache est.
                </span>
              )}
            </div>
          </div>

          <div className="tt-split">
            {TOOLS.map((t) => (
              <div key={t.key} style={{ width: fmtPct(view.toolTotals[t.key] / grand), background: t.color }} />
            ))}
          </div>

          <div className="tt-toolcards">
            {TOOLS.map((t) => {
              const active = t.key === sel;
              const nModels = modelRows.filter((r) => r.source === t.key).length;
              return (
                <button
                  key={t.key}
                  className={'tt-toolcard' + (active ? ' active' : '')}
                  onClick={() => setSel(t.key)}
                  style={active ? { borderColor: t.color, background: t.color + '1e' } : undefined}
                >
                  <div className="lbl">
                    <span className="dot" style={{ background: t.color }} />
                    {t.label}
                  </div>
                  <div className="num">{fmtPct(view.toolTotals[t.key] / grand)}</div>
                  <div className="sub">{nModels} model{nModels === 1 ? '' : 's'}</div>
                </button>
              );
            })}
          </div>

          <div className="tt-b8-grid">
            <div className="tt-b8-col">
              <Heatmap days={days} compact />
              <AggTrend data={view.trend} per={view.per} rangeLabel={rangeLabel} />
              <SmallMultiples items={view.sparks} rangeLabel={rangeLabel} />
            </div>

            <div className="tt-b8-col">
              <div>
                <ContextBreakdown
                  tool={tool}
                  ctx={view.ctx}
                  buckets={ctxBuckets.find((b) => b.source === sel) ?? null}
                  toolRows={ctxToolRows.filter((r) => r.source === sel)}
                  execRows={ctxExecRows.filter((r) => r.source === sel)}
                  meta={ctxMeta(ctxRes, sel)}
                />
              </div>
              <div>
                <TokenBreakdown tool={tool} cats={view.cats} />
              </div>
              <div>
                <ModelsList
                  tool={tool}
                  toolTokens={view.toolTotals[sel]}
                  models={modelBars(modelRows, sel, view.toolTotals[sel])}
                />
              </div>
            </div>
          </div>

          <BreakdownTable dailyRows={view.dailyRows} projectRows={projectTableRows(projRows)} />
        </div>
      </div>
    </div>
  );
}

// ---- aggregate usage-trend bars (no interval toggle; driven by the range) ----
const VW = 560;
const PL = 30;
const PR = 8;
const PT = 14;
const BASE = 176;
const LABEL_Y = 194;

function AggTrend({ data, per, rangeLabel }: { data: Bucket[]; per: string; rangeLabel: string }) {
  const [hover, setHover] = useState<number | null>(null);
  const maxTotal = Math.max(1, ...data.map((b) => b.total));
  const total = data.reduce((a, b) => a + b.total, 0);
  const avg = total / (data.length || 1);
  const peak = data.reduce((a, b) => (b.total > a.total ? b : a), data[0] ?? { label: '—', byTool: emptyByTool(), total: 0 });
  const plotW = VW - PL - PR;
  const slot = plotW / (data.length || 1);
  const barW = Math.min(38, slot * 0.62);
  const h = (v: number) => (v / maxTotal) * (BASE - PT);
  const grid = [0, 1, 2, 3, 4].map((i) => ({ y: BASE - (i / 4) * (BASE - PT), label: fmtTok((maxTotal * i) / 4) }));
  const shown = hover != null ? data[hover] : null;
  const dense = data.length > 16;

  return (
    <div className="tt-card">
      <div className="tt-head">
        <div>
          <div className="tt-title">Usage trend</div>
          <div className="tt-sub">Stacked by tool · {rangeLabel}</div>
        </div>
        <div style={{ textAlign: 'right' }}>
          <div className="tt-read-big">{fmtTok(shown ? shown.total : total)}</div>
          <div style={{ fontSize: 10.5, color: 'var(--tt-mut3)' }}>{shown ? shown.label : 'total'}</div>
        </div>
      </div>
      <div style={{ marginTop: 12 }}>
        <svg viewBox={`0 0 ${VW} 200`} preserveAspectRatio="xMidYMid meet" onMouseLeave={() => setHover(null)} style={{ width: '100%', display: 'block' }}>
          {grid.map((g, i) => (
            <g key={i}>
              <line x1={PL} y1={g.y} x2={VW - PR} y2={g.y} stroke="rgba(255,255,255,.06)" strokeWidth={1} />
              <text x={PL} y={g.y} dy={-4} fill="#5f6880" fontSize={9} fontFamily="ui-monospace,monospace">
                {g.label}
              </text>
            </g>
          ))}
          {data.map((b, i) => {
            const x = PL + i * slot + (slot - barW) / 2;
            let y = BASE;
            const op = hover == null || hover === i ? 1 : 0.32;
            return (
              <g key={i} opacity={op} style={{ transition: 'opacity .2s' }}>
                {TOOLS.map((t) => {
                  const seg = h(b.byTool[t.key]);
                  y -= seg;
                  return <rect key={t.key} x={x} y={y} width={barW} height={Math.max(0, seg)} fill={t.color} />;
                })}
              </g>
            );
          })}
          {data.map((_, i) => (
            <rect key={'h' + i} x={PL + i * slot} y={PT} width={slot} height={BASE - PT} fill="transparent" onMouseEnter={() => setHover(i)} style={{ cursor: 'pointer' }} />
          ))}
          {data.map((b, i) => (
            <text key={'x' + i} x={PL + i * slot + slot / 2} y={LABEL_Y} fill="#6d7793" fontSize={9} fontFamily="ui-monospace,monospace" textAnchor="middle">
              {dense && i % 2 ? '' : b.label}
            </text>
          ))}
        </svg>
      </div>
      <div className="tt-foot">
        <div className="tt-stats">
          <div className="tt-stat">
            <b>{fmtTok(avg)}</b>
            <span>avg / {per}</span>
          </div>
          <div className="tt-stat">
            <b style={{ color: 'var(--tt-green)' }}>{fmtTok(peak.total)}</b>
            <span>peak · {peak.label}</span>
          </div>
        </div>
        <div className="tt-legend">
          {TOOLS.map((t) => (
            <span className="item" key={t.key}>
              <span className="sw" style={{ background: t.color }} />
              {t.label}
            </span>
          ))}
        </div>
      </div>
    </div>
  );
}

// ---- per-tool small multiples (one sparkline per tool) ----
function SmallMultiples({ items, rangeLabel }: { items: ReturnType<typeof smallMultiples>; rangeLabel: string }) {
  const W = 100;
  const H = 40;
  return (
    <div className="tt-card">
      <div className="tt-head">
        <div>
          <div className="tt-title">Per-tool trend</div>
          <div className="tt-sub">{rangeLabel}</div>
        </div>
      </div>
      <div className="tt-sm-grid">
        {items.map((it) => {
          const series = it.series.length ? it.series : [0];
          const max = Math.max(1, ...series);
          const pts = series.map((v, i): [number, number] => [
            series.length > 1 ? (i / (series.length - 1)) * W : 0,
            H - (v / max) * H,
          ]);
          const line = 'M' + pts.map((p) => `${p[0].toFixed(1)} ${p[1].toFixed(1)}`).join('L');
          const area = `${line}L${W} ${H}L0 ${H}Z`;
          const peak = pts[series.indexOf(max)] || [0, H];
          return (
            <div className="tt-sm-card" key={it.key}>
              <div className="top">
                <span className="lbl">
                  <span className="dot" style={{ background: it.color }} />
                  {it.label}
                </span>
                <span className="share">{fmtPct(it.share)}</span>
              </div>
              <div className="num">{fmtTok(it.total)}</div>
              <svg viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" style={{ width: '100%', height: 44, display: 'block', marginTop: 6 }}>
                <path d={area} fill={it.color} opacity={0.14} />
                <path d={line} fill="none" stroke={it.color} strokeWidth={2} strokeLinejoin="round" strokeLinecap="round" vectorEffect="non-scaling-stroke" />
                <circle cx={peak[0]} cy={peak[1]} r={3} fill={it.color} stroke="#0b0d15" strokeWidth={1.4} vectorEffect="non-scaling-stroke" />
              </svg>
            </div>
          );
        })}
      </div>
    </div>
  );
}
