// @vitest-environment node
import { describe, it, expect } from 'vitest';
import { createOverviewStore, selectDays, type ClockPort } from './overviewStore';
import { makeFakeLedger } from './ledger.fake';
import type { SeriesPoint, ScanStatus } from '../types';

// Fixed "now" = 2026-07-16; a separate virtual clock drives the debounce timers.
function fakeClock(): ClockPort & { advance(ms: number): void } {
  const fixed = new Date(2026, 6, 16);
  let vnow = 0;
  let seq = 1;
  const timers = new Map<number, { due: number; fn: () => void }>();
  return {
    now: () => new Date(fixed),
    setTimeout(fn, ms) {
      const h = seq++;
      timers.set(h, { due: vnow + ms, fn });
      return h;
    },
    clearTimeout(h) {
      timers.delete(h);
    },
    advance(ms) {
      vnow += ms;
      for (const [h, t] of [...timers].sort((a, b) => a[1].due - b[1].due)) {
        if (t.due <= vnow && timers.has(h)) {
          timers.delete(h);
          t.fn();
        }
      }
    },
  };
}

// Drain the microtask queue (canned ledger promises resolve there).
const flush = () => new Promise((r) => setTimeout(r, 0));

function pt(over: Partial<SeriesPoint>): SeriesPoint {
  return {
    bucket: '2026-07-16', source: 'claude', byModel: {}, unattributedTokens: 0, hasUnpriced: false,
    inputTokens: 10, outputTokens: 5, cacheReadTokens: 20, cacheWriteTokens: 3,
    totalTokens: 38, reasoningTokens: null, cost: 0, requests: 1, convs: 1,
    ctxMessages: null, ctxSystem: null, ctxReasoning: null, ctxToolcalls: null,
    ctxAgents: null, ctxMcp: null, ctxSkills: null, ...over,
  };
}

const scanWith = (errs: [string, string | null][]): ScanStatus => ({
  scannedAt: 0,
  sources: errs.map(([source, error]) => ({ source, eventsInserted: 0, linesSkipped: 0, error })),
});

// refresh() then flush the initial (delay-0) reload cycle.
async function boot(ledger: ReturnType<typeof makeFakeLedger>, clock: ReturnType<typeof fakeClock>) {
  const store = createOverviewStore({ ledger, clock });
  await store.refresh();
  clock.advance(0);
  await flush();
  return store;
}

describe('overviewStore refresh / scan', () => {
  it('joins per-source scan errors and persists them across a later successful reload', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger({
      scan: scanWith([['Claude Code', 'boom'], ['Codex', 'nope'], ['Gemini CLI', null]]),
    });
    const store = await boot(ledger, clock);
    expect(store.getSnapshot().scanError).toBe('Claude Code: boom · Codex: nope');
    expect(store.getSnapshot().fetchError).toBeNull(); // reload succeeded
    // A later reload never touches scanError.
    store.setRange('week');
    clock.advance(0);
    await flush();
    expect(store.getSnapshot().scanError).toBe('Claude Code: boom · Codex: nope');
  });

  it('fetchError sets on a failing cycle then clears on the next fully-successful one', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger();
    const store = await boot(ledger, clock);

    ledger.failNext('summary', 'kaboom');
    store.setRange('week');
    clock.advance(0);
    await flush();
    expect(store.getSnapshot().fetchError).toBe('kaboom');

    store.setRange('month');
    clock.advance(0);
    await flush();
    expect(store.getSnapshot().fetchError).toBeNull();
  });

  it('backend scannedAt arrives in epoch seconds and is stored as epoch ms', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger({
      scan: { scannedAt: 1_780_300_000, sources: [] },
    });
    const store = await boot(ledger, clock);
    expect(store.getSnapshot().scanAt).toBe(1_780_300_000_000);
  });

  it('an idle rescan (no errors, zero inserted) skips the series+reload fan-out', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger();
    const store = await boot(ledger, clock);
    const seriesCalls = ledger.calls.series.length;
    const summaryCalls = ledger.calls.summary.length;
    const before = store.getSnapshot();

    await store.refresh(); // ledger unchanged: default scan reports nothing inserted
    clock.advance(0);
    await flush();

    expect(ledger.calls.series).toHaveLength(seriesCalls);
    expect(ledger.calls.summary).toHaveLength(summaryCalls);
    expect(store.getSnapshot().allPoints).toBe(before.allPoints); // identity stable → no re-render churn
  });

  it('a rescan that ingested events still reloads', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger();
    const store = await boot(ledger, clock);
    const seriesCalls = ledger.calls.series.length;

    ledger.data.scan = {
      scannedAt: 0,
      sources: [{ source: 'claude', eventsInserted: 3, linesSkipped: 0, error: null }],
    };
    await store.refresh();
    clock.advance(0);
    await flush();

    expect(ledger.calls.series.length).toBeGreaterThan(seriesCalls);
  });

  it('scan() throw sets scanError and skips the series fetch', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger();
    ledger.failNext('scan', 'scanboom');
    const store = createOverviewStore({ ledger, clock });
    await store.refresh();
    expect(store.getSnapshot().scanError).toBe('scanboom');
    expect(ledger.calls.series).toHaveLength(0);
    expect(store.getSnapshot().allPoints).toBeNull();
    expect(store.getSnapshot().loading).toBe(true);
  });
});

describe('overviewStore reload orchestration', () => {
  it('stale-guard: a late response from a superseded range never lands', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger();
    const store = await boot(ledger, clock);

    ledger.hold('summary');
    const summaryA = { ...ledger.data.summary, totalTokens: 111 };
    const summaryB = { ...ledger.data.summary, totalTokens: 222 };

    store.setRange('week'); // range A
    clock.advance(0);
    await flush();
    store.setRange('month'); // range B supersedes A
    clock.advance(0);
    await flush();
    expect(ledger.held('summary')).toHaveLength(2);

    ledger.resolveHeld('summary', 1, summaryB); // B (current)
    await flush();
    ledger.resolveHeld('summary', 0, summaryA); // A (stale) — must be ignored
    await flush();

    expect(store.getSnapshot().summary).toBe(summaryB);
  });

  it('custom debounces at 250ms; presets fire at 0; a preset cancels a pending custom debounce', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger();
    const store = await boot(ledger, clock);

    // Custom range: reload waits for 250ms.
    store.setRange('custom');
    let n = ledger.calls.summary.length;
    clock.advance(249);
    await flush();
    expect(ledger.calls.summary.length).toBe(n); // not yet
    clock.advance(1);
    await flush();
    expect(ledger.calls.summary.length).toBe(n + 1);

    // setCustomRange while custom also debounces at 250ms.
    n = ledger.calls.summary.length;
    store.setCustomRange('2026-01-01', '2026-06-30');
    clock.advance(249);
    await flush();
    expect(ledger.calls.summary.length).toBe(n);
    clock.advance(1);
    await flush();
    expect(ledger.calls.summary.length).toBe(n + 1);

    // Pending custom debounce, then a preset: the custom timer is cleared, so
    // only the preset cycle runs.
    n = ledger.calls.summary.length;
    store.setRange('custom'); // schedules @250
    store.setRange('week'); // clears it, schedules @0
    clock.advance(300);
    await flush();
    expect(ledger.calls.summary.length).toBe(n + 1);
  });

  it('prices-rebuilt re-runs the reload; disposing stops it', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger();
    const store = await boot(ledger, clock);
    const dispose = store.start();

    const n = ledger.calls.summary.length;
    ledger.emitPricesRebuilt();
    clock.advance(0);
    await flush();
    expect(ledger.calls.summary.length).toBe(n + 1);
    // Reload used the current (total) filters — no date bounds.
    const summaryCalls = ledger.calls.summary;
    const lastFilters = summaryCalls[summaryCalls.length - 1][0] as { startTs?: number };
    expect(lastFilters.startTs).toBeUndefined();

    dispose();
    const m = ledger.calls.summary.length;
    ledger.emitPricesRebuilt();
    clock.advance(0);
    await flush();
    expect(ledger.calls.summary.length).toBe(m);
  });

  it('Day range fetches the hourly series; leaving Day clears hourPoints', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger({ hourPoints: [pt({ bucket: '2026-07-16 09:00' })] });
    const store = await boot(ledger, clock);
    expect(store.getSnapshot().hourPoints).toHaveLength(0);

    store.setRange('day');
    clock.advance(0);
    await flush();
    expect(store.getSnapshot().hourPoints).toHaveLength(1);
    expect(ledger.calls.series.some((a) => a[1] === 'hour')).toBe(true);

    store.setRange('week');
    clock.advance(0);
    await flush();
    expect(store.getSnapshot().hourPoints).toHaveLength(0);
  });
});

describe('overviewStore first-load vs later series failure', () => {
  it('first-load failure settles allPoints to [] (loading false); a later failure keeps prior data', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger();
    const store = createOverviewStore({ ledger, clock });

    ledger.failNext('series', 'sboom');
    await store.refresh();
    expect(store.getSnapshot().allPoints).toEqual([]);
    expect(store.getSnapshot().loading).toBe(false);
    expect(store.getSnapshot().fetchError).toBe('sboom');

    // Successful reload of real points.
    const pts = [pt({ totalTokens: 500 })];
    ledger.data.dayPoints = pts;
    await store.refresh();
    clock.advance(0);
    await flush();
    expect(store.getSnapshot().allPoints).toBe(pts);

    // A later series failure keeps the prior points (does not reset to []).
    // Scan must report an ingest: an idle rescan would skip the fetch entirely.
    ledger.data.scan = {
      scannedAt: 0,
      sources: [{ source: 'claude', eventsInserted: 1, linesSkipped: 0, error: null }],
    };
    ledger.failNext('series', 'sboom2');
    await store.refresh();
    expect(store.getSnapshot().allPoints).toBe(pts);
    expect(store.getSnapshot().loading).toBe(false);
    expect(store.getSnapshot().fetchError).toBe('sboom2');
  });
});

describe('overviewStore selection auto-correct', () => {
  it('snaps the selection to the first visible tool when the window drops it', async () => {
    const clock = fakeClock();
    // claude only outside today; codex only today.
    const ledger = makeFakeLedger({
      dayPoints: [
        pt({ bucket: '2026-01-01', source: 'claude', totalTokens: 100 }),
        pt({ bucket: '2026-07-16', source: 'codex', totalTokens: 200 }),
      ],
    });
    const store = await boot(ledger, clock);
    // Total window: claude is visible, selection stays.
    expect(store.getSnapshot().selected).toBe('claude');

    // Day window: only codex has usage → selection snaps.
    store.setRange('day');
    expect(store.getSnapshot().selected).toBe('codex');
  });
});

describe('overviewStore getSnapshot stability', () => {
  it('no-op transitions keep the same snapshot reference; allPoints identity is stable across setRange', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger({ dayPoints: [pt({ totalTokens: 50 })] });
    const store = await boot(ledger, clock);

    const s1 = store.getSnapshot();
    store.setSelected(s1.selected); // no-op
    expect(store.getSnapshot()).toBe(s1);
    store.setRange(s1.range); // no-op
    expect(store.getSnapshot()).toBe(s1);
    store.setCustomRange(s1.customFrom, s1.customTo); // no-op
    expect(store.getSnapshot()).toBe(s1);

    // A real transition rebuilds the snapshot but keeps allPoints's reference,
    // so a caller memoizing selectDays on allPoints identity never recomputes.
    store.setRange('week');
    const s2 = store.getSnapshot();
    expect(s2).not.toBe(s1);
    expect(s2.allPoints).toBe(s1.allPoints);
    expect(selectDays(s1, clock.now())).toBeInstanceOf(Array);
  });
});
