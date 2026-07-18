import { useMemo, useState } from 'react';
import { TOOLS } from './meta';
import { heatStats, type Day } from './data';
import { fmtTok } from '../lib/format';
import { fmtDateL, fmtWeekdayDateL, monthShortL, useOverviewT } from './localize';
import { useChartColors, CHART_LIGHT } from '../lib/chartColors';
import Landscape3D, { INITIAL_YAW } from './Landscape3D';

type Mode = '2d' | '3d';

// DS violet intensity ramp (index 0 = empty cell, 1..4 ascending). WebKit can't
// resolve var() in SVG fills, so these mirror the DS tokens per theme.
export const HEAT_DARK = ['#1c1f27', '#1e3a8a', '#2563eb', '#3b82f6', '#60a5fa'];
export const HEAT_LIGHT = ['#EDEDF0', '#bfdbfe', '#93c5fd', '#3b82f6', '#1d4ed8'];

// ---- 2D grid geometry ----
const S = 13; // cell size
const STEP = 16; // cell + gap
const TOP = 16; // room for month labels

export default function Heatmap({
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
  const [yaw, setYaw] = useState(INITIAL_YAW);
  const [hover, setHover] = useState<Day | null>(null);
  const [pos, setPos] = useState<{ x: number; y: number; flip: boolean }>({ x: 0, y: 0, flip: false });

  // Theme-aware ramp: useChartColors returns the exact CHART_LIGHT/CHART_DARK
  // constant, so reference equality tells us which theme is live.
  const colors = useChartColors();
  const ramp = colors === CHART_LIGHT ? HEAT_LIGHT : HEAT_DARK;
  const accent = ramp[3];

  const cols = useMemo(() => Math.max(1, ...days.map((d) => d.col)) + 1, [days]);
  const stats = useMemo(() => heatStats(days), [days]);

  // month label columns for 2D
  const monthLabels = useMemo(() => {
    const out: { x: number; label: string }[] = [];
    let last = -1;
    for (const d of days) {
      const m = d.date.getMonth();
      if (d.date.getDate() <= 7 && m !== last) {
        out.push({ x: d.col * STEP, label: monthShortL(m, lang) });
        last = m;
      }
    }
    return out;
  }, [days, lang]);

  const view2d = `0 0 ${cols * STEP} ${TOP + 7 * STEP}`;

  function onMove(e: React.MouseEvent<HTMLDivElement>) {
    const w = e.currentTarget.clientWidth;
    setPos({ x: e.nativeEvent.offsetX, y: e.nativeEvent.offsetY, flip: e.nativeEvent.offsetX > w * 0.58 });
  }

  const enter = (d: Day) => () => setHover(d);
  const leave = () => setHover(null);

  // tooltip per-tool rows for the hovered day
  const tipRows = hover
    ? TOOLS.map((t) => ({ ...t, val: hover.byTool[t.key] }))
        .filter((r) => r.val > 0)
        .sort((a, b) => b.val - a.val)
        .slice(0, 3)
    : [];
  const tipMax = Math.max(1, ...tipRows.map((r) => r.val));

  return (
    <div className="tt-card heat">
      <div className="tt-head">
        <div>
          <div className="tt-title">{t('overview.activity')}</div>
          <div className="tt-sub">
            {compact ? (
              mode === '3d' ? t('overview.dragRotate') : t('overview.hoverDay')
            ) : (
              <>
                <span style={{ color: accent, fontWeight: 650 }}>{fmtTok(stats.totalTokens)}</span> {t('overview.tokens')} ·
                {mode === '3d' ? ' ' + t('overview.dragRotate') : ' ' + t('overview.hoverDay')}
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
        className="tt-heat-wrap"
        onMouseMove={onMove}
        onMouseLeave={leave}
        style={compact ? { height: 186 } : undefined}
      >
        {mode === '2d' ? (
          <svg viewBox={view2d} preserveAspectRatio="xMidYMid meet">
            {monthLabels.map((m, i) => (
              <text key={i} x={m.x} y={11} fontSize="10" style={{ fill: 'var(--text-secondary)' }}>
                {m.label}
              </text>
            ))}
            {days.map((d) => (
              <rect
                key={d.index}
                x={d.col * STEP}
                y={TOP + d.row * STEP}
                width={S}
                height={S}
                rx={2.5}
                fill={ramp[d.level]}
                stroke={colors.grid}
                style={{ cursor: 'pointer' }}
                onMouseEnter={enter(d)}
              />
            ))}
          </svg>
        ) : (
          <Landscape3D days={days} ramp={ramp} yaw={yaw} onYaw={setYaw} onHoverDay={setHover} />
        )}

        {hover && (
          <div
            className="tt-tip"
            style={{
              left: pos.x,
              top: pos.y,
              transform: `translate(${pos.flip ? 'calc(-100% - 14px)' : '14px'}, -50%)`,
            }}
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
