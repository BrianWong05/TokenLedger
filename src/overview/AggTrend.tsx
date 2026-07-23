import { memo, useMemo, useState } from 'react';
import { TOOLS, emptyByTool } from './meta';
import { modelColor, rankedModels, stackModels, UNATTRIBUTED_COLOR, type Bucket } from './data';
import { fmtTok, fmtPct } from '../lib/format';
import { PER_UNIT_KEY, useOverviewT } from './localize';
import { useChartColors } from '../lib/chartColors';

// ---- aggregate usage-trend bars (no interval toggle; driven by the range) ----
const VW = 560;
// Left gutter: fits the widest right-aligned y label fmtTok can produce
// ("999.99B" measures ~38px at 9px) plus the 6px tick gap, so labels never
// spill into the plot and get painted over by the bars.
const PL = 45;
const PR = 8;
const PT = 14;
const BASE = 176;
const LABEL_Y = 194;

function AggTrend({
  data,
  per,
  rangeLabel,
  modelTool,
  onEnlarge,
  enlargeRef,
}: {
  data: Bucket[];
  per: string;
  rangeLabel: string;
  modelTool: Record<string, string>;
  onEnlarge?: () => void;
  enlargeRef?: (el: HTMLButtonElement | null) => void;
}) {
  const { t } = useOverviewT();
  const colors = useChartColors();
  const [hover, setHover] = useState<number | null>(null);
  const [pos, setPos] = useState<{ x: number; y: number; flip: boolean }>({ x: 0, y: 0, flip: false });
  const maxTotal = Math.max(1, ...data.map((b) => b.total));
  const total = data.reduce((a, b) => a + b.total, 0);
  const avg = total / (data.length || 1);
  const peak = data.reduce((a, b) => (b.total > a.total ? b : a), data[0] ?? {
    key: '', label: '—', byTool: emptyByTool(), byModel: {},
    unattributedTokens: 0, hasUnpriced: false, total: 0,
  });
  const plotW = VW - PL - PR;
  const slot = plotW / (data.length || 1);
  const barW = Math.min(38, slot * 0.62);
  const h = (v: number) => (v / maxTotal) * (BASE - PT);
  const grid = [0, 1, 2, 3, 4].map((i) => ({ y: BASE - (i / 4) * (BASE - PT), label: fmtTok((maxTotal * i) / 4) }));
  const shown = hover != null ? data[hover] : null;
  const dense = data.length > 16;

  // Segments are per model but colored by the model's tool; grouping the stack
  // by tool keeps each bar reading as contiguous tool blocks.
  const colorOf = (m: string) => modelColor(modelTool, m);
  // Two Sources can run the same Model name (pi and codex both report
  // `gpt-5.6-sol`), so a bare model name is ambiguous. Name the owning Source
  // from the same map that picks the segment colour, so label and colour agree.
  const sourceOf = (m: string) => TOOLS.find((tl) => tl.key === modelTool[m])?.label;
  const models = useMemo(() => stackModels(data, modelTool), [data, modelTool]);
  const segsOf = (b: Bucket) => [
    ...models.map((m) => ({ key: m, color: colorOf(m), val: b.byModel[m] ?? 0 })),
    ...(b.unattributedTokens > 0
      ? [{ key: 'unattributed-usage', color: UNATTRIBUTED_COLOR, val: b.unattributedTokens }]
      : []),
  ];

  // Hovered bucket's model rows, largest first.
  const tipRows = shown
    ? [
        ...rankedModels(shown.byModel).map(([m, v]) => ({
          key: m, label: m, source: sourceOf(m), val: v, color: colorOf(m),
        })),
        ...(shown.unattributedTokens > 0
          ? [{
              key: 'unattributed-usage',
              label: t('overview.unattributedUsage'),
              source: undefined,
              val: shown.unattributedTokens,
              color: UNATTRIBUTED_COLOR,
            }]
          : []),
      ]
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
          <div className="tt-sub">{t('overview.stackedByTool')} · {rangeLabel}</div>
        </div>
        <div className="tt-head-actions">
          <div style={{ textAlign: 'right' }}>
            <div className="tt-read-big">{fmtTok(shown ? shown.total : total)}</div>
            <div style={{ fontSize: 10.5, color: 'var(--text-tertiary)' }}>{shown ? shown.label : t('overview.total')}</div>
          </div>
          {onEnlarge && (
            <button ref={enlargeRef} type="button" className="tt-heat-enlarge" onClick={onEnlarge} title={t('overview.enlarge')} aria-label={t('overview.enlarge')}>
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <path d="M15 3h6v6" />
                <path d="M9 21H3v-6" />
                <path d="m21 3-7 7" />
                <path d="m3 21 7-7" />
              </svg>
            </button>
          )}
        </div>
      </div>
      <div style={{ marginTop: 12, position: 'relative' }} onMouseMove={onMove}>
        <svg viewBox={`0 0 ${VW} 200`} preserveAspectRatio="xMidYMid meet" onMouseLeave={() => setHover(null)} style={{ width: '100%', display: 'block' }}>
          {grid.map((g, i) => (
            <g key={i}>
              <line x1={PL} y1={g.y} x2={VW - PR} y2={g.y} stroke={colors.grid} strokeWidth={1} />
              <text x={PL - 6} y={g.y} dy={3.2} fontSize={9} textAnchor="end" style={{ fill: 'var(--text-tertiary)' }}>
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
            <text key={'x' + i} x={PL + i * slot + slot / 2} y={LABEL_Y} fontSize={9} textAnchor="middle" style={{ fill: 'var(--text-tertiary)' }}>
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
                  <span>
                    {r.label}
                    {r.source && <em className="src">{r.source}</em>}
                  </span>
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
            <b style={{ color: 'var(--success-text)' }}>{fmtTok(peak.total)}</b>
            <span>{t('overview.peak')} · {peak.label}</span>
          </div>
        </div>
        <div className="tt-legend">
          {TOOLS.filter((t) => data.some((b) => b.byTool[t.key] > 0)).map((t) => (
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

// Memoized: props stay identity-stable across the shell's per-tick re-renders.
export default memo(AggTrend);
