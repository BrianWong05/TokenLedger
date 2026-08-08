// PROTOTYPE — throwaway demo entry (demo-trend.html). Mounts the Trend Enlarge
// directly over fake ports to compare three placements for a window-scoped
// model breakdown (?variant=A|B|C, floating switcher, ←/→ cycle). Delete after
// the round; only the winning placement gets folded into TrendModal properly.
import { useEffect, useRef, useState, type CSSProperties } from 'react';
import ReactDOM from 'react-dom/client';
import TrendModal from './overview/TrendModal';
import { makeFakeLedger } from './overview/ledger.fake';
import { makeFakeSettings } from './settings/settings.fake';
import { SettingsProvider } from './settings/SettingsContext';
import { isoOf } from './overview/data';
import type { Filters, SeriesPoint, Summary } from './types';
import './index.css';
import './overview/overview.css';

type Variant = 'A' | 'B' | 'C';
const VARIANTS: Variant[] = ['A', 'B', 'C'];
const VARIANT_NAME: Record<Variant, string> = {
  A: 'Section below inspector',
  B: 'Window at rest, bucket on hover',
  C: 'Replace bucket by-model rows',
};

// ---- seed data: ~30 days across two Sources, screenshot-like hourly today ----

function pt(over: Partial<SeriesPoint>): SeriesPoint {
  return {
    bucket: '2026-01-01', source: 'claude', byModel: {},
    unattributedTokens: 0, hasUnpriced: false,
    inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0,
    totalTokens: 0, reasoningTokens: null, cost: 0, requests: 1, convs: 1,
    ctxMessages: null, ctxSystem: null, ctxReasoning: null, ctxToolcalls: null,
    ctxAgents: null, ctxMcp: null, ctxSkills: null, ...over,
  };
}

// Split a bucket total into two Source points with fixed model weights —
// 8 models overall so the window breakdown exercises the top-6 + "2 more" fold.
function sourcePoints(bucket: string, total: number): SeriesPoint[] {
  const claude = Math.round(total * 0.92);
  const qoder = total - claude;
  const cm = {
    'claude-fable-5': Math.round(claude * 0.7),
    'claude-opus-5': Math.round(claude * 0.15),
    'claude-sonnet-5': Math.round(claude * 0.08),
    'claude-haiku-4-5': Math.round(claude * 0.05),
    'claude-opus-4-8': Math.round(claude * 0.02),
  };
  const qm = {
    'qoder-pro': Math.round(qoder * 0.6),
    'qoder-flash': Math.round(qoder * 0.3),
    'qoder-mini': Math.round(qoder * 0.1),
  };
  return [
    pt({ bucket, source: 'claude', totalTokens: Object.values(cm).reduce((a, b) => a + b, 0), byModel: cm }),
    pt({ bucket, source: 'qoder', totalTokens: Object.values(qm).reduce((a, b) => a + b, 0), byModel: qm }),
  ];
}

const today = new Date();
const todayIso = isoOf(today);
const isoBack = (n: number) => {
  const d = new Date(today);
  d.setDate(d.getDate() - n);
  return isoOf(d);
};

// Early-morning-heavy hourly curve, like the screenshot.
const HOURLY: [number, number][] = [
  [0, 38e6], [1, 35e6], [2, 40e6], [3, 51.4e6], [4, 9e6], [5, 4.5e6],
  [13, 3e6], [14, 6e6], [15, 4e6],
];
const hourPoints: SeriesPoint[] = HOURLY.flatMap(([h, total]) =>
  sourcePoints(`${todayIso} ${String(h).padStart(2, '0')}:00`, Math.round(total)),
);

const dayPoints: SeriesPoint[] = [];
for (let i = 29; i >= 1; i--) {
  const total = Math.round(40e6 + 110e6 * Math.abs(Math.sin(i * 1.3)));
  dayPoints.push(...sourcePoints(isoBack(i), total));
}
// Today's daily point mirrors the hourly seed: a Day window's footer total (and
// the window breakdown's % base) sums the DAILY series, while its bars sum the
// hourly one — real data keeps those consistent, so the seed must too.
dayPoints.push(...sourcePoints(todayIso, HOURLY.reduce((a, [, v]) => a + v, 0)));

const windowSummary: Summary = {
  inputTokens: 12e6, outputTokens: 4e6, cacheReadTokens: 160e6, cacheWriteTokens: 14.7e6,
  totalTokens: 190.7e6, requests: 4200, cost: 233.26, hasUnpriced: false,
  unattributedTokens: 0, unpricedModels: [], cacheEstimatedModels: [], cacheHitRate: 0.84, convs: 31,
};

const ledger = makeFakeLedger({ dayPoints, hourPoints, summary: windowSummary });
// A bucket-scoped Summary (span ≤ 1h) gets a smaller Cost than the window's,
// so the two Est. cost read-outs are visibly different figures in the demo.
const cannedSummary = ledger.summary;
ledger.summary = (filters: Filters) => {
  const span = (filters.endTs ?? 0) - (filters.startTs ?? 0);
  if (span > 0 && span <= 3700) return Promise.resolve({ ...windowSummary, totalTokens: 51.4e6, cost: 76.03 });
  return cannedSummary(filters);
};

// ---- the demo shell: modal always open + floating variant switcher ----

function Switcher({ variant, onPick }: { variant: Variant; onPick: (v: Variant) => void }) {
  const cycle = (dir: 1 | -1) =>
    onPick(VARIANTS[(VARIANTS.indexOf(variant) + dir + VARIANTS.length) % VARIANTS.length]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = document.activeElement;
      if (el instanceof HTMLElement && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable)) return;
      if (e.key === 'ArrowLeft') cycle(-1);
      if (e.key === 'ArrowRight') cycle(1);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });
  const btn: CSSProperties = {
    background: 'none', border: 'none', color: 'inherit', font: 'inherit',
    cursor: 'pointer', padding: '4px 10px',
  };
  return (
    <div
      style={{
        position: 'fixed', bottom: 18, left: '50%', transform: 'translateX(-50%)',
        zIndex: 99999, display: 'flex', alignItems: 'center', gap: 4,
        background: '#111', color: '#fff', border: '1px solid #444',
        borderRadius: 999, padding: '4px 8px', boxShadow: '0 4px 16px rgba(0,0,0,.5)',
        fontSize: 13, fontFamily: 'ui-monospace, monospace', whiteSpace: 'nowrap',
      }}
    >
      <button style={btn} onClick={() => cycle(-1)} aria-label="previous variant">←</button>
      <span>{variant} — {VARIANT_NAME[variant]}</span>
      <button style={btn} onClick={() => cycle(1)} aria-label="next variant">→</button>
    </div>
  );
}

function Demo() {
  const [variant, setVariant] = useState<Variant>(() => {
    const v = new URLSearchParams(window.location.search).get('variant');
    return VARIANTS.includes(v as Variant) ? (v as Variant) : 'A';
  });
  const pick = (v: Variant) => {
    setVariant(v);
    const url = new URL(window.location.href);
    url.searchParams.set('variant', v);
    window.history.replaceState(null, '', url);
  };
  const returnFocus = useRef<HTMLElement | null>(null);
  return (
    <SettingsProvider port={makeFakeSettings({ theme: 'dark', firstRunDone: true })}>
      {variant === 'A' && (
        // Variant A stacks two row lists; let the aside scroll as one column
        // instead of giving each list its own nested scrollbar.
        <style>{'.tt-trend-insp-rows { flex: none; overflow-y: visible; }'}</style>
      )}
      <TrendModal
        key={variant} // remount per variant so dialog-local state resets
        allPoints={dayPoints}
        firstIso={isoBack(29)}
        lastIso={todayIso}
        initialRange="day"
        initialCustomFrom=""
        initialCustomTo=""
        ledger={ledger}
        exporter={{ saveCsv: () => Promise.resolve(true) }}
        returnFocusRef={returnFocus}
        onClose={() => {}} // the demo IS the modal — nothing to close to
        prototypeVariant={variant}
      />
      <Switcher variant={variant} onPick={pick} />
    </SettingsProvider>
  );
}

ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(<Demo />);
