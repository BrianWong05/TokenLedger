import { useEffect, useMemo, useRef } from 'react';
import type { Day } from './data';

// Isometric activity landscape: each day is an extruded tile whose height tracks
// its intensity level. Shared by the Activity card's 3D mode and the full-screen
// enlarge — it owns the view angles and drag-to-rotate; the parent renders any
// tooltip via onHoverDay so hover stays consistent with the 2D grid.
//
// The drag orbits on both axes: horizontal spins (yaw — unbounded, trig wraps
// it), vertical tilts (pitch — the camera's elevation, clamped so the scene
// never degenerates to a line or pure top-down). Each bar renders the two side
// walls that face the camera at the current yaw, with bar heights foreshortened
// by the pitch. The camera holds rigid bounds across the whole yaw circle and
// re-fits smoothly as the pitch changes.

export const TILE = 0.86;
const AX = 10; // iso x scale
const BY = 5.4; // iso tilt scale at the default pitch
export const ZUNIT = 8; // extruded height per level
const CY = 3.5; // grid vertical center (7 weekday rows)

// Pitch is the camera's elevation angle: asin(BY/AX) reproduces the original
// fixed tilt exactly; ZSCALE keeps bar heights identical at that default while
// foreshortening them physically (·cos pitch) as the view tilts top-down.
const PITCH_MIN = 0.3; // ~17° — near-horizon
const PITCH_MAX = 1.15; // ~66° — near top-down
const ZSCALE = 1 / Math.cos(Math.asin(BY / AX));

function shade(hex: string, f: number): string {
  const n = parseInt(hex.slice(1), 16);
  const r = Math.round(((n >> 16) & 255) * f);
  const g = Math.round(((n >> 8) & 255) * f);
  const b = Math.round((n & 255) * f);
  return `rgb(${r},${g},${b})`;
}
const poly = (pts: [number, number][]) =>
  'M' + pts.map(([x, y]) => `${x.toFixed(1)} ${y.toFixed(1)}`).join('L') + 'Z';

export interface View3D {
  yaw: number;
  pitch: number;
}
export const INITIAL_VIEW: View3D = { yaw: -0.14, pitch: Math.asin(BY / AX) };

// The orbit projection, in grid coordinates centered on the scene
// (x = col - cols/2, y = row - CY). Screen y grows downward; z subtracts,
// foreshortened by the pitch.
export function projectPoint(
  x: number,
  y: number,
  z: number,
  yaw: number,
  pitch: number = INITIAL_VIEW.pitch,
): [number, number] {
  const cos = Math.cos(yaw);
  const sin = Math.sin(yaw);
  const rx = x * cos - y * sin;
  const ry = x * sin + y * cos;
  return [(rx - ry) * AX, (rx + ry) * AX * Math.sin(pitch) - z * ZSCALE * Math.cos(pitch)];
}

// Which two of a bar's four side walls face the camera at this yaw: a wall is
// visible when its outward normal projects toward the viewer (positive screen
// y), so exactly one x-facing and one y-facing wall shows at any angle.
export function visibleWalls(yaw: number): { x: 'east' | 'west'; y: 'south' | 'north' } {
  const cos = Math.cos(yaw);
  const sin = Math.sin(yaw);
  return { x: cos + sin > 0 ? 'east' : 'west', y: cos - sin > 0 ? 'south' : 'north' };
}

// Camera bounds that contain the scene at EVERY yaw for the given pitch: the
// projection of a point of grid radius ρ stays within ±√2·ρ on both pre-scale
// axes, so the sweep of the whole rotation fits in the circumscribed-circle
// box (plus the tallest bar's pitch-foreshortened extrusion above the ground).
export function sceneBounds(
  days: Pick<Day, 'col' | 'row' | 'level'>[],
  pitch: number = INITIAL_VIEW.pitch,
): { minX: number; minY: number; maxX: number; maxY: number } {
  const cols = Math.max(1, ...days.map((d) => d.col)) + 1;
  const cx = cols / 2;
  let r2 = 0;
  let maxZ = 0;
  for (const d of days) {
    for (const [gx, gy] of [
      [d.col, d.row],
      [d.col + TILE, d.row],
      [d.col + TILE, d.row + TILE],
      [d.col, d.row + TILE],
    ]) {
      const dx = gx - cx;
      const dy = gy - CY;
      r2 = Math.max(r2, dx * dx + dy * dy);
    }
    maxZ = Math.max(maxZ, d.level * ZUNIT);
  }
  const r = Math.sqrt(r2) * Math.SQRT2;
  const ry = r * AX * Math.sin(pitch);
  return { minX: -r * AX, minY: -ry - maxZ * ZSCALE * Math.cos(pitch), maxX: r * AX, maxY: ry };
}

export default function Landscape3D({
  days,
  ramp,
  view,
  onView,
  onHoverDay,
}: {
  days: Day[];
  ramp: string[];
  view: View3D;
  onView: (v: View3D) => void;
  onHoverDay?: (d: Day | null) => void;
}) {
  const svgRef = useRef<SVGSVGElement>(null);
  const cols = useMemo(() => Math.max(1, ...days.map((d) => d.col)) + 1, [days]);
  const { yaw, pitch } = view;

  const viewBox = useMemo(() => {
    const b = sceneBounds(days, pitch);
    const pad = 10;
    return `${(b.minX - pad).toFixed(1)} ${(b.minY - pad).toFixed(1)} ${(b.maxX - b.minX + pad * 2).toFixed(1)} ${(b.maxY - b.minY + pad * 2).toFixed(1)}`;
  }, [days, pitch]);

  // Faces at the current angles, depth-sorted back-to-front. Ground edges of
  // the visible walls: east x = col+TILE, west x = col; south y = row+TILE,
  // north y = row. x-walls shade darker than y-walls at any angle.
  const cells = useMemo(() => {
    const cx = cols / 2;
    const p = (gx: number, gy: number, z: number) => projectPoint(gx - cx, gy - CY, z, yaw, pitch);
    // Painter's order: a tile whose ground center projects lower on screen
    // (larger screen y) is nearer the camera and must draw later.
    const depth = (gx: number, gy: number) => p(gx, gy, 0)[1];
    const walls = visibleWalls(yaw);

    return days
      .map((d) => {
        const c = d.col;
        const r = d.row;
        const z = d.level * ZUNIT;
        const top = ramp[d.level];
        const t = [p(c, r, z), p(c + TILE, r, z), p(c + TILE, r + TILE, z), p(c, r + TILE, z)];

        // Ground edge of each visible wall, as its two corners.
        const [xa, xb]: [number, number][] =
          walls.x === 'east'
            ? [[c + TILE, r], [c + TILE, r + TILE]]
            : [[c, r], [c, r + TILE]];
        const [ya, yb]: [number, number][] =
          walls.y === 'south'
            ? [[c, r + TILE], [c + TILE, r + TILE]]
            : [[c, r], [c + TILE, r]];
        const wallPoly = (a: [number, number], b: [number, number]) =>
          poly([p(a[0], a[1], 0), p(b[0], b[1], 0), p(b[0], b[1], z), p(a[0], a[1], z)]);

        return {
          d,
          depth: depth(c + TILE / 2, r + TILE / 2),
          top: poly([t[0], t[1], t[2], t[3]]),
          xWall: wallPoly(xa, xb),
          yWall: wallPoly(ya, yb),
          topFill: top,
          xFill: shade(top, 0.62),
          yFill: shade(top, 0.8),
        };
      })
      .sort((a, b) => a.depth - b.depth);
  }, [yaw, pitch, ramp, days, cols]);

  // drag-to-orbit: listeners attach once, with the live view/onView riding in
  // refs — depending on the view would tear the listeners down on the first
  // move of a drag and reset `dragging`, freezing the rotation after one frame.
  const viewRef = useRef(view);
  viewRef.current = view;
  const onViewRef = useRef(onView);
  onViewRef.current = onView;
  const onHoverDayRef = useRef(onHoverDay);
  onHoverDayRef.current = onHoverDay;
  const draggingRef = useRef(false);
  useEffect(() => {
    const svg = svgRef.current;
    if (!svg) return;
    let startX = 0;
    let startY = 0;
    let start: View3D = INITIAL_VIEW;
    const down = (e: MouseEvent) => {
      e.preventDefault(); // WebKit: keep the drag from starting a selection
      draggingRef.current = true;
      startX = e.clientX;
      startY = e.clientY;
      start = viewRef.current;
      onHoverDayRef.current?.(null); // no tooltip while rotating
      svg.classList.add('grabbing');
    };
    const move = (e: MouseEvent) => {
      if (!draggingRef.current) return;
      onViewRef.current({
        // minus: the front of the landscape follows the cursor; no clamp —
        // the spin is free and the trig wraps the angle.
        yaw: start.yaw - (e.clientX - startX) * 0.005,
        // drag down pulls the front over toward a top-down view.
        pitch: Math.min(PITCH_MAX, Math.max(PITCH_MIN, start.pitch + (e.clientY - startY) * 0.005)),
      });
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
          {c.d.level > 0 && <path d={c.xWall} fill={c.xFill} />}
          {c.d.level > 0 && <path d={c.yWall} fill={c.yFill} />}
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
