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

// The camera: yaw/pitch orbit around a look-at target (tx, ty — grid coords
// relative to the scene center); zoom scales the viewport. Zooming at the
// cursor walks the target toward the point under it, so rotation pivots
// around what you zoomed into; zooming back out to 1 recenters.
export interface View3D {
  yaw: number;
  pitch: number;
  zoom: number;
  tx: number;
  ty: number;
}
export const INITIAL_VIEW: View3D = { yaw: -0.14, pitch: Math.asin(BY / AX), zoom: 1, tx: 0, ty: 0 };
const ZOOM_MAX = 8;

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

// Ground-plane (z = 0) inverse of projectPoint — the projection restricted to
// the ground is linear and invertible at any pitch inside the clamp.
export function unprojectGround(sx: number, sy: number, yaw: number, pitch: number): [number, number] {
  const u = sx / AX;
  const w = sy / (AX * Math.sin(pitch));
  const rx = (u + w) / 2;
  const ry = (w - u) / 2;
  const cos = Math.cos(yaw);
  const sin = Math.sin(yaw);
  return [rx * cos + ry * sin, ry * cos - rx * sin];
}

// One wheel step: scale the zoom by `factor`, keeping the grid point under
// `anchor` (viewBox coords; `boxCenter` is the viewport's fixed center) exactly
// under the cursor — the look-at target absorbs the difference, so the
// rotation pivot converges on the spot being zoomed into. Zoom is clamped to
// [1, ZOOM_MAX]; arriving back at 1 snaps the target home to the scene center.
export function zoomView(
  view: View3D,
  factor: number,
  anchor: [number, number],
  boxCenter: [number, number],
): View3D {
  const zoom = Math.min(ZOOM_MAX, Math.max(1, view.zoom * factor));
  if (zoom === view.zoom) return view;
  if (zoom === 1) return { ...view, zoom, tx: 0, ty: 0 };
  // The cursor's viewBox coordinate after the viewport rescales around its
  // fixed center; the target shifts by the ground-plane preimage of the gap.
  const shrink = view.zoom / zoom;
  const dvx = (anchor[0] - boxCenter[0]) * shrink + boxCenter[0] - anchor[0];
  const dvy = (anchor[1] - boxCenter[1]) * shrink + boxCenter[1] - anchor[1];
  const [gx, gy] = unprojectGround(dvx, dvy, view.yaw, view.pitch);
  return { ...view, zoom, tx: view.tx - gx, ty: view.ty - gy };
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
  zoomable = false,
  onHoverDay,
}: {
  days: Day[];
  ramp: string[];
  view: View3D;
  onView: (v: View3D) => void;
  zoomable?: boolean; // wheel zoom — only where the page can't scroll (the enlarge)
  onHoverDay?: (d: Day | null) => void;
}) {
  const svgRef = useRef<SVGSVGElement>(null);
  const cols = useMemo(() => Math.max(1, ...days.map((d) => d.col)) + 1, [days]);
  const { yaw, pitch, zoom, tx, ty } = view;

  // The viewport rescales around the base box's fixed center as the zoom
  // changes; the wheel handler needs that center to anchor its math.
  const camera = useMemo(() => {
    const b = sceneBounds(days, pitch);
    const pad = 10;
    const w = b.maxX - b.minX + pad * 2;
    const h = b.maxY - b.minY + pad * 2;
    const c: [number, number] = [(b.minX + b.maxX) / 2, (b.minY + b.maxY) / 2];
    const zw = w / zoom;
    const zh = h / zoom;
    return {
      center: c,
      viewBox: `${(c[0] - zw / 2).toFixed(1)} ${(c[1] - zh / 2).toFixed(1)} ${zw.toFixed(1)} ${zh.toFixed(1)}`,
    };
  }, [days, pitch, zoom]);
  const boxCenterRef = useRef(camera.center);
  boxCenterRef.current = camera.center;

  // Faces at the current angles, depth-sorted back-to-front. Ground edges of
  // the visible walls: east x = col+TILE, west x = col; south y = row+TILE,
  // north y = row. x-walls shade darker than y-walls at any angle.
  const cells = useMemo(() => {
    const cx = cols / 2;
    const p = (gx: number, gy: number, z: number) =>
      projectPoint(gx - cx - tx, gy - CY - ty, z, yaw, pitch);
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
  }, [yaw, pitch, tx, ty, ramp, days, cols]);

  // drag-to-orbit: listeners attach once, with the live view/onView riding in
  // refs — depending on the view would tear the listeners down on the first
  // move of a drag and reset `dragging`, freezing the rotation after one frame.
  const viewRef = useRef(view);
  viewRef.current = view;
  const onViewRef = useRef(onView);
  onViewRef.current = onView;
  const onHoverDayRef = useRef(onHoverDay);
  onHoverDayRef.current = onHoverDay;
  const zoomableRef = useRef(zoomable);
  zoomableRef.current = zoomable;
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
        ...start,
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
    // Wheel zoom at the cursor. getScreenCTM maps client px into viewBox
    // coords through the meet letterboxing; without layout (jsdom) fall back
    // to zooming about the viewport center.
    const wheel = (e: WheelEvent) => {
      if (!zoomableRef.current) return;
      e.preventDefault();
      const ctm = svg.getScreenCTM?.();
      const anchor: [number, number] = ctm
        ? (() => {
            const pt = new DOMPoint(e.clientX, e.clientY).matrixTransform(ctm.inverse());
            return [pt.x, pt.y];
          })()
        : boxCenterRef.current;
      onViewRef.current(
        zoomView(viewRef.current, Math.exp(-e.deltaY * 0.0015), anchor, boxCenterRef.current),
      );
    };
    svg.addEventListener('mousedown', down);
    svg.addEventListener('wheel', wheel, { passive: false });
    window.addEventListener('mousemove', move);
    window.addEventListener('mouseup', up);
    return () => {
      svg.removeEventListener('mousedown', down);
      svg.removeEventListener('wheel', wheel);
      window.removeEventListener('mousemove', move);
      window.removeEventListener('mouseup', up);
    };
  }, []);

  return (
    <svg ref={svgRef} className="grab" viewBox={camera.viewBox} preserveAspectRatio="xMidYMid meet">
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
