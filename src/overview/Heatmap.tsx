import { memo, useCallback, useMemo, useRef, useState } from 'react';
import { heatStats, type Day } from './data';
import { fmtTok } from '../lib/format';
import { fmtDateL, fmtWeekdayDateL, monthShortL, weekdayShortL, useOverviewT } from './localize';
import { useChartColors, CHART_LIGHT } from '../lib/chartColors';
import Landscape3D from './Landscape3D';

type Mode = '2d' | '3d';

// DS violet intensity ramp (index 0 = empty cell, 1..4 ascending). WebKit can't
// resolve var() in SVG fills, so these mirror the DS tokens per theme.
export const HEAT_DARK = ['#1c1f27', '#1e3a8a', '#2563eb', '#3b82f6', '#60a5fa'];
export const HEAT_LIGHT = ['#EDEDF0', '#bfdbfe', '#93c5fd', '#3b82f6', '#1d4ed8'];

// ---- 2D grid geometry (design v2: fixed pitch, horizontal scroll) ----
const C2 = 22; // cell pitch
const CELL = 18; // cell size (pitch minus gap)
const OX = 2; // left inset
const OY = 26; // room for month labels

// Monday-first row for the 2D grid (Day.row stays Sunday-first for the 3D scene).
const row2d = (d: Day) => (d.weekday + 6) % 7;

function Heatmap({
  days,
  compact = false,
  onEnlarge,
  enlargeRef,
}: {
  days: Day[];
  compact?: boolean;
  onEnlarge?: () => void;
  enlargeRef?: (el: HTMLButtonElement | null) => void;
}) {
  const { t, lang } = useOverviewT();
  const [mode, setMode] = useState<Mode>('2d');
  const [hover, setHover] = useState<Day | null>(null);
  const [pos, setPos] = useState<{ x: number; y: number; flip: boolean }>({ x: 0, y: 0, flip: false });
  const [pos2, setPos2] = useState<{ x: number; y: number; w: number }>({ x: 0, y: 0, w: 1 });
  const wrapRef = useRef<HTMLDivElement>(null);

  // Theme-aware ramp: useChartColors returns the exact CHART_LIGHT/CHART_DARK
  // constant, so reference equality tells us which theme is live.
  const colors = useChartColors();
  const ramp = colors === CHART_LIGHT ? HEAT_LIGHT : HEAT_DARK;
  const accent = ramp[3];

  const cols = useMemo(() => Math.max(1, ...days.map((d) => d.col)) + 1, [days]);
  const stats = useMemo(() => heatStats(days), [days]);

  // month labels sit above each month's first column; months squeezed into a
  // single column (window edges) stay unlabeled
  const monthLabels = useMemo(() => {
    const starts: { c: number; m: number }[] = [];
    let prevCol = -1;
    let lastM = -1;
    for (const d of days) {
      if (d.col === prevCol) continue;
      prevCol = d.col;
      const m = d.date.getMonth();
      if (m !== lastM) {
        starts.push({ c: d.col, m });
        lastM = m;
      }
    }
    return starts.flatMap((s, i) => {
      const end = i + 1 < starts.length ? starts[i + 1].c : cols;
      return end - s.c >= 2 ? [{ x: OX + s.c * C2, label: monthShortL(s.m, lang) }] : [];
    });
  }, [days, cols, lang]);

  const dayLabels = useMemo(() => [1, 2, 3, 4, 5, 6, 0].map((dow) => weekdayShortL(dow, lang)), [lang]);

  const pxW = OX + cols * C2 + 4;
  const pxH = OY + 7 * C2 + 2;

  // open on the most recent weeks; stable identity so re-renders keep scroll
  const scrollInit = useCallback((el: HTMLDivElement | null) => {
    if (el) el.scrollLeft = el.scrollWidth;
  }, []);

  function onMove(e: React.MouseEvent<HTMLDivElement>) {
    const w = e.currentTarget.clientWidth;
    setPos({ x: e.nativeEvent.offsetX, y: e.nativeEvent.offsetY, flip: e.nativeEvent.offsetX > w * 0.58 });
  }

  // 2D tooltip anchors to the hovered cell (in wrap coordinates), not the mouse
  const enter2d = (d: Day) => (e: React.MouseEvent<SVGRectElement>) => {
    const host = wrapRef.current;
    if (host) {
      const r = e.currentTarget.getBoundingClientRect();
      const hb = host.getBoundingClientRect();
      setPos2({ x: r.left + r.width / 2 - hb.left, y: r.top - hb.top, w: hb.width });
    }
    setHover(d);
  };
  const leave = () => setHover(null);

  // tooltip per-model rows for the hovered day
  const tipRows = hover
    ? Object.entries(hover.byModel)
        .map(([model, val]) => ({ key: model, label: model, val }))
        .filter((r) => r.val > 0)
        .sort((a, b) => b.val - a.val)
        .slice(0, 3)
    : [];
  const tipMax = Math.max(1, ...tipRows.map((r) => r.val));

  const hint = mode === '2d' ? t('overview.fullYearScroll') : t('overview.hoverDay');
  // WebKit can't resolve var() in SVG strokes either — pick the outline per theme.
  const outline = ramp === HEAT_LIGHT ? '#12151b' : '#e8ecf4';
  const lf = pos2.w ? pos2.x / pos2.w : 0.5;
  const tip2dTransform = `translate(${lf < 0.14 ? '-8%' : lf > 0.86 ? '-92%' : '-50%'}, ${pos2.y < 96 ? '30px' : '-116%'})`;

  return (
    <div className="tt-card heat">
      <div className="tt-head">
        <div>
          <div className="tt-title">{t('overview.activity')}</div>
          <div className="tt-sub">
            {compact ? (
              hint
            ) : (
              <>
                <span style={{ color: accent, fontWeight: 650 }}>{fmtTok(stats.totalTokens)}</span> {t('overview.tokens')} · {hint}
              </>
            )}
          </div>
        </div>
        <div className="tt-head-actions">
          <div className="tt-seg">
            <button className={mode === '2d' ? 'active' : ''} onClick={() => setMode('2d')}>
              2D
            </button>
            <button className={mode === '3d' ? 'active' : ''} onClick={() => setMode('3d')}>
              3D
            </button>
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

      <div
        ref={wrapRef}
        className="tt-heat-wrap"
        onMouseMove={mode === '3d' ? onMove : undefined}
        onMouseLeave={leave}
        style={compact ? { height: 186 } : undefined}
      >
        {mode === '2d' ? (
          <div className="tt-heat2d">
            <div className="tt-heat2d-days">
              {dayLabels.map((l, i) => (
                <span key={i}>{l}</span>
              ))}
            </div>
            <div className="tt-heat2d-scroll" ref={scrollInit}>
              <div className="tt-heat2d-inner" style={{ width: pxW }}>
                {monthLabels.map((m, i) => (
                  <span key={i} className="tt-heat2d-month" style={{ left: m.x }}>
                    {m.label}
                  </span>
                ))}
                <svg width={pxW} height={pxH} viewBox={`0 0 ${pxW} ${pxH}`}>
                  {days.map((d) => (
                    <rect
                      key={d.index}
                      x={OX + d.col * C2}
                      y={OY + row2d(d) * C2}
                      width={CELL}
                      height={CELL}
                      rx={4.5}
                      fill={ramp[d.level]}
                      style={{ cursor: 'pointer' }}
                      onMouseEnter={enter2d(d)}
                    />
                  ))}
                  {hover && (
                    <rect
                      x={OX + hover.col * C2}
                      y={OY + row2d(hover) * C2}
                      width={CELL}
                      height={CELL}
                      rx={4.5}
                      fill="none"
                      stroke={outline}
                      strokeWidth={1.5}
                      style={{ pointerEvents: 'none' }}
                    />
                  )}
                </svg>
              </div>
            </div>
          </div>
        ) : (
          <Landscape3D days={days} ramp={ramp} onHoverDay={setHover} />
        )}

        {hover && (
          <div
            className="tt-tip"
            style={
              mode === '2d'
                ? { left: pos2.x, top: pos2.y, transform: tip2dTransform }
                : {
                    left: pos.x,
                    top: pos.y,
                    transform: `translate(${pos.flip ? 'calc(-100% - 14px)' : '14px'}, -50%)`,
                  }
            }
          >
            <div className="tt-tip-head">
              <b>{fmtWeekdayDateL(hover.date, lang)}</b>
              <span className="tt-tip-badge">Lv {hover.level}</span>
            </div>
            <div className="tt-tip-tok">
              <b>{fmtTok(hover.tokens)}</b>
              <span>{t('overview.tokens')}</span>
            </div>
            {tipRows.map((r) => (
              <div className="tt-tip-row" key={r.key}>
                <div className="lab">
                  <span>{r.label}</span>
                  <span>{fmtTok(r.val)}</span>
                </div>
                <div className="track">
                  <div className="fill" style={{ width: (r.val / tipMax) * 100 + '%', background: accent }} />
                </div>
              </div>
            ))}
            {tipRows.length === 0 && <div className="tt-ctx-meta">{t('overview.noActivity')}</div>}
          </div>
        )}
      </div>

      <div className="tt-foot">
        <div className="tt-stats">
          <div className="tt-stat">
            <b>{stats.activeDays}</b>
            <span>{t('overview.activeDays')}</span>
          </div>
          <div className="tt-stat">
            <b style={{ color: ramp[4] }}>{stats.streak}</b>
            <span>{t('overview.dayStreak')}</span>
          </div>
          {!compact && (
            <div className="tt-stat">
              <b>{fmtTok(stats.bestDay.tokens)}</b>
              <span>{t('overview.best')} · {fmtDateL(stats.bestDay.date, lang)}</span>
            </div>
          )}
        </div>
        <div className="tt-heat-legend">
          <span>{t('overview.heatLess')}</span>
          {ramp.map((c, i) => (
            <span key={i} className="cell" style={{ background: c }} />
          ))}
          <span>{t('overview.heatMore')}</span>
        </div>
      </div>
    </div>
  );
}

// Memoized: props stay identity-stable across the shell's per-tick re-renders.
export default memo(Heatmap);
