/** @vitest-environment jsdom */

// Presentation-level coverage for the rebuilt Overview: the hero proportion bar
// widths, the source-card selection wiring (a card click drives the rest of the
// Overview), and the per-source scan footer. Mounts the real component over a
// fake Ledger + Settings/Pricing ports.
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import Overview from './Overview';
import { RANGES_8B } from './meta';
import { systemClock } from './overviewStore';
import { makeFakeLedger, type FakeLedger } from './ledger.fake';
import { makeFakePricing } from '../pricing/pricing.fake';
import { makeFakeSettings } from '../settings/settings.fake';
import { SettingsProvider } from '../settings/SettingsContext';
import type { BreakdownRow, SeriesPoint, Summary, ScanStatus } from '../types';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

// applyTheme() and TokenTotalHeadline read matchMedia, which jsdom lacks.
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
    unattributedTokens: 0, hasUnpriced: false,
    inputTokens: 10, outputTokens: 5, cacheReadTokens: 20, cacheWriteTokens: 3,
    totalTokens: 38, reasoningTokens: null, cost: 0, requests: 1, convs: 1,
    ctxMessages: null, ctxSystem: null, ctxReasoning: null, ctxToolcalls: null,
    ctxAgents: null, ctxMcp: null, ctxSkills: null, ...over,
  };
}

const summary: Summary = {
  inputTokens: 10, outputTokens: 5, cacheReadTokens: 20, cacheWriteTokens: 3,
  totalTokens: 400, requests: 2, cost: 1.5, hasUnpriced: false,
  unattributedTokens: 0, unpricedModels: [], cacheEstimatedModels: [], cacheHitRate: 0, convs: 0,
};

// Fake file-save port: records (filename, contents) instead of opening a
// dialog. The three results are the three things a save sheet can do —
// 'cancelled' resolves false (the user backed out; not a failure), 'fails'
// rejects (the write itself broke). Both record the call, so a test can assert
// what would have been written either way.
function makeFakeExporter(result: 'written' | 'cancelled' | 'fails' = 'written') {
  const calls: [string, string][] = [];
  return {
    calls,
    saveCsv: (filename: string, contents: string): Promise<boolean> => {
      calls.push([filename, contents]);
      if (result === 'fails') return Promise.reject(new Error('disk full'));
      return Promise.resolve(result === 'written');
    },
  };
}

type FakeExporter = ReturnType<typeof makeFakeExporter>;

const mountedRoots: Root[] = [];

async function settle(times = 4) {
  for (let i = 0; i < times; i++) {
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });
  }
}

// The general mount: every port faked, including the file-save one, so no test
// can reach the real `invoke`. mount(seed) below is this with the ledger seed
// spelled out, which is all most tests care about.
async function mountOverview(
  opts: { ledger?: FakeLedger; exporter?: FakeExporter } = {},
): Promise<{ container: HTMLElement; ledger: FakeLedger; exporter: FakeExporter }> {
  const ledger = opts.ledger ?? makeFakeLedger({ dayPoints: [pt({})], summary });
  const exporter = opts.exporter ?? makeFakeExporter();
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

async function mount(
  seed: Parameters<typeof makeFakeLedger>[0],
): Promise<{ container: HTMLElement; ledger: FakeLedger }> {
  return mountOverview({ ledger: makeFakeLedger(seed) });
}

async function click(el: Element) {
  await act(async () => {
    (el as HTMLElement).click();
  });
  await settle(2);
}

// The headline paints a compact figure ("1.2K"), so the exact total it stands
// for is only readable from its aria-label — "<exact> total tokens. <action>",
// optionally prefixed by the ≥ reading. Digits only, which drops the prefix
// and the thousands separators together.
function headlineTotal(container: HTMLElement): number {
  const label = container.querySelector('.tt-b8-total')!.getAttribute('aria-label')!;
  return Number(label.slice(0, label.indexOf(' total tokens.')).replace(/\D/g, ''));
}

// Like mount(), but on fake timers, advanced past the ≥1s Rescan spin hold so
// the initial refresh has fully settled (refreshing false) before assertions.
async function mountSettled(
  seed: Parameters<typeof makeFakeLedger>[0],
): Promise<{ container: HTMLElement; ledger: FakeLedger }> {
  vi.useFakeTimers();
  const ledger = makeFakeLedger(seed);
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  mountedRoots.push(root);
  await act(async () => {
    root.render(
      <SettingsProvider port={makeFakeSettings()}>
        <Overview ports={{ ledger, clock: systemClock, pricing: makeFakePricing(), export: makeFakeExporter() }} />
      </SettingsProvider>,
    );
  });
  await act(async () => {
    await vi.advanceTimersByTimeAsync(1_200);
  });
  return { container, ledger };
}

afterEach(() => {
  for (const root of mountedRoots.splice(0)) act(() => root.unmount());
  document.body.replaceChildren();
  // The headline's entrance gate lives in sessionStorage; leaving a test's key
  // behind would silently suppress the entrance for every later test.
  sessionStorage.clear();
  vi.useRealTimers();
});

describe('Overview presentation', () => {
  it('rolls the headline total when a period switch lands a different window', async () => {
    // The entrance already played this session, so the headline starts at rest
    // and any roll below is the in-place period-switch motion.
    sessionStorage.setItem('tokenledger.tokenTotalEntrancePlayed', 'true');
    const { container: c, ledger } = await mountSettled({
      dayPoints: [pt({ bucket: new Date().toISOString().slice(0, 10), totalTokens: 400 })],
      summary,
    });
    const headline = c.querySelector<HTMLElement>('.tt-b8-total')!;
    expect(headline.getAttribute('aria-busy')).toBeNull();

    // The Week window starts its roll as soon as the range changes, before its
    // different Summary lands.
    ledger.data.summary = { ...summary, totalTokens: 987_654_321 };
    const week = Array.from(c.querySelectorAll('button')).find((b) =>
      /week/i.test(b.textContent ?? ''),
    )!;
    await act(async () => {
      week.click();
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(50);
    });
    expect(headline.getAttribute('aria-busy')).toBe('true');
  });

  it('starts the range-switch roll before the new summary lands', async () => {
    sessionStorage.setItem('tokenledger.tokenTotalEntrancePlayed', 'true');
    const dayPoints = [
      pt({ bucket: '2026-08-14', totalTokens: 400 }),
      pt({ bucket: '2026-08-13', totalTokens: 600 }),
    ];
    const { container: c, ledger } = await mountSettled({
      dayPoints,
      summary,
    });
    const headline = c.querySelector<HTMLElement>('.tt-b8-total')!;
    const dayTab = Array.from(c.querySelectorAll('button')).find((b) =>
      /^day$/i.test(b.textContent?.trim() ?? ''),
    )!;
    const totalTab = Array.from(c.querySelectorAll('button')).find((b) =>
      /^total$/i.test(b.textContent?.trim() ?? ''),
    )!;

    await act(async () => {
      dayTab.click();
      await vi.advanceTimersByTimeAsync(1_400);
    });
    await act(async () => {
      ledger.emitPricesRebuilt();
      await vi.advanceTimersByTimeAsync(1);
    });
    ledger.hold('summary');
    await act(async () => {
      totalTab.click();
      await vi.advanceTimersByTimeAsync(1);
    });

    expect(headline.getAttribute('aria-label')).not.toContain('400 total tokens');
    expect(headline.getAttribute('aria-busy')).toBe('true');
  });

  it('sizes the hero proportion bar by each source token share', async () => {
    // claude 300 + codex 100 = 400 → 75% / 25%; the other five Sources are 0.
    const { container: c } = await mount({
      dayPoints: [
        pt({ source: 'claude', totalTokens: 300 }),
        pt({ source: 'codex', totalTokens: 100 }),
      ],
      summary,
    });

    const widths = Array.from(
      c.querySelectorAll<HTMLElement>('.tt-split div'),
      (d) => d.style.width,
    );
    // Only Sources with tokens render a segment; the five zero-token Sources are
    // omitted so their flex gaps don't wedge dead space between the visible ones.
    expect(widths).toEqual(['75%', '25%']);
  });

  it('renders lowercase pi after Google Antigravity with scan status and official mark', async () => {
    const piModel: BreakdownRow = {
      key: 'pi-response-model', source: 'pi', inputTokens: 135, outputTokens: 62,
      cacheReadTokens: 23, cacheWriteTokens: 19, totalTokens: 239, requests: 3,
      cost: 0.000805, reasoningTokens: 10, convs: 1, cacheEstimated: false,
      hasUnpriced: false, unattributedTokens: 0,
    };
    const { container: c } = await mount({
      dayPoints: [
        pt({ source: 'antigravity', totalTokens: 100 }),
        pt({ source: 'pi', totalTokens: 239, byModel: { 'pi-response-model': 239 } }),
      ],
      summary: { ...summary, totalTokens: 339, requests: 4 },
      modelRows: [piModel],
      projectRows: [{ ...piModel, key: '/Users/dev/projects/pi-demo', source: null }],
      scan: {
        scannedAt: 1_782_907_202,
        sources: [
          { source: 'antigravity', eventsInserted: 0, linesSkipped: 0, limitReadings: 0, artifactsUnreadable: 0, unreadableMaxMtime: null, error: null },
          { source: 'pi', eventsInserted: 3, linesSkipped: 2, limitReadings: 0, artifactsUnreadable: 0, unreadableMaxMtime: null, error: null },
        ],
      },
    });

    const cards = Array.from(c.querySelectorAll<HTMLButtonElement>('.tt-toolcards button'));
    expect(cards.map((card) => card.textContent?.trim())).toEqual([
      expect.stringContaining('Antigravity'),
      expect.stringContaining('pi'),
    ]);
    const piCard = cards[cards.length - 1];
    expect(piCard.querySelector('img')?.getAttribute('src')).toMatch(/^data:image\/svg\+xml/);
    expect(Array.from(c.querySelectorAll('.tt-legend .item'), (item) => item.textContent)).toContain('pi');
    expect(Array.from(c.querySelectorAll('.tt-sm-card .lbl'), (item) => item.textContent)).toContain('pi');
    const projectTab = Array.from(c.querySelectorAll<HTMLButtonElement>('.tt-tbl-tabs button'))
      .find((button) => button.textContent?.includes('Project Usage'))!;
    await act(async () => projectTab.click());
    expect(c.querySelector('.tt-tbl-row span')?.getAttribute('title')).toBe('/Users/dev/projects/pi-demo');
    expect(c.querySelector('.tt-scan-foot')?.textContent).toContain('pi: 3 in / 2 skipped');
  });

  it('drives the rest of the Overview when a source card is selected', async () => {
    const { container: c } = await mount({
      dayPoints: [
        pt({ source: 'claude', totalTokens: 300 }),
        pt({ source: 'codex', totalTokens: 100 }),
      ],
      summary,
    });

    const ctxTitle = () => c.querySelector('.tt-ctx-title')!.textContent ?? '';
    // First visible source (claude) is selected by default.
    expect(ctxTitle()).toContain('Claude Code');
    expect(ctxTitle()).not.toContain('Codex');

    const codexCard = Array.from(
      c.querySelectorAll<HTMLButtonElement>('.tt-toolcards button'),
    ).find((b) => b.textContent?.includes('Codex'))!;
    await act(async () => codexCard.click());

    // Selecting the Codex card re-points the source-driven panels.
    expect(ctxTitle()).toContain('Codex');
  });

  it('shows each source card\'s absolute tokens with its share beside them', async () => {
    const { container: c } = await mount({
      dayPoints: [
        pt({ source: 'claude', totalTokens: 300 }),
        pt({ source: 'codex', totalTokens: 100 }),
      ],
      summary,
    });

    const nums = Array.from(c.querySelectorAll('.tt-toolcards button .num'));
    // Separated, so the pair never reads as the single number "30075%".
    expect(nums.map((n) => n.textContent)).toEqual(['300 75%', '100 25%']);
    expect(nums.map((n) => n.querySelector('.pct')?.textContent)).toEqual(['75%', '25%']);
  });

  it('renders all-Unattributed Usage without exposing a Model Override action', async () => {
    const unattributed: BreakdownRow = {
      key: null, source: 'claude', inputTokens: 30, outputTokens: 20,
      cacheReadTokens: 0, cacheWriteTokens: 0, totalTokens: 50, requests: 1,
      cost: null, reasoningTokens: null, convs: 1, cacheEstimated: false,
      hasUnpriced: false, unattributedTokens: 50,
    };
    const { container: c } = await mount({
      dayPoints: [pt({ totalTokens: 50, byModel: {}, unattributedTokens: 50 })],
      summary: { ...summary, totalTokens: 50, requests: 1, cost: null, unattributedTokens: 50 },
      modelRows: [unattributed],
    });

    expect(c.querySelector('.tt-b8-cost')?.textContent).toContain('Unavailable');
    const row = c.querySelector<HTMLElement>('.tt-model')!;
    expect(row.textContent).toContain('Unattributed usage');
    expect(row.textContent).toContain('Unavailable');
    expect(row.getAttribute('role')).toBeNull();
    expect(row.tabIndex).toBe(-1);
  });

  it('separates a Model row\'s figures so they never read as one run', async () => {
    const priced: BreakdownRow = {
      key: 'claude-opus-4-8', source: 'claude', inputTokens: 30, outputTokens: 20,
      cacheReadTokens: 0, cacheWriteTokens: 0, totalTokens: 400, requests: 1,
      cost: 1.5, reasoningTokens: null, convs: 1, cacheEstimated: false,
      hasUnpriced: false, unattributedTokens: 0,
    };
    const { container: c } = await mount({
      dayPoints: [pt({ source: 'claude', totalTokens: 400, byModel: { 'claude-opus-4-8': 400 } })],
      summary,
      modelRows: [priced],
    });

    // Name, tokens, Cost and share each stand apart, single-spaced.
    expect(c.querySelector('.tt-model .top')?.textContent).toBe('claude-opus-4-8 400 $1.50 100%');
  });

  it('renders the 2D activity grid as a fixed-pitch scrollable calendar', async () => {
    const { container: c } = await mount({
      dayPoints: [pt({
        source: 'claude', totalTokens: 350,
        byModel: { 'claude-opus-4-8': 300 }, unattributedTokens: 50,
      })],
      summary,
    });

    // Weekday rail: Monday-first, 7 rows.
    const rail = Array.from(c.querySelectorAll('.tt-heat2d-days span'), (s) => s.textContent);
    expect(rail).toHaveLength(7);
    expect(rail[0]).toBe('Mon');
    expect(rail[6]).toBe('Sun');

    // Fixed-pitch grid inside a horizontal scroller: 365 day cells and an
    // explicit pixel width far wider than the card (22px per week column).
    const svg = c.querySelector<SVGSVGElement>('.tt-heat2d-scroll svg')!;
    const cells = svg.querySelectorAll('rect');
    expect(cells).toHaveLength(365);
    expect(Number(svg.getAttribute('width'))).toBeGreaterThan(52 * 22);

    // Month labels ride above the columns (single-column edge months are skipped).
    expect(c.querySelectorAll('.tt-heat2d-month').length).toBeGreaterThanOrEqual(10);

    // Calendar integrity: cells render in day order, so across the grid the
    // column may never decrease, and within a column each next day must sit on
    // a lower row — a Sunday filed into the wrong week (Sunday-start columns
    // under Monday-first rows) breaks this and leaves a hole in the grid.
    let px = -1;
    let py = -1;
    for (const cell of cells) {
      const x = Number(cell.getAttribute('x'));
      const y = Number(cell.getAttribute('y'));
      expect(x).toBeGreaterThanOrEqual(px);
      if (x === px) expect(y).toBeGreaterThan(py);
      px = x;
      py = y;
    }

    // Hovering the seeded day adds the outline rect and a cell-anchored
    // tooltip listing tokens per MODEL (not per tool). Cell index mirrors
    // seriesToDays: index 364 is today, one per calendar day back from there.
    const today = new Date();
    const midnight = new Date(today.getFullYear(), today.getMonth(), today.getDate());
    const seeded = 364 - Math.round((midnight.getTime() - new Date(2026, 6, 16).getTime()) / 86_400_000);
    await act(async () => {
      cells[seeded].dispatchEvent(new MouseEvent('mouseover', { bubbles: true }));
    });
    expect(svg.querySelectorAll('rect')).toHaveLength(366);
    const tip = c.querySelector('.tt-tip')!;
    expect(tip.textContent).toContain('claude-opus-4-8');
    expect(tip.textContent).toContain('Unattributed usage');
    expect(tip.textContent).not.toContain('Claude Code');
  });

  it('keeps Unattributed Usage distinct in the usage-trend tooltip', async () => {
    const { container: c } = await mount({
      dayPoints: [pt({
        source: 'claude', totalTokens: 150,
        byModel: { 'claude-opus-4-8': 100 }, unattributedTokens: 50,
      })],
      summary,
    });
    const card = Array.from(c.querySelectorAll('.tt-card')).find(
      (el) => el.querySelector('.tt-title')?.textContent === 'Usage over time',
    )!;
    const hit = Array.from(card.querySelectorAll<SVGRectElement>('rect')).find(
      (rect) => rect.getAttribute('fill') === 'transparent',
    )!;
    await act(async () => hit.dispatchEvent(new MouseEvent('mouseover', { bubbles: true })));

    expect(card.querySelector('.tt-tip')?.textContent).toContain('Unattributed usage');
  });

  it('names the owning Source on each usage-trend tooltip model row', async () => {
    // A bare model name is ambiguous once two Sources share one (pi and codex
    // both report gpt-5.6-sol), so every row carries its Source.
    const { container: c } = await mount({
      dayPoints: [
        pt({ source: 'claude', totalTokens: 100, byModel: { 'claude-opus-4-8': 100 } }),
        pt({ source: 'codex', totalTokens: 60, byModel: { 'gpt-5.6-sol': 60 } }),
      ],
      summary,
    });
    const card = Array.from(c.querySelectorAll('.tt-card')).find(
      (el) => el.querySelector('.tt-title')?.textContent === 'Usage over time',
    )!;
    const hit = Array.from(card.querySelectorAll<SVGRectElement>('rect')).find(
      (rect) => rect.getAttribute('fill') === 'transparent',
    )!;
    await act(async () => hit.dispatchEvent(new MouseEvent('mouseover', { bubbles: true })));

    const rows = Array.from(
      card.querySelectorAll<HTMLElement>('.tt-tip-row .lab span:first-child'),
      (el) => el.textContent ?? '',
    );
    expect(rows.some((r) => r.includes('claude-opus-4-8') && r.includes('Claude'))).toBe(true);
    expect(rows.some((r) => r.includes('gpt-5.6-sol') && r.includes('Codex'))).toBe(true);
  });

  it('attributes a Model shared by two Sources per bucket, not range-wide', async () => {
    // codex ran gpt-5.6-sol on the 16th, pi ran the SAME model name on the 17th.
    // A range-wide model->Source map is last-write-wins, so it would label the
    // 16th "pi"; the 16th's row must say Codex.
    const { container: c } = await mount({
      dayPoints: [
        pt({ bucket: '2026-07-16', source: 'codex', totalTokens: 90, byModel: { 'gpt-5.6-sol': 90 } }),
        pt({ bucket: '2026-07-17', source: 'pi', totalTokens: 40, byModel: { 'gpt-5.6-sol': 40 } }),
      ],
      summary,
    });
    const card = Array.from(c.querySelectorAll('.tt-card')).find(
      (el) => el.querySelector('.tt-title')?.textContent === 'Usage over time',
    )!;
    const hits = Array.from(card.querySelectorAll<SVGRectElement>('rect')).filter(
      (rect) => rect.getAttribute('fill') === 'transparent',
    );
    const rowText = async (hit: SVGRectElement) => {
      await act(async () => hit.dispatchEvent(new MouseEvent('mouseover', { bubbles: true })));
      return card.querySelector('.tt-tip-row .lab span:first-child')?.textContent ?? '';
    };
    // Buckets are zero-filled across the window, so pick the two with usage.
    const texts: string[] = [];
    for (const hit of hits) {
      const text = await rowText(hit);
      if (text.includes('gpt-5.6-sol')) texts.push(text);
    }
    expect(texts.some((r) => r.includes('Codex'))).toBe(true);
    expect(texts.some((r) => r.includes('pi'))).toBe(true);
  });

  it('names the owning Source per day in the activity heatmap tooltip', async () => {
    // The SAME Model name on two days, produced by different Sources: each day's
    // row must name the Source that actually produced it that day.
    const { container: c } = await mount({
      dayPoints: [
        pt({ bucket: '2026-07-16', source: 'codex', totalTokens: 90, byModel: { 'gpt-5.6-sol': 90 } }),
        pt({ bucket: '2026-07-18', source: 'pi', totalTokens: 40, byModel: { 'gpt-5.6-sol': 40 } }),
      ],
      summary,
    });
    const svg = c.querySelector<SVGSVGElement>('.tt-heat2d-scroll svg')!;
    const cells = Array.from(svg.querySelectorAll('rect'));
    const today = new Date();
    const midnight = new Date(today.getFullYear(), today.getMonth(), today.getDate());
    // Cell index mirrors seriesToDays: index 364 is today, one per day back.
    const idxOf = (y: number, m: number, d: number) =>
      364 - Math.round((midnight.getTime() - new Date(y, m, d).getTime()) / 86_400_000);
    const rowAt = async (i: number) => {
      await act(async () => cells[i].dispatchEvent(new MouseEvent('mouseover', { bubbles: true })));
      return c.querySelector('.tt-card.heat .tt-tip-row .lab span:first-child')?.textContent ?? '';
    };

    expect(await rowAt(idxOf(2026, 6, 16))).toContain('Codex');
    expect(await rowAt(idxOf(2026, 6, 18))).toContain('pi');
  });

  it('keeps the usage-trend y-axis labels out of the plot area', async () => {
    const { container: c } = await mount({
      dayPoints: [pt({ source: 'claude', totalTokens: 300, byModel: { 'claude-opus-4-8': 300 } })],
      summary,
    });

    const card = Array.from(c.querySelectorAll('.tt-card')).find(
      (el) => el.querySelector('.tt-title')?.textContent === 'Usage over time',
    )!;
    // The card header's Enlarge control carries its own icon svg; the chart is
    // the svg that holds the bars.
    const svg = Array.from(card.querySelectorAll('svg')).find((s) => s.querySelector('rect'))!;

    // Bars paint from the plot's left edge rightward; the y labels live in the
    // gutter left of it, end-anchored so they grow away from the bars. Drawing
    // them inside the plot is what let tall bars cover the numbers.
    const bars = Array.from(svg.querySelectorAll('rect')).filter((r) => r.getAttribute('fill') !== 'transparent');
    expect(bars.length).toBeGreaterThan(0);
    const plotLeft = Math.min(...bars.map((r) => Number(r.getAttribute('x'))));

    const yLabels = Array.from(svg.querySelectorAll('text')).filter((el) => el.getAttribute('text-anchor') === 'end');
    expect(yLabels).toHaveLength(5);
    for (const label of yLabels) expect(Number(label.getAttribute('x'))).toBeLessThan(plotLeft);
  });

  it('renders a static 3D landscape on the Activity card', async () => {
    const { container: c } = await mount({
      dayPoints: [pt({ source: 'claude', totalTokens: 300 })],
      summary,
    });

    const threeD = Array.from(c.querySelectorAll<HTMLButtonElement>('.tt-seg button')).find(
      (b) => b.textContent === '3D',
    )!;
    await act(async () => threeD.click());

    // The landscape draws (a top face per day at minimum) and is static:
    // no grab cursor, and a drag must not change the geometry.
    const svg = c.querySelector<SVGSVGElement>('.tt-heat-wrap svg')!;
    expect(svg.querySelectorAll('path').length).toBeGreaterThanOrEqual(365);
    expect(svg.classList.contains('grab')).toBe(false);

    const shape = () => svg.querySelector('path')!.getAttribute('d');
    const before = shape();
    await act(async () => {
      svg.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true, clientX: 300 }));
      window.dispatchEvent(new MouseEvent('mousemove', { bubbles: true, clientX: 400 }));
      window.dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
    });
    expect(shape()).toBe(before);
  });

  it('lists per-source scan stats in the footer', async () => {
    const scan: ScanStatus = {
      scannedAt: 0,
      sources: [
        { source: 'claude', eventsInserted: 412, linesSkipped: 0, limitReadings: 0, artifactsUnreadable: 0, unreadableMaxMtime: null, error: null },
        { source: 'codex', eventsInserted: 88, linesSkipped: 2, limitReadings: 0, artifactsUnreadable: 0, unreadableMaxMtime: null, error: null },
      ],
    };
    const { container: c } = await mount({
      dayPoints: [pt({ source: 'claude', totalTokens: 300 })],
      summary,
      scan,
    });

    const foot = c.querySelector('.tt-scan-foot')!.textContent ?? '';
    expect(foot).toContain('claude: 412 in / 0 skipped');
    expect(foot).toContain('codex: 88 in / 2 skipped');
  });

  // Unreadable Artifacts (ADR-0017): the window's token totals are floors, so
  // the headline and the affected Source's card carry the same ≥ marker as
  // Partial Cost, with the per-Source reasons as the marker's hover text.
  it('marks token totals as floors when a Source holds Unreadable Artifacts', async () => {
    const scan: ScanStatus = {
      scannedAt: 0,
      sources: [
        { source: 'antigravity', eventsInserted: 0, linesSkipped: 0, limitReadings: 0, artifactsUnreadable: 100, unreadableMaxMtime: 1_782_907_202, error: null },
        { source: 'claude', eventsInserted: 412, linesSkipped: 0, limitReadings: 0, artifactsUnreadable: 0, unreadableMaxMtime: null, error: null },
      ],
    };
    const { container: c } = await mount({
      dayPoints: [
        pt({ source: 'claude', totalTokens: 300 }),
        pt({ source: 'antigravity', totalTokens: 100 }),
      ],
      summary,
      scan,
    });

    const mark = c.querySelector<HTMLElement>('.tt-b8-total .tt-b8-total-mark')!;
    expect(mark.textContent).toContain('≥');
    expect(mark.getAttribute('title')).toBe('Antigravity: 100 sessions unreadable');

    // The reason is visible text beside the eyebrow (Partial Cost's pattern),
    // not tooltip-only, and it reaches the screen reader via the aria-label.
    expect(c.querySelector('.tt-eyebrow')?.textContent).toContain('100 sessions unreadable');
    expect(c.querySelector('.tt-b8-total')?.getAttribute('aria-label')).toContain(
      'Antigravity: 100 sessions unreadable',
    );

    const cards = Array.from(c.querySelectorAll<HTMLButtonElement>('.tt-toolcards button'));
    const card = (name: string) => cards.find((el) => el.textContent?.includes(name))!;
    expect(card('Antigravity').textContent).toContain('≥');
    expect(card('Antigravity').getAttribute('title')).toBe('Antigravity: 100 sessions unreadable');
    expect(card('Claude').textContent).not.toContain('≥');
    expect(card('Claude').getAttribute('title')).toBeNull();

    expect(c.querySelector('.tt-scan-foot')?.textContent).toContain(
      'antigravity: 0 in / 0 skipped / 100 unreadable',
    );
  });

  // ADR-0018: the ≥ has exactly one remedy, so Decrypt sits beside the reason
  // rather than in the toolbar. Pressing it runs the companion and then
  // rescans, because the Artifacts it writes are not usage until a Scan reads
  // them — without that second step the button would look like it did nothing.
  it('offers Decrypt beside the unreadable reason and rescans once it has run', async () => {
    // Settled timers on purpose: useAutoRefresh coalesces calls for a full
    // second after each scan, and only the fake companion returns fast enough
    // to land inside that window — a real export takes far longer.
    const { container: c, ledger } = await mountSettled({
      dayPoints: [pt({ source: 'antigravity', totalTokens: 100 })],
      summary,
      scan: {
        scannedAt: 0,
        sources: [
          { source: 'antigravity', eventsInserted: 0, linesSkipped: 0, limitReadings: 0, artifactsUnreadable: 100, unreadableMaxMtime: 1_782_907_202, error: null },
        ],
      },
      exportReport: 'exported 100 Session(s), 4466 generation(s)',
    });

    const button = c.querySelector<HTMLButtonElement>('.tt-decrypt')!;
    expect(button).toBeTruthy();
    expect(button.textContent).toBe('Decrypt');
    expect(button.disabled).toBe(false);

    const scansBefore = ledger.calls.scan.length;
    await act(async () => button.click());
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_200); // ride out the rescan's spin hold
    });

    expect(ledger.calls.exportAntigravity.length).toBe(1);
    expect(ledger.calls.scan.length).toBeGreaterThan(scansBefore);
    expect(c.querySelector('.tt-decrypt-note')?.textContent).toContain('exported 100 Session(s)');
  });

  // Offered only where it can act: the companion reads Antigravity and nothing
  // else, so an unreadable-free scan must not advertise it.
  it('offers no Decrypt when nothing is unreadable', async () => {
    const { container: c } = await mount({
      dayPoints: [pt({ source: 'claude', totalTokens: 300 })],
      summary,
      scan: {
        scannedAt: 0,
        sources: [
          { source: 'claude', eventsInserted: 412, linesSkipped: 0, limitReadings: 0, artifactsUnreadable: 0, unreadableMaxMtime: null, error: null },
        ],
      },
    });
    expect(c.querySelector('.tt-decrypt')).toBeNull();
  });

  // The same scan, viewed through a window that starts after every Unreadable
  // Artifact's last write, is definitely complete — no marker. Content is
  // never newer than its file, so only the window's start matters.
  it('drops the ≥ marker when the window starts after the last unreadable write', async () => {
    const scan: ScanStatus = {
      scannedAt: 0,
      sources: [
        // mtime far in the past; the day/week/month ranges all start later.
        { source: 'antigravity', eventsInserted: 0, linesSkipped: 0, limitReadings: 0, artifactsUnreadable: 100, unreadableMaxMtime: 946_684_800, error: null },
      ],
    };
    const { container: c } = await mount({
      dayPoints: [pt({ source: 'claude', totalTokens: 300 })],
      summary,
      scan,
    });

    // Default range is total (unbounded start): marked.
    expect(c.querySelector('.tt-b8-total .tt-b8-total-mark')).not.toBeNull();

    const monthBtn = Array.from(c.querySelectorAll<HTMLButtonElement>('button'))
      .find((b) => b.textContent === 'Month')!;
    await act(async () => monthBtn.click());
    await settle();
    expect(c.querySelector('.tt-b8-total .tt-b8-total-mark')).toBeNull();
  });

  it('renders the last-scan label at the scan wall-clock time (backend sends epoch seconds)', async () => {
    const scannedAtSec = 1_780_300_000; // 2026-06-01T… — any real epoch-second instant
    const scan: ScanStatus = { scannedAt: scannedAtSec, sources: [] };
    const { container: c } = await mountSettled({
      dayPoints: [pt({ source: 'claude', totalTokens: 300 })],
      summary,
      scan,
    });

    const label = c.querySelector('.tt-lastscan')!.textContent ?? '';
    expect(label).toContain(new Date(scannedAtSec * 1000).toLocaleTimeString());
  });

  it('renders the toolbar: range control, last-scan status, and Rescan (no select/avatar)', async () => {
    const { container: c } = await mount({
      dayPoints: [pt({ source: 'claude', totalTokens: 300 })],
      summary,
    });

    const toolbar = c.querySelector('.tt-toolbar')!;
    // range segmented control (one button per preset)
    expect(toolbar.querySelectorAll('.tt-seg button').length).toBe(RANGES_8B.length);
    // last-scan status + Rescan replace the old auto-refresh select + avatar
    expect(toolbar.querySelector('.tt-lastscan')).not.toBeNull();
    const rescan = toolbar.querySelector('.tt-rescan');
    expect(rescan).not.toBeNull();
    expect(rescan!.textContent).toContain('Rescan');
    expect(toolbar.querySelector('select')).toBeNull();
    expect(c.querySelector('.tt-avatar')).toBeNull();
    expect(c.querySelector('.tt-h1')).toBeNull();
  });

  it('re-runs the scan when the toolbar Rescan is clicked', async () => {
    const { container: c, ledger } = await mountSettled({
      dayPoints: [pt({ source: 'claude', totalTokens: 300 })],
      summary,
    });

    // The initial mount load already ran one scan through the same refresh path.
    expect(ledger.calls.scan.length).toBe(1);

    const rescan = c.querySelector<HTMLButtonElement>('.tt-rescan')!;
    await act(async () => rescan.click());
    // The spinner holds ≥1s even for an instant scan; ride it out.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_200);
    });

    expect(ledger.calls.scan.length).toBe(2);
    expect(c.querySelector('.tt-lastscan')?.textContent).toMatch(/^last scan/);
  });
});

// The window report (docs/specs 2026-08-10): one CSV over whatever the Overview
// is showing, written through the same save port the Trend enlarge uses.
describe('Export', () => {
  it('writes the window under the range it covers', async () => {
    const { container, exporter } = await mountOverview();
    await click(container.querySelector('.tt-export')!);
    expect(exporter.calls).toHaveLength(1);
    const [filename, contents] = exporter.calls[0];
    expect(filename).toMatch(/^usage-\d{4}-\d{2}-\d{2}_\d{4}-\d{2}-\d{2}\.csv$/);
    expect(contents.startsWith('tokenledger_report,1\n')).toBe(true);
  });

  // The property the architecture exists for: the file is a function of the
  // state that rendered, so it cannot report a total the screen never showed.
  it('reports the same total the headline shows', async () => {
    const { container, exporter } = await mountOverview();
    const headline = headlineTotal(container);
    await click(container.querySelector('.tt-export')!);
    const [, contents] = exporter.calls[0];
    const summaryRow = contents.split('\n\n').find((b) => b.startsWith('window,input_tokens'))!.split('\n')[1];
    expect(Number(summaryRow.split(',')[5])).toBe(headline);
  });

  it('says nothing when the save dialog is cancelled', async () => {
    const { container } = await mountOverview({ exporter: makeFakeExporter('cancelled') });
    await click(container.querySelector('.tt-export')!);
    expect(container.querySelector('.tt-error')).toBe(null);
  });

  it('surfaces a failed write in the error band', async () => {
    const { container } = await mountOverview({ exporter: makeFakeExporter('fails') });
    await click(container.querySelector('.tt-export')!);
    expect(container.querySelector('.tt-error')?.textContent).toContain('disk full');
  });

  it('offers Export for an empty window, which costs $0.00 rather than nothing', async () => {
    const { container } = await mountOverview({ ledger: makeFakeLedger({ dayPoints: [] }) });
    expect(container.querySelector<HTMLButtonElement>('.tt-export')!.disabled).toBe(false);
  });

  // A window whose Summary has not landed cannot be serialized honestly: the
  // selector's token fields are non-nullable, so an absent Summary becomes 0 —
  // a figure meaning "unknown". Only the caller can refuse, and it does.
  //
  // Reaching that state takes steps, because the fake's hold() is scoped to
  // the METHOD, not the call: holding 'summary' also holds the Profile-sessions
  // count that rides with each series load — and the first load runs twice,
  // once as the provisional pre-scan paint and once as the post-scan
  // reconcile. Resolving those two Profile calls lets both series land (which
  // is what ends `loading`); every summary still held after that is a window
  // Summary — the provisional reload's, which paints but is still owed a
  // reconcile, and the reconcile's, which is the one whose landing enables
  // Export.
  it('withholds Export until the window\'s Summary has landed', async () => {
    const ledger = makeFakeLedger({ dayPoints: [pt({})], summary });
    ledger.hold('summary');
    const { container } = await mountOverview({ ledger });
    const exportBtn = () => container.querySelector<HTMLButtonElement>('.tt-export')!;

    ledger.resolveHeld('summary', 0); // provisional Profile — the first paint lands
    await settle();
    ledger.resolveHeld('summary', 1); // reconcile Profile — the post-scan series lands
    await settle();
    expect(container.querySelector('.tt')?.classList.contains('tt-loading')).toBe(false);
    expect(exportBtn().disabled).toBe(true); // the window's Summary is still pending

    // Pin the launch fan-out: exactly four summaries — two Profile counts
    // (resolved above) and two window Summaries. A fifth appearing here means
    // boot grew another fetch pass; this line is where that regression fails.
    expect(ledger.held('summary').length).toBe(4);
    ledger.resolveHeld('summary', 2); // provisional reload's Summary — paints, still behind
    ledger.resolveHeld('summary', 3); // reconcile's Summary — the one that lands
    await settle();
    expect(exportBtn().disabled).toBe(false);
  });

  // The launch paint is real figures read from a pre-scan Ledger: the cost line
  // reads out rather than showing '…', and yet no file may state those figures
  // (Overview's `provisional` gate) — the screen corrects itself in half a
  // second where a file would keep them. Holding the scan holds the app in
  // exactly that state.
  it('withholds Export through the launch paint, then offers it once the scan settles', async () => {
    const ledger = makeFakeLedger({ dayPoints: [pt({}), pt({ bucket: '2026-07-15' })], summary });
    ledger.hold('scan');
    const { container } = await mountOverview({ ledger });
    const exportBtn = () => container.querySelector<HTMLButtonElement>('.tt-export')!;

    // The provisional window reload landed — this is a painted screen, not a
    // loading one — so the gate under test is `provisional`, not the Summary.
    expect(container.querySelector('.tt-b8-cost')?.textContent ?? '').not.toContain('…');
    expect(exportBtn().disabled).toBe(true);

    ledger.resolveHeld('scan', 0);
    await settle(8);
    expect(exportBtn().disabled).toBe(false);
  });

  // A Summary that has landed is not the same as a Summary for THIS window.
  // Switching range moves the header instantly and leaves every figure behind
  // until the reload lands; exporting in that gap would write the new window
  // over the old window's numbers, and unlike the screen a file keeps it.
  it('withholds Export while the window has moved ahead of its figures', async () => {
    const ledger = makeFakeLedger({ dayPoints: [pt({}), pt({ bucket: '2026-07-15' })], summary });
    const { container, exporter } = await mountOverview({ ledger });
    const exportBtn = () => container.querySelector<HTMLButtonElement>('.tt-export')!;
    expect(exportBtn().disabled).toBe(false);

    ledger.hold('summary');
    await click(Array.from(container.querySelectorAll<HTMLButtonElement>('.tt-toolbar .tt-seg button'))
      .find((b) => b.textContent === 'Week')!);
    expect(exportBtn().disabled).toBe(true);

    // Even clicked through, nothing is written: the gate is the button's own.
    await click(exportBtn());
    expect(exporter.calls).toHaveLength(0);

    ledger.resolveHeld('summary', ledger.held('summary').length - 1);
    await settle();
    expect(exportBtn().disabled).toBe(false);
  });

  // Two instants, deliberately different. The window and its grain describe the
  // render being exported; `generated` describes the save. A tab rendered on
  // the 20th and exported on the 25th must write the 20th's trailing-30-day
  // window under the 25th's timestamp — swapping either instant for the other
  // breaks exactly one of these two assertions.
  it('stamps the save instant while the window stays the render\'s', async () => {
    vi.setSystemTime(new Date(2026, 6, 20, 12));
    const { container, exporter } = await mountOverview();
    // Month, so the bounds are computed from an instant rather than from the
    // Ledger's extent as an unbounded Total window would be.
    await click(Array.from(container.querySelectorAll<HTMLButtonElement>('.tt-toolbar .tt-seg button'))
      .find((b) => b.textContent === 'Month')!);

    // Five days pass with the tab open. No view dependency moves, so the render
    // — and the window it is showing — stays where it was.
    vi.setSystemTime(new Date(2026, 6, 25, 9, 30));
    await click(container.querySelector('.tt-export')!);

    const [filename, contents] = exporter.calls[0];
    // The first block is the header; the summary block's own header also starts
    // "window,", so scope the lookup rather than scanning the whole file.
    const headerBlock = contents.split('\n\n')[0].split('\n');
    const line = (prefix: string) => headerBlock.find((l) => l.startsWith(prefix))!;
    expect(line('generated,')).toBe(`generated,${new Date(2026, 6, 25, 9, 30).toISOString()}`);
    // 30 trailing days ending on the render's day, not the save's.
    expect(line('window,')).toBe('window,2026-06-21,2026-07-20');
    expect(filename).toBe('usage-2026-06-21_2026-07-20.csv');
  });
});
