/** @vitest-environment jsdom */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it } from 'vitest';
import {
  CHART_DARK,
  CHART_LIGHT,
  paletteFor,
  useChartColors,
  type ChartColors,
} from './chartColors';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const roots: Root[] = [];

afterEach(() => {
  for (const r of roots.splice(0)) act(() => r.unmount());
  document.body.replaceChildren();
  document.documentElement.removeAttribute('data-theme');
});

async function flush() {
  await act(async () => {
    await Promise.resolve();
  });
}

describe('paletteFor', () => {
  it('returns the light palette only for data-theme="light" (dark is the default)', () => {
    expect(paletteFor('light')).toBe(CHART_LIGHT);
    expect(paletteFor('dark')).toBe(CHART_DARK);
    expect(paletteFor(null)).toBe(CHART_DARK);
  });

  it('carries the ds-colors.css hexes for both themes', () => {
    expect(CHART_DARK.chart1).toBe('#7C6BF2');
    expect(CHART_DARK.grid).toBe('#222228');
    expect(CHART_LIGHT.chart1).toBe('#6555D6');
    expect(CHART_LIGHT.grid).toBe('#E8E8EC');
  });
});

describe('useChartColors', () => {
  it('reflects the root data-theme and re-renders when it flips', async () => {
    document.documentElement.setAttribute('data-theme', 'dark');
    let latest: ChartColors | null = null;
    function Probe() {
      latest = useChartColors();
      return null;
    }

    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    roots.push(root);
    await act(async () => root.render(<Probe />));

    expect(latest).toEqual(CHART_DARK);

    // Flip to light: the MutationObserver on the root fires and the hook re-renders.
    await act(async () => {
      document.documentElement.setAttribute('data-theme', 'light');
    });
    await flush();
    expect(latest).toEqual(CHART_LIGHT);
  });
});
