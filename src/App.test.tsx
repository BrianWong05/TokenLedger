/** @vitest-environment jsdom */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import App from './App';
import type { Platform } from './lib/platform';
import { systemClock } from './overview/overviewStore';
import { makeFakeLedger } from './overview/ledger.fake';
import { makeFakeSettings } from './settings/settings.fake';
import type { Settings, Summary } from './types';
import { seriesPoint as pt } from './overview/seriesPoint';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

// The shell reads the running version (off-runtime getVersion would reject and
// hide the auto-updated toast from every test).
vi.mock('@tauri-apps/api/app', () => ({ getVersion: vi.fn().mockResolvedValue('1.4.2') }));

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
  // The shell records the running version here; a value left behind would
  // announce a phantom update to the next test.
  localStorage.clear();
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

  // TOKL-21: an available update shows as a dot on the Settings nav item, and
  // visiting Settings (where the banner and install flow live) retires it.
  it('marks the Settings nav item when an update is available, until visited', async () => {
    const mount = async (settings: ReturnType<typeof makeFakeSettings>) => {
      const container = document.createElement('div');
      document.body.append(container);
      const root = createRoot(container);
      mountedRoots.push(root);
      await act(async () => {
        root.render(
          <App ports={{ ledger: makeFakeLedger({ dayPoints: [pt({})], summary }), clock: systemClock, settings }} />,
        );
      });
      await settle();
      return container;
    };

    // An update pending: the dot appears without any tab being opened…
    const available = makeFakeSettings({}, { state: 'available', version: '9.9.9' });
    const withUpdate = await mount(available);
    expect(withUpdate.querySelector('.tl-nav-dot')).not.toBeNull();

    // …and opening Settings retires it, even after leaving again.
    const nav = () =>
      Array.from(withUpdate.querySelectorAll('.tl-nav button')) as HTMLButtonElement[];
    await act(async () => {
      nav()[3].click();
      await import('./settings/SettingsPage');
    });
    expect(withUpdate.querySelector('.tl-nav-dot')).toBeNull();
    await act(async () => nav()[0].click());
    expect(withUpdate.querySelector('.tl-nav-dot')).toBeNull();

    // No update pending: no dot. Auto-check off: not even a check.
    const none = await mount(makeFakeSettings());
    expect(none.querySelector('.tl-nav-dot')).toBeNull();
    const off = makeFakeSettings({ autoCheckUpdates: false }, { state: 'available', version: '9.9.9' });
    await mount(off);
    expect(off.calls.checkUpdates).toBe(0);
  });

  // TOKL-21: the bottom-right update card. Its Update button drives the same
  // download → restart flow as the Settings banner; ✕ merely dismisses the
  // card, leaving the nav dot as the quieter reminder.
  it('offers the update flow from the bottom-right card', async () => {
    const mount = async (settings: ReturnType<typeof makeFakeSettings>) => {
      const container = document.createElement('div');
      document.body.append(container);
      const root = createRoot(container);
      mountedRoots.push(root);
      await act(async () => {
        root.render(
          <App ports={{ ledger: makeFakeLedger({ dayPoints: [pt({})], summary }), clock: systemClock, settings }} />,
        );
      });
      await settle();
      return container;
    };

    // Update: downloads and stages it, then the same button restarts into it.
    const port = makeFakeSettings({}, { state: 'available', version: '9.9.9' });
    const card = await mount(port);
    expect(card.querySelector('.tl-toast')?.textContent).toContain('9.9.9');
    const btn = () => card.querySelector('.tl-toast-btn') as HTMLButtonElement;
    await act(async () => btn().click());
    await settle();
    expect(port.calls.downloadUpdate).toBe(1);
    await act(async () => btn().click());
    expect(port.calls.restartApp).toBe(1);

    // ✕: the card goes, the dot stays.
    const dismissed = await mount(makeFakeSettings({}, { state: 'available', version: '9.9.9' }));
    await act(async () =>
      (dismissed.querySelector('.tl-toast-close') as HTMLButtonElement).click(),
    );
    expect(dismissed.querySelector('.tl-toast')).toBeNull();
    expect(dismissed.querySelector('.tl-nav-dot')).not.toBeNull();

    // Visiting Settings retires the card — the banner there takes over.
    const visited = await mount(makeFakeSettings({}, { state: 'available', version: '9.9.9' }));
    const nav = Array.from(visited.querySelectorAll('.tl-nav button')) as HTMLButtonElement[];
    await act(async () => {
      nav[3].click();
      await import('./settings/SettingsPage');
    });
    await settle();
    expect(visited.querySelector('.tl-toast')).toBeNull();
  });

  it('announces an executed auto-update with the version jump, once', async () => {
    const mount = async () => {
      const container = document.createElement('div');
      document.body.append(container);
      const root = createRoot(container);
      mountedRoots.push(root);
      await act(async () => {
        root.render(
          <App
            ports={{ ledger: makeFakeLedger({ dayPoints: [pt({})], summary }), clock: systemClock, settings: makeFakeSettings() }}
          />,
        );
      });
      await settle();
      return container;
    };

    // First ever run: nothing to compare against, so no announcement — but the
    // running version is recorded for the next run.
    const first = await mount();
    expect(first.querySelector('.tl-toast')).toBeNull();
    expect(localStorage.getItem('tl-last-version')).toBe('1.4.2');

    // A run after an update was applied: the jump is announced…
    localStorage.setItem('tl-last-version', '1.0.0');
    const updated = await mount();
    expect(updated.querySelector('.tl-toast')?.textContent).toContain('1.0.0 → 1.4.2');

    // …and the record moves forward, so the next run stays quiet.
    expect(localStorage.getItem('tl-last-version')).toBe('1.4.2');
    const quiet = await mount();
    expect(quiet.querySelector('.tl-toast')).toBeNull();
  });
});
