/** @vitest-environment jsdom */

// Activity Enlarge (design 3a) at the agreed primary seam: the real Overview
// mounted over fake ports. Covers the dialog's open path (title + year stats),
// the close paths (Escape, backdrop, ✕) with focus restore, and the scroll
// lock — external behaviour only, no component internals.
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

// Buckets pinned to real local days: the heatmap window is the trailing 365
// days ending today, so the seed must land inside it.
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

async function mount(): Promise<{ container: HTMLElement; ledger: ReturnType<typeof makeFakeLedger> }> {
  // Point costs are deliberately zero: the enlarge's Cost must come from the
  // year-window Summary fetch, never from summing series points.
  const ledger = makeFakeLedger({
    dayPoints: [
      pt({ bucket: daysAgo(2), source: 'claude', totalTokens: 400 }),
      pt({ bucket: daysAgo(1), source: 'claude', totalTokens: 100 }),
      pt({ bucket: daysAgo(1), source: 'codex', totalTokens: 200 }),
    ],
    summary,
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
  return { container, ledger };
}

afterEach(() => {
  for (const root of mountedRoots.splice(0)) act(() => root.unmount());
  document.body.replaceChildren();
  document.documentElement.style.overflow = '';
  document.body.style.overflow = '';
});

const enlargeButton = (c: HTMLElement) =>
  c.querySelector<HTMLButtonElement>('.tt-heat-enlarge')!;
const dialog = () => document.querySelector<HTMLElement>('.tt-heat-modal');

async function open(c: HTMLElement): Promise<HTMLElement> {
  await act(async () => enlargeButton(c).click());
  return dialog()!;
}

describe('Activity Enlarge', () => {
  it('opens a dialog with the 3D perspective title and the year stats', async () => {
    const { container: c } = await mount();
    const modal = await open(c);

    expect(modal.getAttribute('role')).toBe('dialog');
    expect(modal.getAttribute('aria-modal')).toBe('true');
    expect(modal.querySelector('#tt-heat-modal-title')!.textContent).toBe(
      'Token 3D perspective',
    );

    // Stats derive from the seeded series: 700 tokens over 2 active days,
    // a 2-day streak, peak day of 400 tokens; Cost from the year-window
    // Summary fetch (seeded points carry zero cost).
    const stats = modal.querySelector('.tt-heat-modal-stats')!.textContent!;
    expect(stats).toContain('700');
    expect(stats).toContain('$1.50');
    expect(stats).toContain('400');

    // The landscape itself is present and rotatable (grab cursor class).
    expect(modal.querySelector('.tt-heat-modal-canvas svg.grab')).toBeTruthy();
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
    expect(document.activeElement).toBe(enlargeButton(c));
  });

  it('closes on the ✕ button and on a backdrop click, but not on clicks inside', async () => {
    const { container: c } = await mount();
    const modal = await open(c);

    // A click inside the panel must NOT close it.
    await act(async () => modal.querySelector<HTMLElement>('.tt-heat-modal-stats')!.click());
    expect(dialog()).not.toBeNull();

    await act(async () =>
      modal.querySelector<HTMLButtonElement>('.tt-heat-modal-close')!.click(),
    );
    expect(dialog()).toBeNull();

    await open(c);
    const backdrop = document.querySelector<HTMLElement>('.tt-heat-modal-backdrop')!;
    await act(async () => {
      backdrop.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
      backdrop.click();
    });
    expect(dialog()).toBeNull();
  });

  it('does not dismiss when a rotate drag is released over the backdrop', async () => {
    const { container: c } = await mount();
    const modal = await open(c);
    const svg = modal.querySelector<SVGSVGElement>('.tt-heat-modal-canvas svg')!;

    // mousedown on the landscape, release over the margin: the click event
    // lands on the backdrop (common ancestor) but must not close the dialog.
    await act(async () => {
      svg.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true, clientX: 300 }));
      window.dispatchEvent(new MouseEvent('mousemove', { bubbles: true, clientX: 200 }));
      window.dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
      document.querySelector<HTMLElement>('.tt-heat-modal-backdrop')!.click();
    });
    expect(dialog()).not.toBeNull();
  });

  it('keeps rotating across commits during a drag and suppresses the hover inspector', async () => {
    const { container: c } = await mount();
    const modal = await open(c);
    const svg = modal.querySelector<SVGSVGElement>('.tt-heat-modal-canvas svg')!;
    const topPath = () =>
      Array.from(svg.querySelectorAll('path')).find((p) => p.style.cursor === 'pointer')!;
    const shape = () => svg.querySelector('path')!.getAttribute('d');
    const tip = () => document.querySelector('.tt-tip');

    // Hover works before the drag…
    await act(async () => {
      topPath().dispatchEvent(new MouseEvent('mouseover', { bubbles: true }));
    });
    expect(tip()).not.toBeNull();

    // …and clears the moment a drag starts.
    await act(async () => {
      svg.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true, clientX: 300 }));
    });
    expect(tip()).toBeNull();

    // Each committed move keeps rotating — regression for the mid-drag freeze,
    // where the first re-render tore down the listeners and froze the view.
    // The later moves sweep far past the old ±0.5 rad clamp: a free spin keeps
    // turning (and keeps every bar solid — same number of wall faces).
    const pathCount = svg.querySelectorAll('path').length;
    let prev = shape();
    for (const x of [340, 380, 700, 1100]) {
      await act(async () => {
        window.dispatchEvent(new MouseEvent('mousemove', { bubbles: true, clientX: x }));
      });
      await settle(1);
      const next = shape();
      expect(next).not.toBe(prev);
      prev = next;
    }
    expect(svg.querySelectorAll('path').length).toBe(pathCount);

    // Sweeping over a day mid-drag must not pop the inspector.
    await act(async () => {
      topPath().dispatchEvent(new MouseEvent('mouseover', { bubbles: true }));
    });
    expect(tip()).toBeNull();

    // After release, hover inspects again.
    await act(async () => {
      window.dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
      topPath().dispatchEvent(new MouseEvent('mouseover', { bubbles: true }));
    });
    expect(tip()).not.toBeNull();
  });

  it('requests a Summary for the trailing-365-day window at the port when opened', async () => {
    const { container: c, ledger } = await mount();
    const before = ledger.calls.summary.length;
    await open(c);
    await settle(1);

    expect(ledger.calls.summary.length).toBe(before + 1);
    const [filters] = ledger.calls.summary[before] as [Filters];
    // The displayed window: [local midnight 364 days ago, midnight after today).
    const midnight = (offsetDays: number) => {
      const now = new Date();
      const d = new Date(now.getFullYear(), now.getMonth(), now.getDate() + offsetDays);
      return Math.floor(d.getTime() / 1000);
    };
    expect(filters.startTs).toBe(midnight(-364));
    expect(filters.endTs).toBe(midnight(1));
    expect(filters.tools).toEqual([]);
    expect(filters.models).toEqual([]);
    expect(filters.project).toBeNull();
  });

  it('shows a placeholder until the year Summary lands, then its ≥-marked figure', async () => {
    const { container: c, ledger } = await mount();
    ledger.hold('summary');
    const modal = await open(c);
    const stats = () => modal.querySelector('.tt-heat-modal-stats')!.textContent!;

    // Held fetch → placeholder, never the range-scoped Summary's figure.
    expect(stats()).toContain('…');
    expect(stats()).not.toContain('$');

    await act(async () => {
      ledger.resolveHeld('summary', 0, {
        ...summary,
        cost: 9.99,
        hasUnpriced: true,
        unpricedModels: ['self-hosted-x'],
      });
    });
    await settle(1);

    // Figure, ≥ Partial Cost marker, and Unpriced count all from the year window.
    expect(stats()).toContain('≥ $9.99');
    expect(stats()).toContain('1 unpriced');
    expect(stats()).not.toContain('…');
  });

  it('ignores a stale year Summary from a previous open after a quick reopen', async () => {
    const { container: c, ledger } = await mount();
    ledger.hold('summary');

    // Open (fetch #1 held), close, reopen (fetch #2 held).
    await open(c);
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    });
    const modal = await open(c);
    const stats = () => modal.querySelector('.tt-heat-modal-stats')!.textContent!;

    // The abandoned first fetch resolving must not replace the placeholder…
    await act(async () => {
      ledger.resolveHeld('summary', 0, { ...summary, cost: 111.11 });
    });
    await settle(1);
    expect(stats()).toContain('…');
    expect(stats()).not.toContain('$111.11');

    // …only the reopen's own fetch may land.
    await act(async () => {
      ledger.resolveHeld('summary', 1, { ...summary, cost: 2.22 });
    });
    await settle(1);
    expect(stats()).toContain('$2.22');
  });
});
