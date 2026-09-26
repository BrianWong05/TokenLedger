// PROTOTYPE (panel-metric switch, branch prototype/panel-chart-metric-switch).
// Throwaway: four designs for switching the Menu Bar Extra chart between
// tokens and Cost, on the real panel with fake ports. Dev server only:
//   ?prototype=panel-metric&variant=A   (← → or the bar to flip)
import { useState } from 'react';
import TrayPanel, { type ChartMetric, type MetricPrototype } from './TrayPanel';
import { makeFakeLedger } from '../overview/ledger.fake';
import { makeFakeSettings } from '../settings/settings.fake';
import { seriesPoint } from '../overview/seriesPoint';
import { PrototypeSwitcher } from '../lib/PrototypeSwitcher';
import type { BreakdownRow, Summary } from '../types';
import type { LimitsPort } from '../limits/limits';
import './panelMetric.prototype.css';

const VARIANTS = [
  { key: 'A', name: 'Second toggle beside Columns/Line' },
  { key: 'B', name: 'Headline figures pick the chart' },
  { key: 'C', name: 'Tabs heading the chart' },
  { key: 'D', name: 'Both at once, no switch' },
];

const p2 = (n: number) => String(n).padStart(2, '0');
const iso = (back: number) => {
  const d = new Date();
  d.setDate(d.getDate() - back);
  return `${d.getFullYear()}-${p2(d.getMonth() + 1)}-${p2(d.getDate())}`;
};

// [hour, source, tokens, cost]. Today's 14:00 is the costliest hour (Opus
// output), 17:00 the busiest (cache reads), so the switch moves the peak.
const TODAY: [number, string, number, number][] = [
  [0, 'claude', 1_800_000, 1.2], [1, 'claude', 900_000, 0.6], [2, 'claude', 1_100_000, 0.8],
  [8, 'claude', 400_000, 0.3], [9, 'claude', 2_600_000, 2.1], [10, 'claude', 3_900_000, 2.9],
  [11, 'claude', 4_000_000, 2.4], [11, 'grok', 1_200_000, 1.0], [12, 'claude', 2_200_000, 1.6],
  [13, 'claude', 4_800_000, 3.1], [14, 'claude', 4_100_000, 9.8], [15, 'claude', 6_900_000, 4.1],
  [15, 'codex', 600_000, 0.1], [16, 'claude', 9_900_000, 4.9], [17, 'claude', 14_600_000, 3.2],
  [18, 'claude', 8_200_000, 3.6], [19, 'claude', 3_000_000, 1.9], [20, 'grok', 5_600_000, 2.7],
  [21, 'claude', 1_200_000, 0.8], [22, 'claude', 2_400_000, 1.5], [23, 'claude', 700_000, 0.5],
];
// Yesterday: 10:00 is the costliest, 20:00 the busiest.
const YESTERDAY: [number, string, number, number][] = [
  [9, 'claude', 3_100_000, 2.2], [10, 'claude', 3_400_000, 11.6], [11, 'claude', 5_200_000, 3.4],
  [13, 'claude', 6_100_000, 4.0], [14, 'grok', 2_300_000, 1.8], [15, 'claude', 7_700_000, 4.4],
  [16, 'claude', 9_000_000, 5.1], [19, 'claude', 11_800_000, 3.9], [20, 'claude', 16_900_000, 4.2],
  [21, 'claude', 6_400_000, 2.8], [22, 'codex', 900_000, 0.2],
];
const nowHour = new Date().getHours();
const hourPoints = [
  ...TODAY.filter(([h]) => h <= nowHour).map(([h, source, totalTokens, cost]) =>
    seriesPoint({ bucket: `${iso(0)} ${p2(h)}:00`, source, totalTokens, cost }),
  ),
  ...YESTERDAY.map(([h, source, totalTokens, cost]) =>
    seriesPoint({ bucket: `${iso(1)} ${p2(h)}:00`, source, totalTokens, cost }),
  ),
];
// 30 days: tokens and Cost on unrelated rhythms, with a quiet day each week.
const dayPoints = Array.from({ length: 30 }, (_, back) => back)
  .filter((back) => back % 7 !== 5)
  .map((back) =>
    seriesPoint({
      bucket: iso(back),
      source: 'claude',
      totalTokens: 30_000_000 + ((back * 7919) % 17) * 9_000_000,
      cost: 15 + ((back * 104_729) % 19) * 6.5,
    }),
  );

const today = TODAY.filter(([h]) => h <= nowHour);
const totalTokens = today.reduce((a, [, , t]) => a + t, 0);
const totalCost = today.reduce((a, [, , , c]) => a + c, 0);
const summary: Summary = {
  inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0,
  totalTokens, requests: 343, cost: totalCost, hasUnpriced: false, unattributedTokens: 0,
  unpricedModels: [], cacheEstimatedModels: [], cacheHitRate: 0.964, convs: 0,
};
const bySource = (s: string) =>
  today.filter(([, src]) => src === s).reduce((a, [, , t, c]) => [a[0] + t, a[1] + c], [0, 0]);
const row = (key: string, source: string | null, [t, c]: number[]): BreakdownRow => ({
  key, source, inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0,
  totalTokens: t, requests: 0, cost: c, reasoningTokens: null, convs: 0,
  cacheEstimated: false, hasUnpriced: false, unattributedTokens: 0,
});

const store = new Map<string, string>();
const limits: LimitsPort = {
  list: () => Promise.resolve([]),
  checkLive: () => Promise.resolve(),
  scan: () => Promise.resolve(),
  onLimitsChanged: () => () => {},
  read: (k) => store.get(k) ?? null,
  write: (k, v) => void store.set(k, v),
};
// Module-level so the panel's effects see one stable set of ports.
const PORTS = {
  ledger: makeFakeLedger({
    summary,
    hourPoints,
    dayPoints,
    lastScan: Math.floor(Date.now() / 1000) - 20,
    sourceRows: [
      row('claude', null, bySource('claude')),
      row('grok', null, bySource('grok')),
      row('codex', null, bySource('codex')),
    ],
    modelRows: [
      row('claude-opus-5-5', 'claude', bySource('claude')),
      row('grok-4.7', 'grok', bySource('grok')),
      row('gpt-6-luna', 'codex', bySource('codex')),
    ],
  }),
  settings: makeFakeSettings(),
  limits,
};

document.documentElement.style.background = '#3a3d44';

type Variant = MetricPrototype['variant'];

export default function PanelMetricPrototype() {
  const [variant, setVariant] = useState<Variant>(() => {
    const v = new URLSearchParams(window.location.search).get('variant');
    return (VARIANTS.some((x) => x.key === v) ? v : 'A') as Variant;
  });
  const [metric, setMetric] = useState<ChartMetric>('tokens');
  const choose = (key: string) => {
    const u = new URL(window.location.href);
    u.searchParams.set('variant', key);
    window.history.replaceState(null, '', u);
    setVariant(key as Variant);
  };
  return (
    <>
      <TrayPanel ports={PORTS} platform="windows" prototype={{ variant, metric, onMetric: setMetric }} />
      <PrototypeSwitcher
        variants={VARIANTS}
        current={variant}
        onChange={choose}
        status={variant === 'D' ? 'chart: tokens + $ line' : `chart: ${metric}`}
      />
    </>
  );
}
