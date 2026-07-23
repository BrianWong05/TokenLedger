import { describe, it, expect } from 'vitest';
import {
  projectPoint,
  unprojectGround,
  visibleWalls,
  sceneBounds,
  zoomView,
  TILE,
  ZUNIT,
  INITIAL_VIEW,
} from './Landscape3D';
import { seriesToDays } from './data';
import type { SeriesPoint } from '../types';

// The free-spin geometry at the pure seam: which walls face the camera at a
// given angle, and that the fixed camera's bounds hold at every angle. These
// re-derive the scene's centering independently of the component on purpose —
// the duplication triangulates the projection contract.

function pt(bucket: string, totalTokens: number): SeriesPoint {
  return {
    bucket, source: 'claude', byModel: {}, unattributedTokens: 0, hasUnpriced: false,
    inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0,
    totalTokens, reasoningTokens: null, cost: 0, requests: 1, convs: 1,
    ctxMessages: null, ctxSystem: null, ctxReasoning: null, ctxToolcalls: null,
    ctxAgents: null, ctxMcp: null, ctxSkills: null,
  };
}

const TODAY = new Date(2026, 6, 10);
// A real 365-day grid with a few extruded bars (levels from the rank quartiles).
const DAYS = seriesToDays(
  [pt('2026-07-09', 1000), pt('2026-07-08', 400), pt('2026-01-15', 100), pt('2025-08-01', 900)],
  TODAY,
);

describe('visibleWalls', () => {
  it('shows exactly one x-facing and one y-facing wall in each quadrant', () => {
    expect(visibleWalls(0)).toEqual({ x: 'east', y: 'south' });
    expect(visibleWalls(INITIAL_VIEW.yaw)).toEqual({ x: 'east', y: 'south' }); // default view
    expect(visibleWalls(Math.PI / 2)).toEqual({ x: 'east', y: 'north' });
    expect(visibleWalls(Math.PI)).toEqual({ x: 'west', y: 'north' });
    expect(visibleWalls(-Math.PI / 2)).toEqual({ x: 'west', y: 'south' });
  });

  it('flips the y wall at the quarter-circle boundary', () => {
    const eps = 0.01;
    expect(visibleWalls(Math.PI / 4 - eps).y).toBe('south');
    expect(visibleWalls(Math.PI / 4 + eps).y).toBe('north');
    expect(visibleWalls(-Math.PI / 4 + eps).x).toBe('east');
    expect(visibleWalls(-Math.PI / 4 - eps).x).toBe('west');
  });

  it('is periodic — a full extra turn changes nothing', () => {
    for (const yaw of [0, 0.7, -1.3, 2.9]) {
      expect(visibleWalls(yaw + 2 * Math.PI)).toEqual(visibleWalls(yaw));
      expect(visibleWalls(yaw - 2 * Math.PI)).toEqual(visibleWalls(yaw));
    }
  });
});

describe('projectPoint', () => {
  it('wraps — a full turn maps every point to itself', () => {
    for (const yaw of [0, 0.7, -1.3, 2.9]) {
      const [sx, sy] = projectPoint(3.2, -1.5, 16, yaw);
      const [wx, wy] = projectPoint(3.2, -1.5, 16, yaw + 2 * Math.PI);
      expect(wx).toBeCloseTo(sx, 8);
      expect(wy).toBeCloseTo(sy, 8);
    }
  });

  it('reproduces the original fixed-tilt projection at the default pitch', () => {
    // Legacy formula: sx = (rx−ry)·10, sy = (rx+ry)·5.4 − z. The default
    // pitch must keep the landscape pixel-identical to the pre-pitch look.
    for (const [x, y, z, yaw] of [
      [3.2, -1.5, 16, 0.4],
      [-12, 2.8, 32, -0.14],
      [26, 3.9, 0, 2.7],
    ]) {
      const cos = Math.cos(yaw);
      const sin = Math.sin(yaw);
      const rx = x * cos - y * sin;
      const ry = x * sin + y * cos;
      const [sx, sy] = projectPoint(x, y, z, yaw, INITIAL_VIEW.pitch);
      expect(sx).toBeCloseTo((rx - ry) * 10, 8);
      expect(sy).toBeCloseTo((rx + ry) * 5.4 - z, 8);
    }
  });

  it('unprojectGround inverts the ground-plane projection at any angle', () => {
    for (const yaw of [0, 0.7, -1.3, 2.9]) {
      for (const pitch of [0.3, INITIAL_VIEW.pitch, 1.15]) {
        const [sx, sy] = projectPoint(7.5, -2.25, 0, yaw, pitch);
        const [x, y] = unprojectGround(sx, sy, yaw, pitch);
        expect(x).toBeCloseTo(7.5, 8);
        expect(y).toBeCloseTo(-2.25, 8);
      }
    }
  });

  it('tilting toward top-down grows the ground and shortens the bars', () => {
    const flat = 0.35;
    const steep = 1.1;
    // Ground point: vertical spread grows with pitch.
    expect(Math.abs(projectPoint(0, 3, 0, 0, steep)[1])).toBeGreaterThan(
      Math.abs(projectPoint(0, 3, 0, 0, flat)[1]),
    );
    // Bar height (z-only offset at the origin) shrinks with pitch.
    const heightAt = (p: number) => Math.abs(projectPoint(0, 0, 32, 0, p)[1]);
    expect(heightAt(steep)).toBeLessThan(heightAt(flat));
  });
});

describe('sceneBounds', () => {
  it('contains every projected corner of every day at any yaw, per pitch', () => {
    const cols = Math.max(1, ...DAYS.map((d) => d.col)) + 1;
    const cx = cols / 2;
    const cy = 3.5;
    // Pitch range extremes + default; a spread of yaws around the full circle.
    for (const pitch of [0.3, INITIAL_VIEW.pitch, 1.15]) {
      const b = sceneBounds(DAYS, pitch);
      for (const yaw of [0, 0.7, 1.6, Math.PI, 4.5, 5.9, -2.3, 9.1]) {
        for (const d of DAYS) {
          for (const [gx, gy] of [
            [d.col, d.row],
            [d.col + TILE, d.row],
            [d.col + TILE, d.row + TILE],
            [d.col, d.row + TILE],
          ]) {
            for (const z of [0, d.level * ZUNIT]) {
              const [sx, sy] = projectPoint(gx - cx, gy - cy, z, yaw, pitch);
              expect(sx).toBeGreaterThanOrEqual(b.minX);
              expect(sx).toBeLessThanOrEqual(b.maxX);
              expect(sy).toBeGreaterThanOrEqual(b.minY);
              expect(sy).toBeLessThanOrEqual(b.maxY);
            }
          }
        }
      }
    }
  });
});

describe('zoomView', () => {
  const box: [number, number] = [0, -19];
  const view = { yaw: 0.6, pitch: 0.8, zoom: 2, tx: 3, ty: -1 };

  it('keeps the grid point under the cursor fixed while zooming', () => {
    const anchor: [number, number] = [120, -60];
    // The grid point currently under the anchor.
    const [gx, gy] = unprojectGround(anchor[0], anchor[1], view.yaw, view.pitch);
    const g: [number, number] = [gx + view.tx, gy + view.ty];

    const next = zoomView(view, 1.5, anchor, box);
    expect(next.zoom).toBeCloseTo(3, 8);
    // Where that same grid point projects now…
    const [sx, sy] = projectPoint(g[0] - next.tx, g[1] - next.ty, 0, view.yaw, view.pitch);
    // …must be the anchor's viewBox coordinate after the viewport rescaled
    // around its fixed center (same screen pixel).
    const shrink = view.zoom / next.zoom;
    expect(sx).toBeCloseTo((anchor[0] - box[0]) * shrink + box[0], 6);
    expect(sy).toBeCloseTo((anchor[1] - box[1]) * shrink + box[1], 6);
  });

  it('clamps at the top and snaps the target home at zoom 1', () => {
    expect(zoomView({ ...view, zoom: 8 }, 2, [0, 0], box).zoom).toBe(8);
    const out = zoomView(view, 0.1, [50, 50], box);
    expect(out.zoom).toBe(1);
    expect(out.tx).toBe(0);
    expect(out.ty).toBe(0);
    // No-op factor at the clamp returns the same view object.
    expect(zoomView(out, 0.5, [50, 50], box)).toBe(out);
  });
});
