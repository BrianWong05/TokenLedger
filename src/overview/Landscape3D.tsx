import { useEffect, useMemo, useRef } from 'react';
import type { Day } from './data';

// Isometric activity landscape: each day is an extruded tile whose height tracks
// its intensity level. Shared by the Activity card's 3D mode and the full-screen
// enlarge — it owns the view angle and drag-to-rotate; the parent renders any
// tooltip via onHoverDay so hover stays consistent with the 2D grid.

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

export const INITIAL_YAW = -0.14;
const YAW_MIN = -0.5;
const YAW_MAX = 0.4;

const proj = (
  x: number,
  y: number,
  z: number,
  cos: number,
  sin: number,
): [number, number] => {
  const rx = x * cos - y * sin;
  const ry = x * sin + y * cos;
  return [(rx - ry) * AX, (rx + ry) * BY - z];
};

export default function Landscape3D({
  days,
  ramp,
  yaw,
  onYaw,
  onHoverDay,
}: {
  days: Day[];
  ramp: string[];
  yaw: number;
  onYaw: (y: number) => void;
  onHoverDay?: (d: Day | null) => void;
}) {
  const svgRef = useRef<SVGSVGElement>(null);
  const cols = useMemo(() => Math.max(1, ...days.map((d) => d.col)) + 1, [days]);

  // Fixed camera: the viewBox fits the scene's bounds across the whole yaw
  // range, not the current angle — refitting per frame would rescale and
  // recenter the scene while dragging, so rotation reads as warping.
  const viewBox = useMemo(() => {
    const cx = cols / 2;
    const cy = 3.5;
    let minX = Infinity;
    let minY = Infinity;
    let maxX = -Infinity;
    let maxY = -Infinity;
    for (let i = 0; i <= 6; i++) {
      const th = YAW_MIN + (i / 6) * (YAW_MAX - YAW_MIN);
      const cos = Math.cos(th);
      const sin = Math.sin(th);
      for (const d of days) {
        for (const [gx, gy] of [
          [d.col, d.row],
          [d.col + TILE, d.row],
          [d.col + TILE, d.row + TILE],
          [d.col, d.row + TILE],
        ]) {
          const [px, py] = proj(gx - cx, gy - cy, 0, cos, sin);
          minX = Math.min(minX, px);
          maxX = Math.max(maxX, px);
          maxY = Math.max(maxY, py); // ground is the low edge
          minY = Math.min(minY, py - d.level * ZUNIT); // top face is the high edge
        }
      }
    }
    const pad = 10;
    return `${(minX - pad).toFixed(1)} ${(minY - pad).toFixed(1)} ${(maxX - minX + pad * 2).toFixed(1)} ${(maxY - minY + pad * 2).toFixed(1)}`;
  }, [days, cols]);

  // Faces at the current angle, depth-sorted back-to-front.
  const cells = useMemo(() => {
    const cos = Math.cos(yaw);
    const sin = Math.sin(yaw);
    const cx = cols / 2;
    const cy = 3.5;
    const p = (gx: number, gy: number, z: number) => proj(gx - cx, gy - cy, z, cos, sin);
    const depth = (gx: number, gy: number) => (gx - cx) * sin + (gy - cy) * cos; // larger = nearer

    return days
      .map((d) => {
        const c = d.col;
        const r = d.row;
        const z = d.level * ZUNIT;
        const g = [p(c, r, 0), p(c + TILE, r, 0), p(c + TILE, r + TILE, 0), p(c, r + TILE, 0)];
        const t = [p(c, r, z), p(c + TILE, r, z), p(c + TILE, r + TILE, z), p(c, r + TILE, z)];
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
      })
      .sort((a, b) => a.depth - b.depth);
  }, [yaw, ramp, days, cols]);

  // drag-to-rotate: listeners attach once, with the live yaw/onYaw riding in
  // refs — depending on yaw would tear the listeners down on the first move of
  // a drag and reset `dragging`, freezing the rotation after one frame.
  const yawRef = useRef(yaw);
  yawRef.current = yaw;
  const onYawRef = useRef(onYaw);
  onYawRef.current = onYaw;
  const onHoverDayRef = useRef(onHoverDay);
  onHoverDayRef.current = onHoverDay;
  const draggingRef = useRef(false);
  useEffect(() => {
    const svg = svgRef.current;
    if (!svg) return;
    let startX = 0;
    let startYaw = 0;
    const down = (e: MouseEvent) => {
      e.preventDefault(); // WebKit: keep the drag from starting a selection
      draggingRef.current = true;
      startX = e.clientX;
      startYaw = yawRef.current;
      onHoverDayRef.current?.(null); // no tooltip while rotating
      svg.classList.add('grabbing');
    };
    const move = (e: MouseEvent) => {
      if (!draggingRef.current) return;
      // minus: the front of the landscape follows the cursor
      const next = startYaw - (e.clientX - startX) * 0.005;
      onYawRef.current(Math.max(YAW_MIN, Math.min(YAW_MAX, next)));
    };
    const up = () => {
      draggingRef.current = false;
      svg.classList.remove('grabbing');
    };
    svg.addEventListener('mousedown', down);
    window.addEventListener('mousemove', move);
    window.addEventListener('mouseup', up);
    return () => {
      svg.removeEventListener('mousedown', down);
      window.removeEventListener('mousemove', move);
      window.removeEventListener('mouseup', up);
    };
  }, []);

  return (
    <svg ref={svgRef} className="grab" viewBox={viewBox} preserveAspectRatio="xMidYMid meet">
      {cells.map((c) => (
        <g key={c.d.index}>
          {c.d.level > 0 && <path d={c.right} fill={c.rightFill} />}
          {c.d.level > 0 && <path d={c.front} fill={c.frontFill} />}
          <path
            d={c.top}
            fill={c.topFill}
            stroke="rgba(0,0,0,.25)"
            strokeWidth={0.4}
            style={{ cursor: 'pointer' }}
            onMouseEnter={() => {
              if (!draggingRef.current) onHoverDay?.(c.d);
            }}
          />
        </g>
      ))}
    </svg>
  );
}
