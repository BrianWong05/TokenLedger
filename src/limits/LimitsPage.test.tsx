/** @vitest-environment jsdom */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';
import LimitsPage from './LimitsPage';
import { LIVE_ENABLED_KEY, MODE_KEY, type LimitsPort } from './limits';
import type { SourceLimits } from '../types';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const roots: Root[] = [];
afterEach(() => {
  for (const r of roots.splice(0)) act(() => r.unmount());
  document.body.replaceChildren();
});

async function settle(times = 4) {
  for (let i = 0; i < times; i++) await act(async () => { await new Promise((r) => setTimeout(r, 0)); });
}

// 2026-08-12T00:00:00Z
const NOW_MS = 1_786_492_800_000;
const NOW = NOW_MS / 1000;
const HOUR = 3600;
const DAY = 24 * HOUR;

function fakePort(over: Partial<LimitsPort> & { store?: Record<string, string> } = {}): LimitsPort & {
  store: Record<string, string>;
  liveCalls: string[];
} {
  const store = over.store ?? { [LIVE_ENABLED_KEY]: 'true' };
  const liveCalls: string[] = [];
  return {
    store,
    liveCalls,
    list: over.list ?? (() => Promise.resolve([])),
    checkLive:
      over.checkLive ??
      ((source: string): Promise<void> => {
        liveCalls.push(source);
        return Promise.resolve();
      }),
    scan: over.scan ?? (() => Promise.resolve(null)),
    read: (k) => store[k] ?? null,
    write: (k, v) => { store[k] = v; },
  };
}

// A clock the test drives, so the fetch floor can be walked past deliberately
// rather than waited out.
let clock = NOW_MS;

async function mount(port: LimitsPort, at = NOW_MS) {
  clock = at;
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  roots.push(root);
  await act(async () => root.render(<LimitsPage ports={{ limits: port }} now={() => clock} />));
  await settle();
  return container;
}

// Same page, same stored preferences, fresh mount — what a tab switch does.
async function remount(port: LimitsPort, at = clock) {
  for (const r of roots.splice(0)) act(() => r.unmount());
  return mount(port, at);
}

const cardEls = (c: HTMLElement) => Array.from(c.querySelectorAll('.tl-lim-card')) as HTMLElement[];
const cardFor = (c: HTMLElement, label: string) =>
  cardEls(c).find((el) => el.querySelector('.tl-lim-name')?.textContent === label)!;
const rows = (card: HTMLElement) => Array.from(card.querySelectorAll('.tl-lim-row')) as HTMLElement[];
const btn = (c: HTMLElement, text: string) =>
  Array.from(c.querySelectorAll('button')).find((b) => b.textContent === text) as HTMLButtonElement;

const CODEX_WEEKLY: SourceLimits = {
  source: 'codex',
  plan: 'plus',
  windows: [
    { windowKey: 'w10080', windowMinutes: 10080, usedPct: 59, resetsAt: NOW + 4 * DAY, observedAt: NOW - 3 * HOUR },
  ],
};

const CLAUDE_LIVE: SourceLimits = {
  source: 'claude',
  plan: 'Team 5x',
  windows: [
    { windowKey: 'five_hour', windowMinutes: 300, usedPct: 18, resetsAt: NOW + 185 * 60, observedAt: NOW - 12 },
    { windowKey: 'seven_day', windowMinutes: 10080, usedPct: 59, resetsAt: NOW + 4 * DAY, observedAt: NOW - 12 },
    { windowKey: 'seven_day_zephyr', windowMinutes: 10080, usedPct: 37, resetsAt: NOW + 4 * DAY, observedAt: NOW - 12 },
  ],
};

const GROK_CREDITS: SourceLimits = {
  source: 'grok',
  plan: 'SuperGrok',
  windows: [
    { windowKey: 'w10080', windowMinutes: 10080, usedPct: 16, resetsAt: NOW + 4 * DAY, observedAt: NOW - 3 * HOUR },
  ],
};

describe('the opt-in disclosure', () => {
  it('replaces the card area on first visit and reads no credential', async () => {
    const port = fakePort({ store: {}, list: () => Promise.resolve([CODEX_WEEKLY]) });
    const c = await mount(port);

    expect(c.querySelector('.tl-lim-optin')).not.toBeNull();
    expect(cardEls(c)).toHaveLength(0);
    expect(port.liveCalls).toEqual([]);
    // The bounds are part of the disclosure, not a footnote to add later.
    expect(c.querySelector('.tl-lim-optin .bounds')?.textContent).toMatch(/never on a timer/);
  });

  it('enabling persists one boolean, checks live, and reveals the cards', async () => {
    const port = fakePort({ store: {}, list: () => Promise.resolve([CODEX_WEEKLY]) });
    const c = await mount(port);

    await act(async () => btn(c, 'Enable live limit checks').click());
    await settle();

    expect(port.store[LIVE_ENABLED_KEY]).toBe('true');
    expect(port.liveCalls).toEqual(['claude', 'codex']);
    expect(c.querySelector('.tl-lim-optin')).toBeNull();
    // Codex readings flowed from ordinary scans all along, so its card already
    // has history the moment the disclosure is dismissed.
    expect(rows(cardFor(c, 'Codex'))).toHaveLength(1);
  });
});

describe('card states', () => {
  it('renders a live card per window with its plan pill and freshness', async () => {
    const c = await mount(fakePort({ list: () => Promise.resolve([CLAUDE_LIVE, CODEX_WEEKLY]) }));

    const claude = cardFor(c, 'Claude');
    expect(claude.className).not.toMatch(/off/);
    expect(claude.querySelector('.tl-lim-plan')?.textContent).toBe('Team 5x');
    expect(claude.querySelector('.tl-lim-fresh')?.textContent).toBe('checked just now');
    expect(rows(claude)).toHaveLength(3);

    // Codex checks live now too; its stored readings' age is the fetch age.
    expect(cardFor(c, 'Codex').querySelector('.tl-lim-fresh')?.textContent).toBe(
      'checked 3h ago',
    );
  });

  it('reads a live Source with nothing recorded as not signed in', async () => {
    const c = await mount(fakePort({ list: () => Promise.resolve([CODEX_WEEKLY]) }));
    const claude = cardFor(c, 'Claude');

    expect(claude.className).toMatch(/off/);
    expect(claude.querySelector('.tl-lim-trouble .title')?.textContent).toBe('Not signed in');
    expect(claude.querySelector('.tl-lim-trouble .hint')?.textContent).toBe(
      'Sign in with the claude CLI, then check again.',
    );
    expect(claude.querySelector('.tl-lim-plan')).toBeNull();
    expect(btn(claude, 'Check again')).toBeTruthy();
  });

  it('keeps a Companion failure distinct from being signed out', async () => {
    const port = fakePort({
      list: () => Promise.resolve([]),
      checkLive: () => Promise.reject(new Error('security find-generic-password exited 51')),
    });
    const c = await mount(port);
    const claude = cardFor(c, 'Claude');

    const trouble = claude.querySelector('.tl-lim-trouble')!;
    expect(trouble.className).toMatch(/error/);
    expect(trouble.querySelector('.title')?.textContent).toBe("Couldn't check");
    expect(trouble.querySelector('.hint')?.textContent).toBe(
      'security find-generic-password exited 51',
    );
    expect(btn(claude, 'Retry')).toBeTruthy();
  });

  it('renders a refused credential as signed out, not as a failure', async () => {
    const c = await mount(
      fakePort({
        list: () => Promise.resolve([]),
        checkLive: () => Promise.reject(new Error('claude: 401 from the usage endpoint')),
      }),
    );
    expect(cardFor(c, 'Claude').querySelector('.tl-lim-trouble .title')?.textContent).toBe(
      'Not signed in',
    );
  });

  it('reads a codex with nothing recorded as not signed in, naming its own CLI', async () => {
    // Codex is a `live` Source, so an absence there means the credential, not
    // the logs. Grok is the `logs` case, covered below.
    const c = await mount(fakePort({ list: () => Promise.resolve([]) }));
    const codex = cardFor(c, 'Codex');
    expect(codex.className).toMatch(/off/);
    expect(codex.querySelector('.tl-lim-trouble .title')?.textContent).toBe('Not signed in');
    expect(codex.querySelector('.tl-lim-trouble .hint')?.textContent).toBe(
      'Sign in with the codex CLI, then check again.',
    );
  });

  it('reads a logs Source with nothing recorded as nothing recorded, never signed out', async () => {
    // Grok has nothing to sign into from here: its Readings arrive when the CLI
    // next runs, so the card says that rather than sending someone to re-auth.
    const c = await mount(fakePort({ list: () => Promise.resolve([]) }));
    const grok = cardFor(c, 'Grok');
    expect(grok.querySelector('.tl-lim-trouble .title')?.textContent).toBe(
      'No Grok activity recorded yet',
    );
    expect(grok.querySelector('.tl-lim-ghost')).toBeNull();
  });

  it('gives every catalogued limits Source a card and nobody else one', async () => {
    const c = await mount(fakePort({ list: () => Promise.resolve([]) }));
    expect(cardEls(c).map((el) => el.querySelector('.tl-lim-name')?.textContent)).toEqual([
      'Claude',
      'Codex',
      'Grok',
    ]);
  });
});

describe('bars', () => {
  it('draws the framed figure with a neutral time tick', async () => {
    const c = await mount(fakePort({ list: () => Promise.resolve([CLAUDE_LIVE]) }));
    const [session, weekly] = rows(cardFor(c, 'Claude'));

    // 82% left of the session window, with 185 of its 300 minutes to run.
    expect(session.querySelector('.tl-lim-num')?.textContent).toBe('82%');
    expect((session.querySelector('.tl-lim-bar .fill') as HTMLElement).style.width).toBe('82%');
    const tick = session.querySelector('.tl-lim-bar .tick') as HTMLElement;
    expect(tick.style.left).toBe(`${(185 / 300) * 100}%`);
    expect(tick.getAttribute('title')).toBe('now — 3h 5m until reset');

    expect(weekly.querySelector('.tl-lim-num')?.textContent).toBe('41%');
    expect(weekly.querySelector('.tl-lim-resets')?.textContent).toBe('Resets in 4d');
  });

  it('labels an unseen per-model window from its own key tail', async () => {
    const c = await mount(fakePort({ list: () => Promise.resolve([CLAUDE_LIVE]) }));
    const model = rows(cardFor(c, 'Claude'))[2];
    expect(model.querySelector('.tl-lim-label')?.textContent).toBe('ZephyrWeekly');
  });

  it('names Grok\'s one bar for the pool it meters, not for a rate-limit window', async () => {
    const c = await mount(fakePort({ list: () => Promise.resolve([GROK_CREDITS]) }));
    const grok = cardFor(c, 'Grok');
    const [credits] = rows(grok);

    expect(rows(grok)).toHaveLength(1);
    expect(credits.querySelector('.tl-lim-label')?.textContent).toBe('Weekly credits');
    expect(credits.querySelector('.tl-lim-num')?.textContent).toBe('84%');
    expect(grok.querySelector('.tl-lim-plan')?.textContent).toBe('SuperGrok');
    // From the CLI's own log, so the freshness line is the age of the last
    // request rather than the age of a fetch.
    expect(grok.querySelector('.tl-lim-fresh')?.textContent).toBe(
      'from your logs · last request 3h ago',
    );
  });

  it('renders a used-up window as spent', async () => {
    const dry: SourceLimits = {
      source: 'codex',
      plan: 'plus',
      windows: [
        { windowKey: 'w300', windowMinutes: 300, usedPct: 100, resetsAt: NOW + HOUR, observedAt: NOW - 60 },
      ],
    };
    const c = await mount(fakePort({ list: () => Promise.resolve([dry]) }));
    const row = rows(cardFor(c, 'Codex'))[0];
    // The figures are REPLACED, not annotated: no "0% left" anywhere on the row.
    expect(row.querySelector('.tl-lim-num')?.textContent).toBe('');
    expect(row.textContent).not.toMatch(/0\s*%/);
    const spent = row.querySelector('.tl-lim-resets')!;
    expect(spent.textContent).toBe('used up · resets in 1h');
    expect(spent.className).toMatch(/spent/);
  });

  it('renders an expired epoch as full and unused', async () => {
    const stale: SourceLimits = {
      source: 'codex',
      plan: 'plus',
      windows: [
        { windowKey: 'w300', windowMinutes: 300, usedPct: 100, resetsAt: NOW - HOUR, observedAt: NOW - 2 * HOUR },
      ],
    };
    const c = await mount(fakePort({ list: () => Promise.resolve([stale]) }));
    const row = rows(cardFor(c, 'Codex'))[0];
    expect(row.querySelector('.tl-lim-num')?.textContent).toBe('100%');
    expect(row.querySelector('.tl-lim-num')?.className).toMatch(/ok/);
  });
});

describe('the Left/Used toggle', () => {
  it('flips every figure, keeps the colors, and remembers the choice', async () => {
    const port = fakePort({ list: () => Promise.resolve([CODEX_WEEKLY]) });
    const c = await mount(port);
    const row = () => rows(cardFor(c, 'Codex'))[0];

    expect(row().querySelector('.tl-lim-num')?.textContent).toBe('41%');
    const toneBefore = row().querySelector('.tl-lim-bar')?.className;

    await act(async () => btn(c, 'Used').click());
    await settle();

    expect(row().querySelector('.tl-lim-num')?.textContent).toBe('59%');
    expect(row().querySelector('.tl-lim-bar')?.className).toBe(toneBefore);
    // The tick flips with the framing: 4/7 left becomes 3/7 elapsed.
    expect((row().querySelector('.tl-lim-bar .tick') as HTMLElement).style.left).toBe(
      `${100 - (4 / 7) * 100}%`,
    );
    expect(port.store[MODE_KEY]).toBe('used');
  });

  it('starts from the remembered choice', async () => {
    const port = fakePort({
      store: { [LIVE_ENABLED_KEY]: 'true', [MODE_KEY]: 'used' },
      list: () => Promise.resolve([CODEX_WEEKLY]),
    });
    const c = await mount(port);
    expect(rows(cardFor(c, 'Codex'))[0].querySelector('.tl-lim-num')?.textContent).toBe('59%');
  });
});

describe('fetch policy', () => {
  it('never checks live on a timer', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const port = fakePort({ list: () => Promise.resolve([CLAUDE_LIVE]) });
      await mount(port);
      expect(port.liveCalls).toEqual(['claude', 'codex']);

      // Nothing polls: time alone adds no calls, however much of it passes.
      await act(async () => { await vi.advanceTimersByTimeAsync(30 * 60_000); });
      expect(port.liveCalls).toEqual(['claude', 'codex']);
    } finally {
      vi.useRealTimers();
    }
  });

  it('holds the floor against Refresh, which no button may override', async () => {
    // The floor is a promise to the vendor's endpoint, so "a person asked" is
    // what permits a call at all, never what exempts it from the floor.
    const port = fakePort({ list: () => Promise.resolve([CLAUDE_LIVE]) });
    const c = await mount(port);
    expect(port.liveCalls).toEqual(['claude', 'codex']);

    await act(async () => btn(c, 'Refresh').click());
    await settle();
    expect(port.liveCalls, 'inside the floor, Refresh does not fetch').toEqual(['claude', 'codex']);

    // Past the floor, the same press does.
    clock = NOW_MS + 61_000;
    await act(async () => btn(c, 'Refresh').click());
    await settle();
    expect(port.liveCalls).toEqual(['claude', 'codex', 'claude', 'codex']);
  });

  it('keeps the floor across tab switches, which unmount the page', async () => {
    // The shell mounts this page on demand, so "page open" is a fresh mount.
    // A floor held in component state would reset with it, and flipping away
    // and back would fetch again immediately.
    const port = fakePort({ list: () => Promise.resolve([CLAUDE_LIVE]) });
    await mount(port);
    expect(port.liveCalls).toEqual(['claude', 'codex']);

    await remount(port, NOW_MS + 30_000);
    expect(port.liveCalls, 'still inside the floor').toEqual(['claude', 'codex']);

    await remount(port, NOW_MS + 61_000);
    expect(port.liveCalls, 'past it, a page open checks again').toEqual(['claude', 'codex', 'claude', 'codex']);
  });

  it('runs an ordinary scan on Refresh, which is how a logs Source updates', async () => {
    let scans = 0;
    let finish!: () => void;
    const port = fakePort({
      list: () => Promise.resolve([CODEX_WEEKLY]),
      scan: () => {
        scans += 1;
        return new Promise((r) => { finish = () => r(null); });
      },
    });
    const c = await mount(port);
    expect(scans).toBe(0);

    await act(async () => btn(c, 'Refresh').click());
    // The scan shows as one: inside the live floor this is the button's only
    // feedback, and without it an unchanged logs card made Refresh look broken.
    const busy = btn(c, 'Checking…');
    expect(busy).toBeTruthy();
    expect(busy.disabled).toBe(true);

    await act(async () => finish());
    await settle();
    expect(scans).toBe(1);
    expect(btn(c, 'Refresh')).toBeTruthy();
  });
});
