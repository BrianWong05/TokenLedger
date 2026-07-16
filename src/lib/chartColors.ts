import { useEffect, useState } from 'react';

// DS chart palette. WebKit (Tauri's macOS webview) does not resolve var() in SVG
// presentation attributes, so chart SVGs can't read --chart-1..5/--chart-grid from
// CSS — they import these constants instead. Values mirror ds-colors.css per theme;
// keep in sync if the DS palette changes.
export interface ChartColors {
  chart1: string;
  chart2: string;
  chart3: string;
  chart4: string;
  chart5: string;
  grid: string;
}

export const CHART_DARK: ChartColors = {
  chart1: '#7C6BF2', chart2: '#45C4D6', chart3: '#4EA8E8',
  chart4: '#4CBE8A', chart5: '#C77BE0', grid: '#222228',
};

export const CHART_LIGHT: ChartColors = {
  chart1: '#6555D6', chart2: '#1FA3B5', chart3: '#2B7FC7',
  chart4: '#1E9E62', chart5: '#A94FC7', grid: '#E8E8EC',
};

// The resolved theme lives in the root's data-theme attribute (set by applyTheme);
// anything other than 'light' is dark (the default).
export function paletteFor(theme: string | null): ChartColors {
  return theme === 'light' ? CHART_LIGHT : CHART_DARK;
}

// Returns the palette for the currently resolved theme and re-renders when the root
// data-theme flips (theme toggle or live OS change). A MutationObserver on the root
// is the simplest seam that works under jsdom — no wiring into theme.ts needed.
export function useChartColors(): ChartColors {
  const root = document.documentElement;
  const [theme, setTheme] = useState<string | null>(() => root.getAttribute('data-theme'));
  useEffect(() => {
    const obs = new MutationObserver(() => setTheme(root.getAttribute('data-theme')));
    obs.observe(root, { attributes: true, attributeFilter: ['data-theme'] });
    setTheme(root.getAttribute('data-theme')); // catch a flip between render and effect
    return () => obs.disconnect();
  }, [root]);
  return paletteFor(theme);
}
