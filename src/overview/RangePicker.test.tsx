/** @vitest-environment jsdom */

// The Custom-range picker at the same seam as the other dialog tests: the real
// Overview over fake ports, driven through the Custom segment that opens it.
// Covers the two-click pick, the shortcuts, the bounds (no selecting days the
// data does not cover), the month/year menus, and the remembered window.
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
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

// Fixtures are daysAgo() off the clock, and the month menu counts the calendar
// months they touch — which depends on where in the month today falls: 99 days
// spans four months from mid-June, five from the 1st. Pinned to a mid-month
// noon so the count is the fixture's, not the day the suite ran on.
const NOW = new Date(2026, 5, 15, 12);

beforeEach(() => {
  vi.setSystemTime(NOW);
  localStorage.clear();
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
    inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0,
    totalTokens: 0, reasoningTokens: null, cost: 0, requests: 1, convs: 1,
    ctxMessages: null, ctxSystem: null, ctxReasoning: null, ctxToolcalls: null,
    ctxAgents: null, ctxMcp: null, ctxSkills: null, ...over,
  };
}

const summary: Summary = {
  inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0,
  totalTokens: 700, requests: 3, cost: 1.5, hasUnpriced: false, unattributedTokens: 0,
  unpricedModels: [], cacheEstimatedModels: [], cacheHitRate: 0, convs: 0,
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

// `first` days of history. The default spans several months; a shorter one puts
// the first record inside the visible pair, where its bound can be asserted on
// whatever today's date happens to be.
async function mount(first = 99): Promise<HTMLElement> {
  const ledger = makeFakeLedger({
    dayPoints: [
      pt({ bucket: daysAgo(first), totalTokens: 10 }),
      pt({ bucket: daysAgo(Math.floor(first / 2)), totalTokens: 900 }),
      pt({ bucket: daysAgo(1), totalTokens: 300 }),
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
  return container;
}

afterEach(() => {
  for (const root of mountedRoots.splice(0)) act(() => root.unmount());
  document.body.replaceChildren();
  vi.useRealTimers();
});

const segment = (c: HTMLElement, label: string) =>
  Array.from(c.querySelectorAll<HTMLButtonElement>('.tt-toolbar .tt-seg button'))
    .find((b) => b.textContent === label)!;
const pop = (c: HTMLElement) => c.querySelector<HTMLElement>('.tt-dp-pop');
const days = (c: HTMLElement) =>
  Array.from(c.querySelectorAll<HTMLButtonElement>('.tt-dp-day'));
const dayCell = (c: HTMLElement, iso: string) =>
  c.querySelector<HTMLButtonElement>(`.tt-dp-day[data-iso="${iso}"]`);
const eyebrow = (c: HTMLElement) => c.querySelector<HTMLElement>('.tt-eyebrow')!.textContent!;

async function openPicker(c: HTMLElement) {
  await act(async () => segment(c, 'Custom').click());
  await settle(1);
  return pop(c)!;
}

describe('Custom-range picker', () => {
  it('opens from the Custom segment and closes when it is clicked again', async () => {
    const c = await mount();
    expect(pop(c)).toBeNull();

    const p = await openPicker(c);
    expect(p).not.toBeNull();
    expect(p.querySelectorAll('.tt-dp-month')).toHaveLength(2);
    expect(segment(c, 'Custom').getAttribute('aria-expanded')).toBe('true');

    // A real press, not just .click(): the mousedown lands outside the popover,
    // and dismissing on it would let the click that follows reopen the picker,
    // so the segment could never close what it opened.
    await act(async () => {
      const b = segment(c, 'Custom');
      b.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
      b.click();
    });
    expect(pop(c)).toBeNull();
    expect(segment(c, 'Custom').getAttribute('aria-expanded')).toBe('false');
  });

  it('closes when a press lands anywhere else on the page', async () => {
    const c = await mount();
    await openPicker(c);
    await act(async () => {
      document.body.dispatchEvent(new MouseEvent('mousedown', { bubbles: true }));
    });
    expect(pop(c)).toBeNull();
  });

  it('offers only shortcuts the segments do not already cover', async () => {
    const c = await mount();
    const p = await openPicker(c);
    const labels = Array.from(p.querySelectorAll('.tt-dp-preset')).map((b) => b.textContent);
    // Week is the trailing 7 days and Month the trailing 30, so neither appears
    // here; Total already means all time.
    expect(labels).toEqual(['Yesterday', 'This month', 'Last 90 days', 'This year']);
  });

  it('applies a shortcut to the page window and closes', async () => {
    const c = await mount();
    const p = await openPicker(c);
    await act(async () => {
      Array.from(p.querySelectorAll<HTMLButtonElement>('.tt-dp-preset'))
        .find((b) => b.textContent === 'Yesterday')!.click();
    });
    await settle(1);
    expect(pop(c)).toBeNull();
    // A one-day window reads as the same date twice in the eyebrow.
    const yest = daysAgo(1);
    const label = new Date(Number(yest.slice(0, 4)), Number(yest.slice(5, 7)) - 1, Number(yest.slice(8)))
      .toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
    expect(eyebrow(c)).toContain(`${label} – ${label}`);
  });

  it('takes a start then an end click, and previews the span in between', async () => {
    const c = await mount();
    const p = await openPicker(c);
    expect(p.querySelector('.tt-dp-nav span')!.textContent).toBe('Pick the start date');

    const start = dayCell(c, daysAgo(20))!;
    await act(async () => start.click());
    expect(pop(c)!.querySelector('.tt-dp-nav span')!.textContent).toBe('Pick the end date');

    // Hovering an end date shows the running count rather than the hint.
    const end = dayCell(c, daysAgo(10))!;
    await act(async () => end.dispatchEvent(new MouseEvent('mouseover', { bubbles: true })));
    await act(async () => end.dispatchEvent(new MouseEvent('mouseenter')));
    expect(pop(c)!.querySelector('.tt-dp-nav span')!.textContent).toBe('11 days');

    await act(async () => end.click());
    await settle(1);
    expect(pop(c)).toBeNull();
    expect(eyebrow(c)).toContain('–');
  });

  it('will not select a day outside the recorded data', async () => {
    // 20 days of history, so both sides of the first record are on screen.
    const c = await mount(20);
    await openPicker(c);
    expect(dayCell(c, daysAgo(25))!.disabled).toBe(true);  // before the first record
    expect(dayCell(c, daysAgo(15))!.disabled).toBe(false); // inside it
    expect(dayCell(c, daysAgo(0))!.disabled).toBe(false);  // today is selectable
    // Nothing after today is, whatever is rendered past it.
    const today = isoOf(new Date());
    expect(days(c).filter((b) => b.dataset.iso! > today).every((b) => b.disabled)).toBe(true);
  });

  it('jumps to a month from the heading menu, and offers no month without data', async () => {
    const c = await mount();
    const p = await openPicker(c);
    const monthButton = p.querySelector<HTMLButtonElement>('.tt-dp-jump-btn')!;
    await act(async () => monthButton.click());

    const items = Array.from(p.querySelectorAll<HTMLButtonElement>('.tt-dp-menu-item'));
    expect(items).toHaveLength(12);
    // 99 days of data touch at most four calendar months, so most of the year
    // is unreachable rather than opening onto an empty grid.
    expect(items.filter((b) => b.disabled).length).toBeGreaterThanOrEqual(8);

    const target = items.find((b) => !b.disabled)!;
    const name = target.textContent;
    await act(async () => target.click());
    expect(p.querySelectorAll('.tt-dp-menu')).toHaveLength(0);
    expect(p.querySelector('.tt-dp-jump-btn')!.textContent).toContain(name!.slice(0, 3));
  });

  it('jumps to a year from the heading menu, landing the picked month on the side it was picked from', async () => {
    const c = await mount(400); // spans two calendar years
    const p = await openPicker(c);
    const headings = () => Array.from(p.querySelectorAll<HTMLButtonElement>('.tt-dp-jump-btn'));
    // [0] month / [1] year of the left calendar, [2] / [3] of the right.
    const rightYear = headings()[3];
    const thisYear = new Date().getFullYear();
    expect(rightYear.textContent).toContain(String(thisYear));

    await act(async () => rightYear.click());
    const lastYear = Array.from(p.querySelectorAll<HTMLButtonElement>('.tt-dp-menu-item'))
      .find((b) => b.textContent === String(thisYear - 1))!;
    expect(lastYear.disabled).toBe(false);
    await act(async () => lastYear.click());

    // Picked on the right heading, so that year lands on the right and the left
    // calendar shows the month before it.
    expect(headings()[3].textContent).toContain(String(thisYear - 1));
    const leftMonth = headings()[0].textContent;
    const rightMonth = headings()[2].textContent;
    expect(leftMonth).not.toBe(rightMonth);
  });

  it('walks the grid with arrow keys, paging the calendar and taking focus with it', async () => {
    const c = await mount();
    await openPicker(c);
    const first = dayCell(c, daysAgo(20))!;
    await act(async () => first.focus());

    const press = async (key: string) => {
      await act(async () => {
        (document.activeElement as HTMLElement).dispatchEvent(
          new KeyboardEvent('keydown', { key, bubbles: true }),
        );
      });
    };
    await press('ArrowRight');
    expect((document.activeElement as HTMLElement).dataset.iso).toBe(daysAgo(19));
    await press('ArrowDown');
    expect((document.activeElement as HTMLElement).dataset.iso).toBe(daysAgo(12));
    await press('ArrowUp');
    expect((document.activeElement as HTMLElement).dataset.iso).toBe(daysAgo(19));

    // Six weeks up leaves the visible pair, so the calendar has to page and
    // focus has to follow it — otherwise focus lands on <body> and every
    // further key is dead.
    for (let i = 0; i < 6; i++) await press('ArrowUp');
    const landed = document.activeElement as HTMLElement;
    expect(landed.dataset.iso).toBe(daysAgo(19 + 6 * 7));
    expect(landed.className).toContain('tt-dp-day');

    // Walking past the first record stops at the bound instead of escaping it.
    for (let i = 0; i < 20; i++) await press('ArrowUp');
    const stopped = (document.activeElement as HTMLElement).dataset.iso!;
    expect(stopped >= daysAgo(99)).toBe(true);
  });

  it('abandoning a half-made pick drops the pending start date', async () => {
    const c = await mount();
    await openPicker(c);
    await act(async () => dayCell(c, daysAgo(20))!.click());
    expect(pop(c)!.querySelector('.tt-dp-nav span')!.textContent).toBe('Pick the end date');

    await act(async () => {
      document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    });
    expect(pop(c)).toBeNull();

    await openPicker(c);
    expect(pop(c)!.querySelector('.tt-dp-nav span')!.textContent).toBe('Pick the start date');
  });

  it('remembers the picked window for the next launch, without restoring the range itself', async () => {
    const c = await mount();
    const p = await openPicker(c);
    await act(async () => {
      Array.from(p.querySelectorAll<HTMLButtonElement>('.tt-dp-preset'))
        .find((b) => b.textContent === 'This month')!.click();
    });
    await settle(1);
    const saved = localStorage.getItem('tokenledger.customRange');
    expect(saved).toContain(`${isoOf(new Date()).slice(0, 8)}01`);

    // Remount: the dates come back, but the app still opens on Total.
    for (const root of mountedRoots.splice(0)) act(() => root.unmount());
    document.body.replaceChildren();
    const c2 = await mount();
    expect(segment(c2, 'Total').className).toContain('active');
    await openPicker(c2);
    expect(pop(c2)!.querySelector('.tt-dp-preset.on')!.textContent).toBe('This month');
  });
});
