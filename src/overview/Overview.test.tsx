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
  unattributedTokens: 0, unpricedModels: [], cacheEstimatedModels: [], cacheHitRate: 0,
};

const mountedRoots: Root[] = [];

async function settle(times = 4) {
  for (let i = 0; i < times; i++) {
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });
  }
}

async function mount(
  seed: Parameters<typeof makeFakeLedger>[0],
): Promise<{ container: HTMLElement; ledger: FakeLedger }> {
  const ledger = makeFakeLedger(seed);
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  mountedRoots.push(root);
  await act(async () => {
    root.render(
      <SettingsProvider port={makeFakeSettings()}>
        <Overview ports={{ ledger, clock: systemClock, pricing: makeFakePricing() }} />
      </SettingsProvider>,
    );
  });
  await settle();
  return { container, ledger };
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
        <Overview ports={{ ledger, clock: systemClock, pricing: makeFakePricing() }} />
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
  vi.useRealTimers();
});

describe('Overview presentation', () => {
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
    // TOOLS order: claude, codex, gemini, hermes, grok, antigravity, pi.
    expect(widths[0]).toBe('75%');
    expect(widths[1]).toBe('25%');
    expect(widths[2]).toBe('0%'); // CSSOM serializes fmtPct(0) "0.0%" → "0%"
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
          { source: 'antigravity', eventsInserted: 0, linesSkipped: 0, error: null },
          { source: 'pi', eventsInserted: 3, linesSkipped: 2, error: null },
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
        { source: 'claude', eventsInserted: 412, linesSkipped: 0, error: null },
        { source: 'codex', eventsInserted: 88, linesSkipped: 2, error: null },
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
