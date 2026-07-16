/** @vitest-environment jsdom */

// Presentation-level coverage for the rebuilt Overview: the hero proportion bar
// widths, the source-card selection wiring (a card click drives the rest of the
// Overview), and the per-source scan footer. Mounts the real component over a
// fake Ledger + Settings/Pricing ports.
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import Overview from './Overview';
import { RANGES_8B } from './meta';
import { REFRESH_PRESETS, STORAGE_KEY } from './useAutoRefresh';
import { systemClock } from './overviewStore';
import { makeFakeLedger } from './ledger.fake';
import { makeFakePricing } from '../pricing/pricing.fake';
import { makeFakeSettings } from '../settings/settings.fake';
import { SettingsProvider } from '../settings/SettingsContext';
import type { SeriesPoint, Summary, ScanStatus } from '../types';

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
    inputTokens: 10, outputTokens: 5, cacheReadTokens: 20, cacheWriteTokens: 3,
    totalTokens: 38, reasoningTokens: null, cost: 0, requests: 1, convs: 1,
    ctxMessages: null, ctxSystem: null, ctxReasoning: null, ctxToolcalls: null,
    ctxAgents: null, ctxMcp: null, ctxSkills: null, ...over,
  };
}

const summary: Summary = {
  inputTokens: 10, outputTokens: 5, cacheReadTokens: 20, cacheWriteTokens: 3,
  totalTokens: 400, requests: 2, cost: 1.5, hasUnpriced: false,
  unpricedModels: [], cacheEstimatedModels: [], cacheHitRate: 0,
};

const mountedRoots: Root[] = [];

async function settle(times = 4) {
  for (let i = 0; i < times; i++) {
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });
  }
}

async function mount(seed: Parameters<typeof makeFakeLedger>[0]) {
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
  return container;
}

afterEach(() => {
  for (const root of mountedRoots.splice(0)) act(() => root.unmount());
  document.body.replaceChildren();
});

describe('Overview presentation', () => {
  it('sizes the hero proportion bar by each source token share', async () => {
    // claude 300 + codex 100 = 400 → 75% / 25%; the other four sources are 0.
    const c = await mount({
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
    // TOOLS order: claude, codex, gemini, hermes, grok, antigravity.
    expect(widths[0]).toBe('75%');
    expect(widths[1]).toBe('25%');
    expect(widths[2]).toBe('0%'); // CSSOM serializes fmtPct(0) "0.0%" → "0%"
  });

  it('drives the rest of the Overview when a source card is selected', async () => {
    const c = await mount({
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

  it('lists per-source scan stats in the footer', async () => {
    const scan: ScanStatus = {
      scannedAt: 0,
      sources: [
        { source: 'claude', eventsInserted: 412, linesSkipped: 0, error: null },
        { source: 'codex', eventsInserted: 88, linesSkipped: 2, error: null },
      ],
    };
    const c = await mount({
      dayPoints: [pt({ source: 'claude', totalTokens: 300 })],
      summary,
      scan,
    });

    const foot = c.querySelector('.tt-scan-foot')!.textContent ?? '';
    expect(foot).toContain('claude: 412 in / 0 skipped');
    expect(foot).toContain('codex: 88 in / 2 skipped');
  });

  it('renders the toolbar: range control, auto-refresh select, and avatar', async () => {
    const c = await mount({
      dayPoints: [pt({ source: 'claude', totalTokens: 300 })],
      summary,
    });

    const toolbar = c.querySelector('.tt-toolbar')!;
    // range segmented control (one button per preset)
    expect(toolbar.querySelectorAll('.tt-seg button').length).toBe(RANGES_8B.length);
    // auto-refresh select carrying the REFRESH_PRESETS options
    const select = toolbar.querySelector('select')!;
    expect(Array.from(select.options, (o) => o.textContent)).toEqual(
      REFRESH_PRESETS.map((p) => p.label),
    );
    // BW avatar, and the old page title is gone
    expect(c.querySelector('.tt-avatar')?.textContent).toBe('BW');
    expect(c.querySelector('.tt-h1')).toBeNull();
  });

  it('persists the auto-refresh interval when the select changes', async () => {
    localStorage.removeItem(STORAGE_KEY);
    const c = await mount({
      dayPoints: [pt({ source: 'claude', totalTokens: 300 })],
      summary,
    });

    const select = c.querySelector<HTMLSelectElement>('.tt-toolbar select')!;
    expect(select.value).toBe('0'); // Off by default

    await act(async () => {
      select.value = '30';
      select.dispatchEvent(new Event('change', { bubbles: true }));
    });

    // onChange → setRefreshSec(30) drives the controlled value and persists it
    expect(select.value).toBe('30');
    expect(localStorage.getItem(STORAGE_KEY)).toBe('30');
  });
});
