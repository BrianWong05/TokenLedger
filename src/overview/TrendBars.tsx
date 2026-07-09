import { useMemo, useState } from 'react';
import { TOOLS, INTERVALS, buckets, fmtTok, type Interval } from './mock';

const VW = 720;
const PL = 32; // left pad (y labels)
const PR = 10;
const PT = 16; // top pad
const BASE = 224; // baseline y
const LABEL_Y = 242;

export default function TrendBars() {
  const [interval, setInterval] = useState<Interval>('M');
  const [hover, setHover] = useState<number | null>(null);

  const data = useMemo(() => buckets(interval), [interval]);
  const per = INTERVALS.find((i) => i.key === interval)!.per;

  const maxTotal = Math.max(1, ...data.map((b) => b.total));
  const total = data.reduce((a, b) => a + b.total, 0);
  const avg = total / data.length;
  const peak = data.reduce((a, b) => (b.total > a.total ? b : a), data[0]);

  const plotW = VW - PL - PR;
  const slot = plotW / data.length;
  const barW = Math.min(44, slot * 0.62);
  const h = (v: number) => (v / maxTotal) * (BASE - PT);

  // 4 gridlines
  const grid = [0, 1, 2, 3, 4].map((i) => {
    const v = (maxTotal * i) / 4;
    return { y: BASE - (i / 4) * (BASE - PT), label: fmtTok(v) };
  });

  const shown = hover != null ? data[hover] : null;

  return (
    <div className="tt-card">
      <div className="tt-head">
        <div>
          <div className="tt-title">Usage trend</div>
          <div className="tt-sub">Stacked by tool</div>
        </div>
        <div className="tt-seg">
          {INTERVALS.map((it) => (
            <button
              key={it.key}
              className={interval === it.key ? 'active' : ''}
              onClick={() => setInterval(it.key)}
            >
              {it.label}
            </button>
          ))}
        </div>
      </div>

      <div className="tt-read">
        <b>{fmtTok(shown ? shown.total : total)}</b>
        <span>{shown ? shown.label : `2025 · ${data.length} ${per}s`}</span>
      </div>

      <div style={{ marginTop: 8 }}>
        <svg viewBox={`0 0 ${VW} 250`} preserveAspectRatio="xMidYMid meet" onMouseLeave={() => setHover(null)} style={{ width: '100%', display: 'block' }}>
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
            <rect
              key={'hit' + i}
              x={PL + i * slot}
              y={PT}
              width={slot}
              height={BASE - PT}
              fill="transparent"
              onMouseEnter={() => setHover(i)}
              style={{ cursor: 'pointer' }}
            />
          ))}

          {data.map((b, i) => (
            <text
              key={'x' + i}
              x={PL + i * slot + slot / 2}
              y={LABEL_Y}
              fill="#6d7793"
              fontSize={9.5}
              fontFamily="ui-monospace,monospace"
              textAnchor="middle"
            >
              {b.label}
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
