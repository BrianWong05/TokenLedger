import { useEffect, useMemo, useRef, useState } from 'react';
import { TOOLS, THEMES, THEME_OPTIONS, MONTHS, type Day } from './data';
import { fmtTok, fmtDate } from '../lib/format';

type Mode = '2d' | '3d';

// ---- 2D grid geometry ----
const S = 13; // cell size
const STEP = 16; // cell + gap
const TOP = 16; // room for month labels

// ---- 3D isometric geometry ----
const TILE = 0.86;
const AX = 10; // iso x scale
const BY = 5.4; // iso tilt scale
const ZUNIT = 8; // extruded height per level

function shade(hex: string, f: number): string {
  const n = parseInt(hex.slice(1), 16);
  const r = Math.round(((n >> 16) & 255) * f);
  const g = Math.round(((n >> 8) & 255) * f);
  const b = Math.round((n & 255) * f);
  return `rgb(${r},${g},${b})`;
}
const poly = (pts: [number, number][]) =>
  'M' + pts.map(([x, y]) => `${x.toFixed(1)} ${y.toFixed(1)}`).join('L') + 'Z';

export default function Heatmap({ days, compact = false }: { days: Day[]; compact?: boolean }) {
  const [mode, setMode] = useState<Mode>('2d');
  const [theme, setTheme] = useState('ocean');
  const [yaw, setYaw] = useState(-0.14);
  const [hover, setHover] = useState<Day | null>(null);
  const [pos, setPos] = useState<{ x: number; y: number; flip: boolean }>({ x: 0, y: 0, flip: false });
  const wrap = useRef<HTMLDivElement>(null);

  const ramp = THEMES[theme];
  const accent = ramp[3];

  const cols = useMemo(() => Math.max(1, ...days.map((d) => d.col)) + 1, [days]);
  const stats = useMemo(() => {
    const totalTokens = days.reduce((a, d) => a + d.tokens, 0);
    const activeDays = days.filter((d) => d.tokens > 0).length;
    let streak = 0, run = 0;
    for (const d of days) {
      if (d.tokens > 0) { run += 1; streak = Math.max(streak, run); } else run = 0;
    }
    const bestDay = days.reduce((a, d) => (d.tokens > a.tokens ? d : a), days[0]);
    return { totalTokens, activeDays, streak, bestDay };
  }, [days]);

  // month label columns for 2D
  const monthLabels = useMemo(() => {
    const out: { x: number; label: string }[] = [];
    let last = -1;
    for (const d of days) {
      const m = d.date.getMonth();
      if (d.date.getDate() <= 7 && m !== last) {
        out.push({ x: d.col * STEP, label: MONTHS[m] });
        last = m;
      }
    }
    return out;
  }, [days]);

  const view2d = `0 0 ${cols * STEP} ${TOP + 7 * STEP}`;

  // 3D faces, depth-sorted back-to-front, plus a fitted viewBox
  const three = useMemo(() => {
    const cos = Math.cos(yaw);
    const sin = Math.sin(yaw);
    const cx = cols / 2;
    const cy = 3.5;
    const proj = (gx: number, gy: number, z: number): [number, number] => {
      const x = gx - cx;
      const y = gy - cy;
      const rx = x * cos - y * sin;
      const ry = x * sin + y * cos;
      return [(rx - ry) * AX, (rx + ry) * BY - z];
    };
    const depth = (gx: number, gy: number) => {
      const x = gx - cx;
      const y = gy - cy;
      return x * sin + y * cos; // larger = nearer
    };

    let minX = Infinity;
    let minY = Infinity;
    let maxX = -Infinity;
    let maxY = -Infinity;
    const track = (p: [number, number]) => {
      minX = Math.min(minX, p[0]);
      maxX = Math.max(maxX, p[0]);
      minY = Math.min(minY, p[1]);
      maxY = Math.max(maxY, p[1]);
    };

    const cells = days.map((d) => {
      const c = d.col;
      const r = d.row;
      const z = d.level * ZUNIT;
      const g = [proj(c, r, 0), proj(c + TILE, r, 0), proj(c + TILE, r + TILE, 0), proj(c, r + TILE, 0)];
      const t = [proj(c, r, z), proj(c + TILE, r, z), proj(c + TILE, r + TILE, z), proj(c, r + TILE, z)];
      [...g, ...t].forEach(track);
      const top = ramp[d.level];
      return {
        d,
        depth: depth(c + TILE / 2, r + TILE / 2),
        top: poly([t[0], t[1], t[2], t[3]]),
        right: poly([g[1], g[2], t[2], t[1]]),
        front: poly([g[3], g[2], t[2], t[3]]),
        topFill: top,
        rightFill: shade(top, 0.62),
        frontFill: shade(top, 0.8),
      };
    }).sort((a, b) => a.depth - b.depth);

    const pad = 10;
    const viewBox = `${(minX - pad).toFixed(1)} ${(minY - pad).toFixed(1)} ${(maxX - minX + pad * 2).toFixed(1)} ${(maxY - minY + pad * 2).toFixed(1)}`;
    return { cells, viewBox };
  }, [yaw, ramp, days, cols]);

  // drag-to-rotate (3D only)
  useEffect(() => {
    if (mode !== '3d') return;
    let startX = 0;
    let startYaw = yaw;
    let dragging = false;
    const svg = wrap.current?.querySelector('svg');
    const down = (e: MouseEvent) => {
      dragging = true;
      startX = e.clientX;
      startYaw = yaw;
      (svg as SVGElement)?.classList.add('grabbing');
    };
    const move = (e: MouseEvent) => {
      if (!dragging) return;
      const next = startYaw + (e.clientX - startX) * 0.005;
      setYaw(Math.max(-0.5, Math.min(0.4, next)));
    };
    const up = () => {
      dragging = false;
      (svg as SVGElement)?.classList.remove('grabbing');
    };
    svg?.addEventListener('mousedown', down);
    window.addEventListener('mousemove', move);
    window.addEventListener('mouseup', up);
    return () => {
      svg?.removeEventListener('mousedown', down);
      window.removeEventListener('mousemove', move);
      window.removeEventListener('mouseup', up);
    };
  }, [mode, yaw]);

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
          <div className="tt-title">Activity</div>
          <div className="tt-sub">
            {compact ? (
              mode === '3d' ? 'drag to rotate' : 'hover a day'
            ) : (
              <>
                <span style={{ color: accent, fontWeight: 650 }}>{fmtTok(stats.totalTokens)}</span> tokens ·
                {mode === '3d' ? ' drag to rotate' : ' hover a day'}
              </>
            )}
          </div>
        </div>
        <div style={{ display: 'flex', alignItems: 'center', gap: 9 }}>
          <div className="tt-seg">
            <button className={mode === '2d' ? 'active' : ''} onClick={() => setMode('2d')}>
              2D
            </button>
            <button className={mode === '3d' ? 'active' : ''} onClick={() => setMode('3d')}>
              3D
            </button>
          </div>
          {!compact && (
            <div className="tt-select-wrap">
              <select className="tt-select" value={theme} onChange={(e) => setTheme(e.target.value)}>
                {THEME_OPTIONS.map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.label}
                  </option>
                ))}
              </select>
              <i>▼</i>
            </div>
          )}
        </div>
      </div>

      <div
        className="tt-heat-wrap"
        ref={wrap}
        onMouseMove={onMove}
        onMouseLeave={leave}
        style={compact ? { height: 186 } : undefined}
      >
        {mode === '2d' ? (
          <svg viewBox={view2d} preserveAspectRatio="xMidYMid meet">
            {monthLabels.map((m, i) => (
              <text key={i} x={m.x} y={11} fill="#6d7793" fontSize="10" fontFamily="ui-monospace,monospace">
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
                stroke="rgba(255,255,255,.04)"
                style={{ cursor: 'pointer' }}
                onMouseEnter={enter(d)}
              />
            ))}
          </svg>
        ) : (
          <svg className="grab" viewBox={three.viewBox} preserveAspectRatio="xMidYMid meet">
            {three.cells.map((c) => (
              <g key={c.d.index}>
                {c.d.level > 0 && <path d={c.right} fill={c.rightFill} />}
                {c.d.level > 0 && <path d={c.front} fill={c.frontFill} />}
                <path
                  d={c.top}
                  fill={c.topFill}
                  stroke="rgba(0,0,0,.25)"
                  strokeWidth={0.4}
                  style={{ cursor: 'pointer' }}
                  onMouseEnter={enter(c.d)}
                />
              </g>
            ))}
          </svg>
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
              <b>{hover.date.toLocaleDateString('en-US', { weekday: 'short', month: 'short', day: 'numeric' })}</b>
              <span className="tt-tip-badge">Lv {hover.level}</span>
            </div>
            <div className="tt-tip-tok">
              <b>{fmtTok(hover.tokens)}</b>
              <span>tokens</span>
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
            {tipRows.length === 0 && <div className="tt-ctx-meta">No activity</div>}
          </div>
        )}
      </div>

      <div className="tt-foot">
        <div className="tt-stats">
          <div className="tt-stat">
            <b>{stats.activeDays}</b>
            <span>active days</span>
          </div>
          <div className="tt-stat">
            <b style={{ color: accent }}>{stats.streak}</b>
            <span>day streak</span>
          </div>
          {!compact && (
            <div className="tt-stat">
              <b>{fmtTok(stats.bestDay.tokens)}</b>
              <span>best · {fmtDate(stats.bestDay.date)}</span>
            </div>
          )}
        </div>
        <div className="tt-heat-legend">
          <span>Less</span>
          {ramp.map((c, i) => (
            <span key={i} className="cell" style={{ background: c }} />
          ))}
          <span>More</span>
        </div>
      </div>
    </div>
  );
}
