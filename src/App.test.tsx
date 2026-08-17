/** @vitest-environment jsdom */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import App from './App';
import type { Platform } from './lib/platform';
import { systemClock } from './overview/overviewStore';
import { makeFakeLedger } from './overview/ledger.fake';
import { makeFakeSettings } from './settings/settings.fake';
import type { Settings, SeriesPoint, Summary } from './types';

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
    bucket: '2026-07-16', source: 'claude', byModel: {}, unattributedTokens: 0, hasUnpriced: false,
    inputTokens: 10, outputTokens: 5, cacheReadTokens: 20, cacheWriteTokens: 3,
    totalTokens: 38, reasoningTokens: null, cost: 0, requests: 1, convs: 1,
    ctxMessages: null, ctxSystem: null, ctxReasoning: null, ctxToolcalls: null,
    ctxAgents: null, ctxMcp: null, ctxSkills: null, ...over,
  };
}

const summary: Summary = {
  inputTokens: 10, outputTokens: 5, cacheReadTokens: 20, cacheWriteTokens: 3,
  totalTokens: 100, requests: 2, cost: 1.5, hasUnpriced: false, unattributedTokens: 0,
  unpricedModels: [], cacheEstimatedModels: [], cacheHitRate: 0, convs: 0,
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
  // The headline's entrance gate lives in sessionStorage; a key left behind
  // would silently suppress the entrance for every later test.
  sessionStorage.clear();
});

describe('App shell', () => {
  // The strip exists to clear the macOS traffic lights, which sit over the
  // window because its title bar is hidden there. Windows and Linux wear their
  // native title bar, so the same strip would be a gap holding nothing.
  it('clears space for the traffic lights on macOS alone', async () => {
    const strip = async (platform: Platform) => {
      const container = document.createElement('div');
      document.body.append(container);
      const root = createRoot(container);
      mountedRoots.push(root);
      await act(async () => {
        root.render(
          <App
            ports={{ ledger: makeFakeLedger({ dayPoints: [pt({})], summary }), clock: systemClock, settings: makeFakeSettings() }}
            platform={platform}
          />,
        );
      });
      await settle();
      return container.querySelector('.tl-traffic');
    };

    expect(await strip('macos')).not.toBeNull();
    expect(await strip('windows')).toBeNull();
    expect(await strip('linux')).toBeNull();
  });

  // The caption describes whatever appearance setting the machine has, so it
  // names no operating system — in either language, and on every platform.
  it('says the theme follows the system without naming a platform', async () => {
    const caption = async (language: Settings['language'], platform: Platform) => {
      const container = document.createElement('div');
      document.body.append(container);
      const root = createRoot(container);
      mountedRoots.push(root);
      await act(async () => {
        root.render(
          <App
            ports={{ ledger: makeFakeLedger({ dayPoints: [pt({})], summary }), clock: systemClock, settings: makeFakeSettings({ language }) }}
            platform={platform}
          />,
        );
      });
      await settle();
      const nav = Array.from(container.querySelectorAll('.tl-nav button')) as HTMLButtonElement[];
      await act(async () => nav[2].click());
      return container.querySelector('.set-row-caption')?.textContent ?? '';
    };

    for (const [language, platform] of [
      ['en', 'macos'],
      ['en', 'windows'],
      ['zh-Hant', 'linux'],
    ] as const) {
      expect(await caption(language, platform)).not.toMatch(/mac|windows|linux/i);
    }
  });

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

    // Four tabs, Overview active by default and showing its data.
    expect(nav().map((b) => b.textContent)).toEqual(['Overview', 'Pricing', 'Limits', 'Settings']);
    expect(nav()[0].classList.contains('active')).toBe(true);
    const overviewTab = container.querySelector('.tl-tab') as HTMLElement;
    expect(overviewTab.hidden).toBe(false);
    expect(container.querySelector('.tt-toolcards')).not.toBeNull();
    expect(container.querySelector('.tl-page-pricing')).toBeNull();
    expect(ledger.calls.scan.length).toBe(1);

    // Switch to Pricing: its page renders; Overview stays mounted but hidden.
    await act(async () => {
      nav()[1].click();
      await import('./pricing/PricingPage');
    });
    expect(nav()[1].classList.contains('active')).toBe(true);
    expect(container.querySelector('.tl-page-pricing')).not.toBeNull();
    expect(overviewTab.hidden).toBe(true);
    expect(container.querySelector('.tt-toolcards')).not.toBeNull(); // still in the DOM

    // Switch to Limits: it mounts on demand like Pricing.
    await act(async () => {
      nav()[2].click();
      await import('./limits/LimitsPage');
    });
    expect(container.querySelector('.tl-page-limits')).not.toBeNull();
    expect(container.querySelector('.tl-page-pricing')).toBeNull();

    // Switch to Settings.
    await act(async () => {
      nav()[3].click();
      await import('./settings/SettingsPage');
    });
    expect(container.querySelector('.tl-page-settings')).not.toBeNull();
    expect(container.querySelector('.tl-page-limits')).toBeNull();

    // Back to Overview: no remount, no re-scan, data intact.
    await act(async () => nav()[0].click());
    expect(overviewTab.hidden).toBe(false);
    expect(container.querySelector('.tt-toolcards')).not.toBeNull();
    expect(ledger.calls.scan.length).toBe(1);
  });

  it('rolls the token total when the Overview tab comes back into view', async () => {
    // The entrance is spent, so the roll below can only be the tab return's.
    sessionStorage.setItem('tokenledger.tokenTotalEntrancePlayed', 'true');
    const ledger = makeFakeLedger({ dayPoints: [pt({ totalTokens: 100 })], summary });
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => {
      root.render(<App ports={{ ledger, clock: systemClock, settings: makeFakeSettings() }} />);
    });
    await settle();

    const nav = () => Array.from(container.querySelectorAll('.tl-nav button')) as HTMLButtonElement[];
    const headline = () => container.querySelector('.tt-b8-total')!;
    expect(headline().getAttribute('aria-busy')).toBeNull();

    await act(async () => {
      nav()[1].click();
      await import('./pricing/PricingPage');
    });
    // Hidden, not unmounted: still at rest, nothing about the total moved.
    expect(headline().getAttribute('aria-busy')).toBeNull();

    await act(async () => nav()[0].click());
    expect(headline().getAttribute('aria-busy')).toBe('true');
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

  // The same chord the Menu Bar Extra's panel carries, in the window: one
  // shortcut means one action wherever it is pressed (src/lib/hotkeys.ts).
  it.each([
    ['macos' as Platform, { key: ',', metaKey: true }],
    ['windows' as Platform, { key: ',', ctrlKey: true }],
    ['linux' as Platform, { key: ',', ctrlKey: true }],
  ])('opens the Settings tab on the %s Settings shortcut', async (platform, chord) => {
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => {
      root.render(
        <App
          ports={{ ledger: makeFakeLedger({ dayPoints: [pt({})], summary }), clock: systemClock, settings: makeFakeSettings() }}
          platform={platform}
        />,
      );
    });
    await settle();
    expect(container.querySelector('.tl-page-settings')).toBeNull();

    await act(async () => {
      document.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, ...chord }));
    });
    await settle();

    expect(container.querySelector('.tl-page-settings')).not.toBeNull();
  });

  it('leaves a near-miss of the Settings shortcut alone', async () => {
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => {
      root.render(
        <App
          ports={{ ledger: makeFakeLedger({ dayPoints: [pt({})], summary }), clock: systemClock, settings: makeFakeSettings() }}
          platform="macos"
        />,
      );
    });
    await settle();

    for (const chord of [
      { key: ',' }, // no modifier
      { key: ',', ctrlKey: true }, // the other platform's spelling
      { key: ',', metaKey: true, altKey: true },
      { key: ',', metaKey: true, shiftKey: true },
    ]) {
      await act(async () => {
        document.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, ...chord }));
      });
      await settle();
      expect(container.querySelector('.tl-page-settings')).toBeNull();
    }
  });
});
