import type { SmallMultipleItem } from './data';
import { fmtTok, fmtPct } from '../lib/format';

// ---- per-tool small multiples (one sparkline per tool) ----
export default function SmallMultiples({ items, rangeLabel }: { items: SmallMultipleItem[]; rangeLabel: string }) {
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
