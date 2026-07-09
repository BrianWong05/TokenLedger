import { useMemo, useState } from 'react';
import './overview.css';
import Heatmap from './Heatmap';
import ContextBreakdown from './ContextBreakdown';
import ModelsList from './ModelsList';
import BreakdownTable from './BreakdownTable';
import {
  TOOLS,
  MODELS,
  DAYS,
  RANGES_8B,
  sliceDays,
  daysBetween,
  sumTokens,
  toolTotalsOf,
  bucketsOf,
  smallMultiples,
  perOf,
  fmtIsoDate,
  FIRST_ISO,
  LAST_ISO,
  costOf,
  fmtTok,
  fmtUSD,
  fmtPct,
  type Range8b,
  type ToolKey,
  type Bucket,
} from './mock';

const NAV = ['Overview', 'Insights', 'Models', 'Settings'];

// Design 8b — "App · Overview": totals + tool cards + heatmap + aggregate trend
// + per-tool small multiples + context/models, all scoped to a global range.
export default function Overview8b() {
  const [nav, setNav] = useState('Overview');
  const [range, setRange] = useState<Range8b>('total');
  const [sel, setSel] = useState<ToolKey>('claude');
  const [customFrom, setCustomFrom] = useState(sliceDays('month')[0].iso);
  const [customTo, setCustomTo] = useState(LAST_ISO);

  const view = useMemo(() => {
    const days = range === 'custom' ? daysBetween(customFrom, customTo) : sliceDays(range);
    const total = sumTokens(days);
    return {
      days,
      count: days.length,
      total,
      cost: costOf(total),
      toolTotals: toolTotalsOf(days),
      trend: bucketsOf(days, range),
      sparks: smallMultiples(days, range),
    };
  }, [range, customFrom, customTo]);

  const rangeLabel =
    range === 'custom' ? `${fmtIsoDate(customFrom)} – ${fmtIsoDate(customTo)}` : RANGES_8B.find((r) => r.key === range)!.long;
  const per = perOf(range, view.count);
  const grand = view.total || 1;
  const tool = TOOLS.find((t) => t.key === sel)!;

  return (
    <div className="tt">
      <div className="tt-app">
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
            <span className="tt-avatar">MK</span>
          </div>
        </div>

        {range === 'custom' && (
          <div className="tt-custom-row">
            <span className="lbl">Custom range</span>
            <input
              type="date"
              value={customFrom}
              min={FIRST_ISO}
              max={customTo}
              onChange={(e) => e.target.value && setCustomFrom(e.target.value)}
            />
            <span className="to">to</span>
            <input
              type="date"
              value={customTo}
              min={customFrom}
              max={LAST_ISO}
              onChange={(e) => e.target.value && setCustomTo(e.target.value)}
            />
          </div>
        )}

        <div className="tt-b8-body">
          <div className="tt-b8-head">
            <div className="tt-eyebrow">Total tokens · {rangeLabel}</div>
            <div className="tt-b8-total">{fmtTok(view.total)}</div>
            <div className="tt-b8-cost">{fmtUSD(view.cost)} est.</div>
          </div>

          <div className="tt-split">
            {TOOLS.map((t) => (
              <div key={t.key} style={{ width: fmtPct(view.toolTotals[t.key] / grand), background: t.color }} />
            ))}
          </div>

          <div className="tt-toolcards">
            {TOOLS.map((t) => {
              const active = t.key === sel;
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
                  <div className="sub">{MODELS[t.key].length} models</div>
                </button>
              );
            })}
          </div>

          <div className="tt-b8-grid">
            <div className="tt-b8-col">
              <Heatmap days={DAYS} compact />
              <AggTrend data={view.trend} per={per} rangeLabel={rangeLabel} />
              <SmallMultiples items={view.sparks} rangeLabel={rangeLabel} />
            </div>

            <div className="tt-b8-col">
              <div>
                <ContextBreakdown tool={tool} toolTokens={view.toolTotals[sel]} showBars />
              </div>
              <div>
                <ModelsList tool={tool} toolTokens={view.toolTotals[sel]} />
              </div>
            </div>
          </div>

          <BreakdownTable days={view.days} total={view.total} />
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
  const peak = data.reduce((a, b) => (b.total > a.total ? b : a), data[0]);
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
