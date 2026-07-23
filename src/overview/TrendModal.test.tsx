/** @vitest-environment jsdom */

// Usage-trend Enlarge (design 1b, shell slice) at the agreed primary seam: the
// real Overview mounted over fake ports. Covers the dialog's open path (title,
// subtitle, window footer figures, stacked chart), the close paths (Escape,
// backdrop, ✕) with focus restore, and the scroll lock — external behaviour
// only, no component internals.
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import Overview from './Overview';
import { systemClock } from './overviewStore';
import { makeFakeLedger } from './ledger.fake';
import { makeFakePricing } from '../pricing/pricing.fake';
import { makeFakeSettings } from '../settings/settings.fake';
import { SettingsProvider } from '../settings/SettingsContext';
import { isoOf } from './data';
import type { Filters, SeriesPoint, Summary } from '../types';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

beforeEach(() => {
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
});

function pt(over: Partial<SeriesPoint>): SeriesPoint {
  return {
    bucket: '2026-07-16', source: 'claude', byModel: {},
    inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0,
    totalTokens: 0, reasoningTokens: null, cost: 0, requests: 1, convs: 1,
    ctxMessages: null, ctxSystem: null, ctxReasoning: null, ctxToolcalls: null,
    ctxAgents: null, ctxMcp: null, ctxSkills: null, ...over,
  };
}

const summary: Summary = {
  inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0,
  totalTokens: 700, requests: 3, cost: 1.5, hasUnpriced: false,
  unpricedModels: [], cacheEstimatedModels: [], cacheHitRate: 0,
};

function daysAgo(n: number): string {
  const d = new Date();
  d.setDate(d.getDate() - n);
  return isoOf(d);
}

// Fake file-save port: records (filename, contents) instead of opening a dialog.
function makeFakeExporter() {
  const calls: [string, string][] = [];
  return { calls, saveCsv: (filename: string, contents: string) => (calls.push([filename, contents]), Promise.resolve(true)) };
}

const mountedRoots: Root[] = [];

async function settle(times = 4) {
  for (let i = 0; i < times; i++) {
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });
  }
}

async function mount(
  over: Partial<Summary> = {},
  seedDayPoints?: SeriesPoint[],
  seedHourPoints?: SeriesPoint[],
): Promise<{
  container: HTMLElement;
  ledger: ReturnType<typeof makeFakeLedger>;
  exporter: ReturnType<typeof makeFakeExporter>;
}> {
  const ledger = makeFakeLedger({
    dayPoints: seedDayPoints ?? [
      pt({ bucket: daysAgo(2), source: 'claude', totalTokens: 400, byModel: { 'claude-opus-4-8': 400 } }),
      pt({ bucket: daysAgo(1), source: 'claude', totalTokens: 100, byModel: { 'claude-opus-4-8': 100 } }),
      pt({ bucket: daysAgo(1), source: 'codex', totalTokens: 200, byModel: { 'gpt-5.5-codex': 200 } }),
    ],
    hourPoints: seedHourPoints ?? [],
    summary: { ...summary, ...over },
  });
  const exporter = makeFakeExporter();
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  mountedRoots.push(root);
  await act(async () => {
    root.render(
      <SettingsProvider port={makeFakeSettings()}>
        <Overview ports={{ ledger, clock: systemClock, pricing: makeFakePricing(), export: exporter }} />
      </SettingsProvider>,
    );
  });
  await settle();
  return { container, ledger, exporter };
}

// The dialog's own window selector lives in its header; the page's range
// control lives in the toolbar. Scope each so they never collide.
const modalRangeButton = (label: string) =>
  Array.from(dialog()!.querySelectorAll<HTMLButtonElement>('.tt-seg button')).find(
    (b) => b.textContent === label,
  )!;
const pageActiveRange = (c: HTMLElement) =>
  c.querySelector<HTMLElement>('.tt-toolbar .tt-seg .active')!.textContent;
const footText = () => dialog()!.querySelector('.tt-trend-modal-foot')!.textContent!;
const costText = () => dialog()!.querySelector('.tt-trend-insp-cost')!.textContent!;
const exportBtn = () => dialog()!.querySelector<HTMLButtonElement>('.tt-trend-insp-export')!;

// The modal fires two kinds of `summary` fetch that share the port: window
// fetches (footer Cost — never carry endTs) and per-bucket fetches (inspector
// Cost — always bounded, so endTs is set). Target held calls by that shape
// instead of a fragile positional index.
type FL = ReturnType<typeof makeFakeLedger>;
const heldIdx = (ledger: FL, pred: (f: Filters) => boolean) =>
  ledger.held('summary').findIndex((d) => pred(d.args[0] as Filters));
const isWindow = (f: Filters) => f.endTs === undefined;
const isBucket = (f: Filters) => f.endTs !== undefined;
const inspText = () => dialog()!.querySelector('.tt-trend-insp')!.textContent!;
const barGroups = () => Array.from(dialog()!.querySelectorAll('svg g[opacity]'));
const hitRects = () => Array.from(dialog()!.querySelectorAll<SVGRectElement>('svg rect[fill="transparent"]'));
const outlineRect = () => dialog()!.querySelector<SVGRectElement>('svg rect[stroke]');
const hoverBar = (i: number) =>
  act(async () => hitRects()[i].dispatchEvent(new MouseEvent('mouseover', { bubbles: true })));

// A window with a clear peak (daysAgo 1 = 800) that carries eight models across
// two sources, so the inspector shows a top-6 split plus a "2 more models" row.
// The window zero-fills up to today, so it holds three buckets: [daysAgo 2,
// daysAgo 1, today] → the peak sits at index 1.
const inspectorSeed = () => [
  pt({ bucket: daysAgo(2), source: 'claude', totalTokens: 200, byModel: { m1: 200 } }),
  pt({
    bucket: daysAgo(1), source: 'claude', totalTokens: 680,
    byModel: { m1: 300, m2: 150, m3: 100, m4: 60, m5: 40, m6: 30 },
  }),
  pt({ bucket: daysAgo(1), source: 'codex', totalTokens: 120, byModel: { c1: 80, c2: 40 } }),
];

// Local-midnight epoch seconds for the day N days ago, and the top of hour H
// today — the exact bounds bucketFilters() produces.
const dayTs = (n: number) => {
  const d = new Date();
  d.setDate(d.getDate() - n);
  return Math.floor(new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime() / 1000);
};
const hourTs = (hh: number) => {
  const d = new Date();
  return Math.floor(new Date(d.getFullYear(), d.getMonth(), d.getDate(), hh).getTime() / 1000);
};

afterEach(() => {
  for (const root of mountedRoots.splice(0)) act(() => root.unmount());
  document.body.replaceChildren();
  document.documentElement.style.overflow = '';
  document.body.style.overflow = '';
});

// Both the Activity and Trend cards use the shared .tt-heat-enlarge control;
// the Trend one is the card that is NOT the .heat card.
const trendEnlarge = (c: HTMLElement) =>
  Array.from(c.querySelectorAll<HTMLButtonElement>('.tt-heat-enlarge')).find(
    (b) => !b.closest('.tt-card')?.classList.contains('heat'),
  )!;
const dialog = () => document.querySelector<HTMLElement>('.tt-trend-modal');

async function open(c: HTMLElement): Promise<HTMLElement> {
  await act(async () => trendEnlarge(c).click());
  return dialog()!;
}

describe('Usage-trend Enlarge', () => {
  it('opens a dialog with the trend title, window subtitle, and footer figures', async () => {
    const { container: c } = await mount();
    const modal = await open(c);

    expect(modal.getAttribute('role')).toBe('dialog');
    expect(modal.getAttribute('aria-modal')).toBe('true');
    expect(modal.querySelector('#tt-trend-modal-title')!.textContent).toBe('Usage over time');
    // Subtitle names the window (default range is "All time").
    expect(modal.querySelector('.tt-trend-modal-sub')!.textContent).toContain('All time');

    // Footer: window total (700 over the two seeded days), avg per bucket, and
    // the Cost from the page Summary (seeded points carry zero cost).
    const foot = modal.querySelector('.tt-trend-modal-foot')!.textContent!;
    expect(foot).toContain('700');
    expect(foot).toContain('$1.50');
    // Legend limited to Sources active in the window.
    expect(foot).toContain('Claude');
    expect(foot).toContain('Codex');
    expect(foot).not.toContain('Gemini');
  });

  it('renders the stacked-by-Source chart full-width', async () => {
    const { container: c } = await mount();
    const modal = await open(c);
    const svg = modal.querySelector<SVGSVGElement>('.tt-trend-modal-chart svg')!;
    expect(svg).toBeTruthy();
    // Two seeded days → at least one stacked bar rect per active model.
    expect(svg.querySelectorAll('rect').length).toBeGreaterThan(0);
  });

  it('shows the window Cost with its Partial-Cost marker when the Summary is unpriced', async () => {
    const { container: c } = await mount({ cost: 9.99, hasUnpriced: true, unpricedModels: ['self-hosted-x'] });
    const modal = await open(c);
    const foot = modal.querySelector('.tt-trend-modal-foot')!.textContent!;
    expect(foot).toContain('≥ $9.99');
    expect(foot).toContain('1 unpriced');
  });

  it('locks page scroll while open and unlocks on close', async () => {
    const { container: c } = await mount();
    await open(c);
    expect(document.documentElement.style.overflow).toBe('hidden');
    expect(document.body.style.overflow).toBe('hidden');

    await act(async () => {
      document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    });
    expect(dialog()).toBeNull();
    expect(document.documentElement.style.overflow).toBe('');
    expect(document.body.style.overflow).toBe('');
  });

  it('closes on Escape and returns focus to the Enlarge control', async () => {
    const { container: c } = await mount();
    await open(c);
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    });
    expect(dialog()).toBeNull();
    expect(document.activeElement).toBe(trendEnlarge(c));
  });

  it('closes on the ✕ button and on a backdrop click, but not on clicks inside', async () => {
    const { container: c } = await mount();
    const modal = await open(c);

    // A click inside the panel must NOT close it.
    await act(async () => modal.querySelector<HTMLElement>('.tt-trend-modal-foot')!.click());
    expect(dialog()).not.toBeNull();

    await act(async () => modal.querySelector<HTMLButtonElement>('.tt-trend-modal-close')!.click());
    expect(dialog()).toBeNull();

    await open(c);
    const backdrop = document.querySelector<HTMLElement>('.tt-trend-modal-backdrop')!;
    await act(async () => {
      backdrop.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
      backdrop.click();
    });
    expect(dialog()).toBeNull();
  });

  it('does not dismiss when a press starts inside and releases over the backdrop', async () => {
    const { container: c } = await mount();
    const modal = await open(c);
    const chart = modal.querySelector<HTMLElement>('.tt-trend-modal-chart')!;

    // mousedown inside, click lands on the backdrop (common ancestor): must
    // not close, since the press did not begin on the backdrop.
    await act(async () => {
      chart.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
      document.querySelector<HTMLElement>('.tt-trend-modal-backdrop')!.click();
    });
    expect(dialog()).not.toBeNull();
  });

  it('has all five presets in its own selector, opening on the page range', async () => {
    const { container: c } = await mount();
    await open(c);
    const labels = Array.from(dialog()!.querySelectorAll('.tt-seg button')).map((b) => b.textContent);
    expect(labels).toEqual(['Day', 'Week', 'Month', 'Total', 'Custom']);
    // Page default range is Total → the dialog opens on Total.
    expect(modalRangeButton('Total').className).toContain('active');
  });

  it('reveals a Custom from/to date row when Custom is picked', async () => {
    const { container: c } = await mount();
    await open(c);
    expect(dialog()!.querySelector('.tt-trend-modal-custom')).toBeNull();
    await act(async () => modalRangeButton('Custom').click());
    const dates = dialog()!.querySelectorAll<HTMLInputElement>('.tt-trend-modal-custom input[type="date"]');
    expect(dates).toHaveLength(2);
  });

  it('changes its window without moving the Overview, and reopens on the page range', async () => {
    const { container: c } = await mount();
    await open(c);
    expect(pageActiveRange(c)).toBe('Total');

    await act(async () => modalRangeButton('Week').click());
    // The dialog followed; the page did not.
    expect(modalRangeButton('Week').className).toContain('active');
    expect(dialog()!.querySelector('.tt-trend-modal-sub')!.textContent).toContain('Last 7 days');
    expect(pageActiveRange(c)).toBe('Total');

    // Close and reopen: the local window was forgotten, so it re-seeds from the
    // page (still Total).
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    });
    await open(c);
    expect(modalRangeButton('Total').className).toContain('active');
  });

  it('fetches its own hourly series for a local Day window', async () => {
    const { container: c, ledger } = await mount(
      {},
      [pt({ bucket: daysAgo(1), source: 'claude', totalTokens: 300, byModel: { 'claude-opus-4-8': 300 } })],
      [pt({ bucket: `${daysAgo(0)} 09:00`, source: 'claude', totalTokens: 120, byModel: { 'claude-opus-4-8': 120 } })],
    );
    await open(c);
    const before = ledger.calls.series.filter((a) => a[1] === 'hour').length;

    await act(async () => modalRangeButton('Day').click());
    await settle(1);

    // The dialog fetched hourly itself, independent of the page (on Total).
    const hourlyCalls = ledger.calls.series.filter((a) => a[1] === 'hour');
    expect(hourlyCalls.length).toBe(before + 1);
    expect(footText()).toContain('avg / hour');
    expect(dialog()!.querySelector('.tt-trend-modal-sub')!.textContent).toContain('Today');
    expect(pageActiveRange(c)).toBe('Total');
  });

  it('recomputes the footer figures for the local window', async () => {
    // One bucket far outside the trailing week, one inside it.
    const { container: c } = await mount({}, [
      pt({ bucket: daysAgo(60), source: 'claude', totalTokens: 400, byModel: { 'claude-opus-4-8': 400 } }),
      pt({ bucket: daysAgo(1), source: 'claude', totalTokens: 300, byModel: { 'claude-opus-4-8': 300 } }),
    ]);
    await open(c);
    // Total window sees both buckets.
    expect(footText()).toContain('700');

    await act(async () => modalRangeButton('Week').click());
    // The trailing week sees only the recent bucket.
    expect(footText()).toContain('300');
    expect(footText()).not.toContain('700');
  });

  it('ignores a window Summary that resolves after a newer window change', async () => {
    const { container: c, ledger } = await mount();
    ledger.hold('summary');
    await open(c); // Total window fetch held (unbounded → no endTs)
    const totalIdx = heldIdx(ledger, (f) => isWindow(f) && f.startTs === undefined);
    expect(footText()).toContain('…');

    await act(async () => modalRangeButton('Week').click()); // Week window fetch held
    expect(footText()).toContain('…');
    const weekIdx = heldIdx(ledger, (f) => isWindow(f) && f.startTs !== undefined);

    // The abandoned Total fetch resolving must not land…
    await act(async () => ledger.resolveHeld('summary', totalIdx, { ...summary, cost: 111.11 }));
    await settle(1);
    expect(footText()).toContain('…');
    expect(footText()).not.toContain('$111.11');

    // …only the current window's own fetch may.
    await act(async () => ledger.resolveHeld('summary', weekIdx, { ...summary, cost: 2.22 }));
    await settle(1);
    expect(footText()).toContain('$2.22');
  });

  it('opens with the window peak selected and its top-6 model split', async () => {
    const { container: c } = await mount({}, inspectorSeed());
    await open(c);

    const insp = inspText();
    expect(insp).toContain('Selected day'); // per=day heading
    expect(insp).toContain('#1 / 3'); // peak is rank 1 of the 3 buckets
    expect(insp).toContain('800'); // the peak bucket's tokens
    // Peak is above the window average → green "up" delta.
    expect(dialog()!.querySelector('.tt-trend-insp-delta.up')).not.toBeNull();

    // Eight models → six rows plus one muted "2 more models" remainder row.
    const rows = dialog()!.querySelectorAll('.tt-trend-insp-row');
    expect(rows).toHaveLength(7);
    const more = dialog()!.querySelector('.tt-trend-insp-row.more')!;
    expect(more.textContent).toContain('2 more models');
    // Top-6 names shown; the folded pair is not.
    expect(insp).toContain('m4');
    expect(insp).toContain('m5');
    expect(insp).not.toContain('m6');
    expect(insp).not.toContain('c2');
  });

  it('moves the selection to a hovered bar, dimming the rest', async () => {
    const { container: c } = await mount({}, inspectorSeed());
    await open(c);
    const peakOutlineX = Number(outlineRect()!.getAttribute('x'));

    await hoverBar(0); // the earlier, smaller bucket (200)

    const insp = inspText();
    expect(insp).toContain('200');
    expect(insp).toContain('#2 / 3');
    // Below the window average → red "down" delta.
    expect(dialog()!.querySelector('.tt-trend-insp-delta.down')).not.toBeNull();

    // Selected bar solid, the others dimmed.
    const groups = barGroups();
    expect(groups[0].getAttribute('opacity')).toBe('1');
    expect(groups[1].getAttribute('opacity')).toBe('0.42');
    expect(groups[2].getAttribute('opacity')).toBe('0.42');
    // The selection outline moved left to the hovered (earlier) bar.
    expect(Number(outlineRect()!.getAttribute('x'))).toBeLessThan(peakOutlineX);
  });

  it('resets the selection to the new peak when the window changes', async () => {
    const { container: c } = await mount({}, inspectorSeed());
    await open(c);
    await hoverBar(0); // move to the small bucket
    expect(inspText()).toContain('#2 / 3');

    // Both seeded days are still inside the trailing week, so the peak is the
    // same bucket — but the selection must snap back to it, not stay put.
    await act(async () => modalRangeButton('Week').click());
    expect(inspText()).toContain('800');
    expect(inspText()).toContain('#1 /');
  });

  it('keeps the selection across a background refresh of the same window', async () => {
    const { container: c, ledger } = await mount({}, inspectorSeed());
    await open(c);
    await hoverBar(0);
    expect(inspText()).toContain('#2 / 3');

    // A store-driven re-render (prices rebuilt → reload) must not reset the selection.
    await act(async () => ledger.emitPricesRebuilt());
    await settle(1);
    expect(inspText()).toContain('200');
    expect(inspText()).toContain('#2 / 3');
  });

  it('renders no hover tooltip inside the dialog', async () => {
    const { container: c } = await mount({}, inspectorSeed());
    await open(c);
    expect(document.querySelector('.tt-tip')).toBeNull();
    await hoverBar(0);
    expect(document.querySelector('.tt-tip')).toBeNull();
  });

  it('fetches a Summary bounded to exactly the selected day', async () => {
    const { container: c, ledger } = await mount({}, inspectorSeed());
    await open(c); // peak (daysAgo 1) preselected → its own bounded fetch
    const bounded = ledger.calls.summary.map((a) => a[0] as Filters);
    // [midnight of the peak day, midnight of the next day).
    expect(bounded.some((f) => f.startTs === dayTs(1) && f.endTs === dayTs(0))).toBe(true);
  });

  it('fetches hourly bounds for a bucket in a local Day window', async () => {
    const { container: c, ledger } = await mount(
      {},
      [pt({ bucket: daysAgo(1), source: 'claude', totalTokens: 300, byModel: { m1: 300 } })],
      [
        pt({ bucket: `${daysAgo(0)} 09:00`, source: 'claude', totalTokens: 500, byModel: { m1: 500 } }),
        pt({ bucket: `${daysAgo(0)} 10:00`, source: 'claude', totalTokens: 100, byModel: { m1: 100 } }),
      ],
    );
    await open(c);
    await act(async () => modalRangeButton('Day').click());
    await settle(1);

    // Peak hour (09:00) selected → a Summary bounded to [09:00, 10:00).
    const bounded = ledger.calls.summary.map((a) => a[0] as Filters);
    expect(bounded.some((f) => f.startTs === hourTs(9) && f.endTs === hourTs(10))).toBe(true);
  });

  it('shows a placeholder in the cost row while the bucket Summary is in flight', async () => {
    const { container: c, ledger } = await mount({}, inspectorSeed());
    ledger.hold('summary');
    await open(c);
    expect(costText()).toContain('…');
    expect(costText()).not.toContain('$');
  });

  it('ignores a bucket Summary that resolves after a newer selection', async () => {
    const { container: c, ledger } = await mount({}, inspectorSeed());
    ledger.hold('summary');
    await open(c); // peak (daysAgo 1) bucket fetch held
    await hoverBar(0); // move to daysAgo 2 → its own bucket fetch held

    const peakIdx = heldIdx(ledger, (f) => isBucket(f) && f.startTs === dayTs(1));
    const pinnedIdx = heldIdx(ledger, (f) => isBucket(f) && f.startTs === dayTs(2));

    // The superseded peak fetch resolving must not land…
    await act(async () => ledger.resolveHeld('summary', peakIdx, { ...summary, cost: 999 }));
    await settle(1);
    expect(costText()).toContain('…');
    expect(costText()).not.toContain('$999');

    // …only the current selection's own fetch may.
    await act(async () => ledger.resolveHeld('summary', pinnedIdx, { ...summary, cost: 5.5 }));
    await settle(1);
    expect(costText()).toContain('$5.50');
  });

  it('renders the bucket cost with Partial-Cost and unpriced markers', async () => {
    const { container: c } = await mount(
      { cost: 9.99, hasUnpriced: true, unpricedModels: ['self-hosted-x'] },
      inspectorSeed(),
    );
    await open(c);
    expect(costText()).toContain('≥ $9.99');
    expect(costText()).toContain('1 unpriced');
  });

  it('never renders $0 for a fully unpriced bucket', async () => {
    const { container: c } = await mount(
      { cost: null, hasUnpriced: true, unpricedModels: ['x', 'y'] },
      inspectorSeed(),
    );
    await open(c);
    expect(costText()).toContain('unpriced');
    expect(costText()).not.toContain('$0');
  });

  it('exports the selected bucket as CSV through the save port', async () => {
    const { container: c, exporter } = await mount({}, inspectorSeed());
    await open(c); // peak (daysAgo 1) preselected

    await act(async () => exportBtn().click());

    expect(exporter.calls).toHaveLength(1);
    const [filename, contents] = exporter.calls[0];
    expect(filename).toBe(`usage-${daysAgo(1)}.csv`);
    expect(contents.split('\n')[0]).toBe('model,tool,tokens,share');
    // The CSV carries every model in the bucket — including the two the
    // inspector folds into its "2 more models" row.
    expect(contents).toContain('m6');
    expect(contents).toContain('c2');
  });

  it('disables Export for a bucket with no usage', async () => {
    const { container: c } = await mount({}, inspectorSeed());
    await open(c);
    expect(exportBtn().disabled).toBe(false); // peak has usage

    await hoverBar(2); // the zero-filled today bucket
    expect(exportBtn().disabled).toBe(true);
  });
});
