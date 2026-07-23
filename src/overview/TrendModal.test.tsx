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
import type { SeriesPoint, Summary } from '../types';

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

const mountedRoots: Root[] = [];

async function settle(times = 4) {
  for (let i = 0; i < times; i++) {
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });
  }
}

async function mount(over: Partial<Summary> = {}): Promise<{ container: HTMLElement }> {
  const ledger = makeFakeLedger({
    dayPoints: [
      pt({ bucket: daysAgo(2), source: 'claude', totalTokens: 400, byModel: { 'claude-opus-4-8': 400 } }),
      pt({ bucket: daysAgo(1), source: 'claude', totalTokens: 100, byModel: { 'claude-opus-4-8': 100 } }),
      pt({ bucket: daysAgo(1), source: 'codex', totalTokens: 200, byModel: { 'gpt-5.5-codex': 200 } }),
    ],
    summary: { ...summary, ...over },
  });
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
  return { container };
}

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
});
