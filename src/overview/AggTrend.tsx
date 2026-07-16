import { useMemo, useState } from 'react';
import { TOOLS, emptyByTool } from './meta';
import { rankModels, type Bucket } from './data';
import { fmtTok, fmtPct } from '../lib/format';
import { PER_UNIT_KEY, useOverviewT } from './localize';

// ---- aggregate usage-trend bars (no interval toggle; driven by the range) ----
const VW = 560;
const PL = 30;
const PR = 8;
const PT = 14;
const BASE = 176;
const LABEL_Y = 194;

export default function AggTrend({ data, per, rangeLabel, modelTool }: { data: Bucket[]; per: string; rangeLabel: string; modelTool: Record<string, string> }) {
  const { t } = useOverviewT();
  const [hover, setHover] = useState<number | null>(null);
  const [pos, setPos] = useState<{ x: number; y: number; flip: boolean }>({ x: 0, y: 0, flip: false });
  const maxTotal = Math.max(1, ...data.map((b) => b.total));
  const total = data.reduce((a, b) => a + b.total, 0);
  const avg = total / (data.length || 1);
  const peak = data.reduce((a, b) => (b.total > a.total ? b : a), data[0] ?? { key: '', label: '—', byTool: emptyByTool(), byModel: {}, total: 0 });
  const plotW = VW - PL - PR;
  const slot = plotW / (data.length || 1);
  const barW = Math.min(38, slot * 0.62);
  const h = (v: number) => (v / maxTotal) * (BASE - PT);
  const grid = [0, 1, 2, 3, 4].map((i) => ({ y: BASE - (i / 4) * (BASE - PT), label: fmtTok((maxTotal * i) / 4) }));
  const shown = hover != null ? data[hover] : null;
  const dense = data.length > 16;

  // Segments are per model but colored by the model's tool; grouping the stack
  // by tool keeps each bar reading as contiguous tool blocks.
  const colorOf = (m: string) => TOOLS.find((t) => t.key === modelTool[m])?.color ?? '#5f6880';
  const models = useMemo(() => {
    const toolIdx = (m: string) => {
      const i = TOOLS.findIndex((t) => t.key === modelTool[m]);
      return i < 0 ? TOOLS.length : i;
    };
    // rankModels is largest-first; the stable sort keeps that order within a tool.
    return rankModels(data).sort((a, b) => toolIdx(a) - toolIdx(b));
  }, [data, modelTool]);
  const segsOf = (b: Bucket) => models.map((m) => ({ key: m, color: colorOf(m), val: b.byModel[m] ?? 0 }));

  // Hovered bucket's model rows, largest first.
  const tipRows = shown
    ? Object.entries(shown.byModel)
        .filter(([, v]) => v > 0)
        .sort((a, b) => b[1] - a[1])
        .map(([m, v]) => ({ key: m, val: v, color: colorOf(m) }))
    : [];
  const tipMore = Math.max(0, tipRows.length - 6);

  function onMove(e: React.MouseEvent<HTMLDivElement>) {
    const w = e.currentTarget.clientWidth;
    setPos({ x: e.nativeEvent.offsetX, y: e.nativeEvent.offsetY, flip: e.nativeEvent.offsetX > w * 0.58 });
  }

  return (
    <div className="tt-card">
      <div className="tt-head">
        <div>
          <div className="tt-title">{t('overview.usageTrend')}</div>
          <div className="tt-sub">{t('overview.stackedByModel')} · {rangeLabel}</div>
        </div>
        <div style={{ textAlign: 'right' }}>
          <div className="tt-read-big">{fmtTok(shown ? shown.total : total)}</div>
          <div style={{ fontSize: 10.5, color: 'var(--tt-mut3)' }}>{shown ? shown.label : t('overview.total')}</div>
        </div>
      </div>
      <div style={{ marginTop: 12, position: 'relative' }} onMouseMove={onMove}>
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
                {segsOf(b).map((s) => {
                  const seg = h(s.val);
                  y -= seg;
                  return <rect key={s.key} x={x} y={y} width={barW} height={Math.max(0, seg)} fill={s.color} />;
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
        {shown && (
          <div
            className="tt-tip"
            style={{
              left: pos.x,
              top: pos.y,
              transform: `translate(${pos.flip ? 'calc(-100% - 14px)' : '14px'}, -50%)`,
            }}
          >
            <div className="tt-tip-head">
              <b>{shown.key || shown.label}</b>
            </div>
            <div className="tt-tip-tok">
              <b>{shown.total.toLocaleString('en-US')}</b>
              <span>{t('overview.tokens')}</span>
            </div>
            {tipRows.length > 0 && <div className="tt-tip-sec">{t('overview.modelBreakdown')}</div>}
            {tipRows.slice(0, 6).map((r) => (
              <div className="tt-tip-row" key={r.key}>
                <div className="lab">
                  <span>{r.key}</span>
                  <span>
                    {r.val.toLocaleString('en-US')} · {fmtPct(r.val / (shown.total || 1))}
                  </span>
                </div>
                <div className="track">
                  <div className="fill" style={{ width: (r.val / (shown.total || 1)) * 100 + '%', background: r.color }} />
                </div>
              </div>
            ))}
            {tipMore > 0 && <div className="tt-ctx-meta">+{tipMore} {t('overview.more')}</div>}
            {tipRows.length === 0 && <div className="tt-ctx-meta">{t('overview.noActivity')}</div>}
          </div>
        )}
      </div>
      <div className="tt-foot">
        <div className="tt-stats">
          <div className="tt-stat">
            <b>{fmtTok(avg)}</b>
            <span>{t('overview.avg')} / {t(PER_UNIT_KEY[per])}</span>
          </div>
          <div className="tt-stat">
            <b style={{ color: 'var(--tt-green)' }}>{fmtTok(peak.total)}</b>
            <span>{t('overview.peak')} · {peak.label}</span>
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
