import { describe, it, expect } from 'vitest';
import { projectPoint, visibleWalls, sceneBounds, TILE, ZUNIT, INITIAL_YAW } from './Landscape3D';
import { seriesToDays } from './data';
import type { SeriesPoint } from '../types';

// The free-spin geometry at the pure seam: which walls face the camera at a
// given angle, and that the fixed camera's bounds hold at every angle. These
// re-derive the scene's centering independently of the component on purpose —
// the duplication triangulates the projection contract.

function pt(bucket: string, totalTokens: number): SeriesPoint {
  return {
    bucket, source: 'claude', byModel: {},
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
    expect(visibleWalls(INITIAL_YAW)).toEqual({ x: 'east', y: 'south' }); // default view
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
});

describe('sceneBounds', () => {
  it('contains every projected corner of every day at any yaw', () => {
    const b = sceneBounds(DAYS);
    const cols = Math.max(1, ...DAYS.map((d) => d.col)) + 1;
    const cx = cols / 2;
    const cy = 3.5;
    // A spread of angles around the full circle, including beyond ±π.
    for (const yaw of [0, 0.7, 1.6, Math.PI, 4.5, 5.9, -2.3, 9.1]) {
      for (const d of DAYS) {
        for (const [gx, gy] of [
          [d.col, d.row],
          [d.col + TILE, d.row],
          [d.col + TILE, d.row + TILE],
          [d.col, d.row + TILE],
        ]) {
          for (const z of [0, d.level * ZUNIT]) {
            const [sx, sy] = projectPoint(gx - cx, gy - cy, z, yaw);
            expect(sx).toBeGreaterThanOrEqual(b.minX);
            expect(sx).toBeLessThanOrEqual(b.maxX);
            expect(sy).toBeGreaterThanOrEqual(b.minY);
            expect(sy).toBeLessThanOrEqual(b.maxY);
          }
        }
      }
    }
  });
});
