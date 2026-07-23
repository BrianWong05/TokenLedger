/** @vitest-environment jsdom */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import App from './App';
import { systemClock } from './overview/overviewStore';
import { makeFakeLedger } from './overview/ledger.fake';
import { makeFakeSettings } from './settings/settings.fake';
import type { SeriesPoint, Summary } from './types';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

// applyTheme() reads matchMedia, which jsdom does not implement.
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
  totalTokens: 100, requests: 2, cost: 1.5, hasUnpriced: false,
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

afterEach(() => {
  for (const root of mountedRoots.splice(0)) act(() => root.unmount());
  document.body.replaceChildren();
});

describe('App shell', () => {
  it('switches tabs, renders the right page, and preserves Overview state', async () => {
    const ledger = makeFakeLedger({
      dayPoints: [
        pt({ source: 'claude', totalTokens: 100 }),
        pt({ source: 'codex', totalTokens: 200 }),
      ],
      summary,
    });
    const settings = makeFakeSettings();

    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => {
      root.render(<App ports={{ ledger, clock: systemClock, settings }} />);
    });
    await settle();

    const nav = () =>
      Array.from(container.querySelectorAll('.tl-nav button')) as HTMLButtonElement[];

    // Three tabs, Overview active by default and showing its data.
    expect(nav().map((b) => b.textContent)).toEqual(['Overview', 'Pricing', 'Settings']);
    expect(nav()[0].classList.contains('active')).toBe(true);
    const overviewTab = container.querySelector('.tl-tab') as HTMLElement;
    expect(overviewTab.hidden).toBe(false);
    expect(container.querySelector('.tt-toolcards')).not.toBeNull();
    expect(container.querySelector('.tl-page-pricing')).toBeNull();
    expect(ledger.calls.scan.length).toBe(1);

    // Switch to Pricing: its page renders; Overview stays mounted but hidden.
    await act(async () => nav()[1].click());
    expect(nav()[1].classList.contains('active')).toBe(true);
    expect(container.querySelector('.tl-page-pricing')).not.toBeNull();
    expect(overviewTab.hidden).toBe(true);
    expect(container.querySelector('.tt-toolcards')).not.toBeNull(); // still in the DOM

    // Switch to Settings.
    await act(async () => nav()[2].click());
    expect(container.querySelector('.tl-page-settings')).not.toBeNull();
    expect(container.querySelector('.tl-page-pricing')).toBeNull();

    // Back to Overview: no remount, no re-scan, data intact.
    await act(async () => nav()[0].click());
    expect(overviewTab.hidden).toBe(false);
    expect(container.querySelector('.tt-toolcards')).not.toBeNull();
    expect(ledger.calls.scan.length).toBe(1);
  });

  it('opens the Settings tab when the Menu Bar Extra asks for it', async () => {
    const ledger = makeFakeLedger({ dayPoints: [pt({})], summary });
    const settings = makeFakeSettings();

    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => {
      root.render(<App ports={{ ledger, clock: systemClock, settings }} />);
    });
    await settle();

    // Starts on Overview; the tray's "Settings…" item fires the event.
    expect(container.querySelector('.tl-page-settings')).toBeNull();
    await act(async () => settings.emitOpenSettings());

    expect(container.querySelector('.tl-page-settings')).not.toBeNull();
    const active = container.querySelector('.tl-nav button[aria-current="page"]');
    expect(active?.textContent).toBe('Settings');
  });
});
