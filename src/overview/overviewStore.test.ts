// @vitest-environment node
import { describe, it, expect } from 'vitest';
import {
  createOverviewStore,
  selectDays,
  selectReportInput,
  selectView,
  selectVisibleTools,
  type ClockPort,
} from './overviewStore';
import { makeFakeLedger } from './ledger.fake';
import { execFacets } from './data';
import type { BreakdownRow, SeriesPoint, ScanStatus, Settings } from '../types';

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
  sources: errs.map(([source, error]) => ({ source, eventsInserted: 0, linesSkipped: 0, limitReadings: 0, artifactsUnreadable: 0, unreadableMaxMtime: null, error })),
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

  it('does not double-prefix an adapter warning that already names the Source', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger({
      scan: scanWith([['grok', 'grok: malformed or unsupported Source Artifact']]),
    });
    const store = await boot(ledger, clock);
    expect(store.getSnapshot().scanError).toBe(
      'Grok Build: malformed or unsupported Source Artifact',
    );
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
      sources: [{ source: 'claude', eventsInserted: 3, linesSkipped: 0, limitReadings: 0, artifactsUnreadable: 0, unreadableMaxMtime: null, error: null }],
    };
    await store.refresh();
    clock.advance(0);
    await flush();

    expect(ledger.calls.series.length).toBeGreaterThan(seriesCalls);
  });

  it('scan() throw on first load sets scanError but keeps the provisional paint', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger({ dayPoints: [pt({ totalTokens: 500 })] });
    ledger.failNext('scan', 'scanboom');
    const store = createOverviewStore({ ledger, clock });
    await store.refresh();
    clock.advance(0);
    await flush();
    expect(store.getSnapshot().scanError).toBe('scanboom');
    // The persisted Ledger still painted: data behind the error, not zeros.
    expect(store.getSnapshot().allPoints).toHaveLength(1);
    expect(store.getSnapshot().loading).toBe(false);
  });

  it('a paint kept through a scan throw stays provisional: the idle retry reconciles it', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger({ dayPoints: [pt({ totalTokens: 500 })] });
    ledger.failNext('scan', 'scanboom');
    const store = createOverviewStore({ ledger, clock });
    await store.refresh(); // paint lands, scan throws
    clock.advance(0);
    await flush();
    const seriesCalls = ledger.calls.series.length;

    // The retry scan succeeds but reports zero inserted. A throw can arrive
    // AFTER the backend committed, so the on-screen paint still predates a
    // settled scan — idle must not excuse skipping the reconcile.
    await store.refresh();
    clock.advance(0);
    await flush();
    expect(store.getSnapshot().scanError).toBeNull();
    expect(ledger.calls.series.length).toBeGreaterThan(seriesCalls);
    expect(store.getSnapshot().reloading).toBe(false);

    // Reconciled: from here the idle gate applies as usual.
    const settled = ledger.calls.series.length;
    await store.refresh();
    clock.advance(0);
    await flush();
    expect(ledger.calls.series.length).toBe(settled);
  });

  it('scan() throw after the first load skips the series fetch', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger();
    const store = await boot(ledger, clock);
    const seriesCalls = ledger.calls.series.length;

    ledger.failNext('scan', 'scanboom');
    await store.refresh();
    expect(store.getSnapshot().scanError).toBe('scanboom');
    expect(ledger.calls.series).toHaveLength(seriesCalls);
  });

  it('first load paints the persisted Ledger before the scan settles', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger({ dayPoints: [pt({ totalTokens: 500 })] });
    ledger.hold('scan');
    const store = createOverviewStore({ ledger, clock });
    const refreshing = store.refresh();
    await flush();
    clock.advance(0); // the provisional window reload's delay-0 timer
    await flush();

    // The scan is still running, yet the Overview already has data.
    expect(store.getSnapshot().loading).toBe(false);
    expect(store.getSnapshot().allPoints).toHaveLength(1);
    expect(store.getSnapshot().summary).not.toBeNull();

    ledger.resolveHeld('scan', 0);
    await refreshing;
  });

  it('reconciles the provisional paint after the scan even when it reports idle', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger({ dayPoints: [pt({ totalTokens: 500 })] });
    ledger.hold('scan');
    const store = createOverviewStore({ ledger, clock });
    const refreshing = store.refresh();
    await flush();
    clock.advance(0);
    await flush();
    const seriesCalls = ledger.calls.series.length;

    // Default scan: zero inserted, no errors — an idle verdict. The idle
    // gate's premise ("what's rendered IS the Ledger") doesn't hold for a
    // pre-scan paint, so everything is refetched anyway.
    ledger.resolveHeld('scan', 0);
    await refreshing;
    clock.advance(0);
    await flush();
    expect(ledger.calls.series.length).toBeGreaterThan(seriesCalls);
    expect(store.getSnapshot().reloading).toBe(false);
  });

  // Boot supersedes its own first window reload: the post-scan reconcile bumps
  // the epoch in the microtask right after the paint, so the reload's ten
  // queries were always issued and always discarded — leaving the headline
  // zero-shaped and the cost '…' until the SECOND fan-out landed (a third, from
  // prices-rebuilt, could supersede that one too). The figures were already
  // paid for; nothing may sit on a placeholder while a response is in hand.
  it('a superseded launch reload still paints, marked as behind', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger({ dayPoints: [pt({ totalTokens: 500 })] });
    ledger.hold('scan');
    const store = createOverviewStore({ ledger, clock });
    const refreshing = store.refresh();
    await flush();

    ledger.hold('summary');
    clock.advance(0); // the provisional window reload fires; its Summary is held
    await flush();
    expect(store.getSnapshot().summary).toBeNull();

    // The scan settles: the reconcile bumps the epoch, superseding that reload.
    ledger.resolveHeld('scan', 0);
    await flush();
    expect(store.getSnapshot().summary).toBeNull();

    // Now it answers. Painting it is what keeps the launch off placeholders.
    ledger.resolveHeld('summary', 0); // the provisional window Summary
    await flush();
    expect(store.getSnapshot().summary).not.toBeNull();
    // Honest about being pre-scan: window-scoped figures are still owed.
    expect(store.getSnapshot().reloading).toBe(true);

    // The reconcile lands over it and clears the flag.
    ledger.resolveHeld('summary', 1); // reconcile Profile count
    await refreshing;
    clock.advance(0);
    await flush();
    ledger.resolveHeld('summary', 2); // reconcile window Summary
    await flush();
    expect(store.getSnapshot().reloading).toBe(false);
  });

  // #14 spends the entrance reel on the first *authoritative* nonzero total.
  // The launch paint is not that: the reconcile may correct it, and #12 story 9
  // holds a same-window correction still — so a reel rolled on the provisional
  // figure would leave the settled one to arrive with no motion. The post-scan
  // series is what earns the flag, so the reveal waits one query pair rather
  // than the ten-query window fan-out behind it.
  it('withholds the entrance until the figure descends from a settled scan', async () => {
    const clock = fakeClock();
    // Two days, so the Total window is not a single day: a one-day window is
    // hourly, and the hourly series it would read is not seeded here.
    const ledger = makeFakeLedger({
      dayPoints: [pt({ totalTokens: 500 }), pt({ bucket: '2026-07-15', totalTokens: 500 })],
    });
    ledger.hold('scan');
    const store = createOverviewStore({ ledger, clock });
    const refreshing = store.refresh();
    await flush();
    clock.advance(0);
    await flush();

    // Figures on screen, but pre-scan: a nonzero total the reel could roll, and
    // a Summary for the cost line — yet the entrance stays owed.
    const painted = store.getSnapshot();
    expect(painted.summary).not.toBeNull();
    expect(selectView(painted, NOW).total).toBeGreaterThan(0);
    expect(selectView(painted, NOW).headline.authoritative).toBe(false);

    ledger.resolveHeld('scan', 0);
    await refreshing;
    clock.advance(0);
    await flush();
    // The post-scan series has landed: the reel may roll this one.
    expect(selectView(store.getSnapshot(), NOW).headline.authoritative).toBe(true);
  });

  it('a window fetched during the launch scan is refetched after it, not replayed', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger({ dayPoints: [pt({ totalTokens: 500 })] });
    ledger.hold('scan');
    const store = createOverviewStore({ ledger, clock });
    const refreshing = store.refresh();
    await flush();
    clock.advance(0);
    await flush();

    // Mid-scan range click: fetches and caches the week window pre-scan.
    store.setRange('week');
    clock.advance(0);
    await flush();
    const summaryCalls = ledger.calls.summary.length;

    ledger.resolveHeld('scan', 0);
    await refreshing;
    clock.advance(0);
    await flush();
    // A cache replay would add no summary call; the reconcile must refetch.
    expect(ledger.calls.summary.length).toBeGreaterThan(summaryCalls);
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

  it('returning to an already-loaded window reuses the cached reload: no refetch, identical references', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger();
    const store = await boot(ledger, clock);

    store.setRange('week');
    clock.advance(0);
    await flush();
    const weekSummary = store.getSnapshot().summary;
    const weekModels = store.getSnapshot().modelRows;
    const n = ledger.calls.summary.length;

    store.setRange('month');
    clock.advance(0);
    await flush();
    expect(ledger.calls.summary.length).toBe(n + 1);

    store.setRange('week');
    clock.advance(0);
    await flush();
    expect(ledger.calls.summary.length).toBe(n + 1); // served from cache
    expect(store.getSnapshot().summary).toBe(weekSummary); // same reference → memoized panels skip
    expect(store.getSnapshot().modelRows).toBe(weekModels);
    expect(store.getSnapshot().reloading).toBe(false); // the cached landing cleared the flag
  });

  it('a scan that ingested Usage Records invalidates every cached window', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger();
    const store = await boot(ledger, clock); // total: cached

    store.setRange('week');
    clock.advance(0);
    await flush();

    ledger.data.scan = {
      scannedAt: 0,
      sources: [{ source: 'claude', eventsInserted: 3, linesSkipped: 0, limitReadings: 0, artifactsUnreadable: 0, unreadableMaxMtime: null, error: null }],
    };
    await store.refresh(); // reloads the current (week) window fresh
    clock.advance(0);
    await flush();
    const n = ledger.calls.summary.length;

    store.setRange('total'); // was cached pre-ingest; must refetch
    clock.advance(0);
    await flush();
    expect(ledger.calls.summary.length).toBe(n + 1);
  });

  // eventsInserted === 0 does not mean unchanged: keep-max adapters upgrade
  // existing Usage Records in place (a turn's output_tokens growing) while
  // reporting nothing inserted. Every scan must therefore drop the cache,
  // even one the idle gate then swallows.
  it('a zero-insert scan still invalidates the cache', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger();
    const store = await boot(ledger, clock); // total: cached

    store.setRange('week');
    clock.advance(0);
    await flush();

    await store.refresh(); // default scan: nothing inserted → idle-gated
    clock.advance(0);
    await flush();
    const n = ledger.calls.summary.length;

    store.setRange('total'); // must refetch, not replay the cached total
    clock.advance(0);
    await flush();
    expect(ledger.calls.summary.length).toBe(n + 1);
  });

  it('Day range fetches the hourly series; leaving Day clears hourPoints', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger({
      dayPoints: [pt({ bucket: '2026-07-15' }), pt({ bucket: '2026-07-16' })],
      hourPoints: [pt({ bucket: '2026-07-16 09:00' })],
    });
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

  // A custom range covering one day buckets by hour, so it needs the same
  // hourly series the Day preset does.
  it('a single-day custom range fetches the hourly series; widening it drops them', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger({
      dayPoints: [pt({ bucket: '2026-07-15' }), pt({ bucket: '2026-07-16' })],
      hourPoints: [pt({ bucket: '2026-07-16 09:00' })],
    });
    const store = await boot(ledger, clock);

    store.setRange('custom');
    store.setCustomRange('2026-07-16', '2026-07-16');
    clock.advance(300);
    await flush();
    expect(store.getSnapshot().hourPoints).toHaveLength(1);

    store.setCustomRange('2026-07-15', '2026-07-16');
    clock.advance(300);
    await flush();
    expect(store.getSnapshot().hourPoints).toHaveLength(0);
  });

  // Both windows are hourly, so nothing "leaves hourly" — but the held series
  // describes the old day, and its keys miss every bucket of the new one.
  it('drops one day of hours when a single-day window moves to another day', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger({
      dayPoints: [pt({ bucket: '2026-07-15' }), pt({ bucket: '2026-07-16' })],
      hourPoints: [pt({ bucket: '2026-07-16 09:00' })],
    });
    const store = await boot(ledger, clock);

    store.setRange('custom');
    store.setCustomRange('2026-07-16', '2026-07-16');
    clock.advance(300);
    await flush();
    expect(store.getSnapshot().hourPoints).toHaveLength(1);

    // The new day's fetch never lands: the stale day must still be gone, or a
    // failed reload would leave the chart painting 24 empty bars for good.
    ledger.failNext('series', 'kaboom');
    store.setCustomRange('2026-07-15', '2026-07-15');
    clock.advance(300);
    await flush();
    expect(store.getSnapshot().hourPoints).toHaveLength(0);
  });

  // A background scan tick re-reloads the same window; the bars must not blink.
  it('keeps the held hours when the window has not moved', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger({
      dayPoints: [pt({ bucket: '2026-07-15' }), pt({ bucket: '2026-07-16' })],
      hourPoints: [pt({ bucket: '2026-07-16 09:00' })],
    });
    const store = await boot(ledger, clock);
    store.setRange('day');
    clock.advance(0);
    await flush();
    expect(store.getSnapshot().hourPoints).toHaveLength(1);

    const held = store.getSnapshot().hourPoints;
    ledger.emitPricesRebuilt();
    clock.advance(0);
    await flush();
    expect(store.getSnapshot().hourPoints).toBe(held);
  });

  // range/from/to move on the click; summary and the breakdowns move when the
  // reload lands. Between the two the snapshot names one window and holds
  // another's figures — survivable on screen, not in a saved file, so the gap
  // has to be visible to anything that states which window its figures are for.
  it('reports reloading from the range click until that window\'s figures land', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger({
      dayPoints: [pt({ bucket: '2026-07-15' }), pt({ bucket: '2026-07-16' })],
    });
    const store = await boot(ledger, clock);
    expect(store.getSnapshot().reloading).toBe(false);

    ledger.hold('summary');
    store.setRange('week');
    // The window has already moved, before any figure describing it exists.
    expect(store.getSnapshot().range).toBe('week');
    expect(store.getSnapshot().reloading).toBe(true);

    clock.advance(0); // debounce fires, queries go out
    await flush();
    expect(store.getSnapshot().reloading).toBe(true);

    ledger.resolveHeld('summary', 0);
    await flush();
    expect(store.getSnapshot().reloading).toBe(false);
  });

  // The flag gates a button, so latching it would disable that button for the
  // rest of the session. A failed reload reports through fetchError and still
  // settles: the figures on screen are once again the ones the window names.
  it('settles reloading when the reload fails, rather than latching it', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger({ dayPoints: [pt({})] });
    const store = await boot(ledger, clock);

    ledger.failNext('summary', 'boom');
    store.setRange('week');
    expect(store.getSnapshot().reloading).toBe(true);

    clock.advance(0);
    await flush();
    expect(store.getSnapshot().fetchError).toContain('boom');
    expect(store.getSnapshot().reloading).toBe(false);
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
      sources: [{ source: 'claude', eventsInserted: 1, linesSkipped: 0, limitReadings: 0, artifactsUnreadable: 0, unreadableMaxMtime: null, error: null }],
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

  it('shows Goose in Overview only after its Ledger points have positive tokens', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger({
      dayPoints: [pt({ source: 'goose', totalTokens: 0 })],
    });
    const store = await boot(ledger, clock);

    expect(selectVisibleTools(store.getSnapshot(), clock.now()).map((source) => source.key)).toEqual([]);

    ledger.data.dayPoints = [pt({ source: 'goose', totalTokens: 42 })];
    ledger.data.scan = {
      scannedAt: 0,
      sources: [{ source: 'goose', eventsInserted: 1, linesSkipped: 0, limitReadings: 0, artifactsUnreadable: 0, unreadableMaxMtime: null, error: null }],
    };
    await store.refresh();
    clock.advance(0);
    await flush();

    expect(selectVisibleTools(store.getSnapshot(), clock.now()).map((source) => source.key)).toEqual([
      'goose',
    ]);
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

describe('overviewStore profile', () => {
  // 30 local days back including today; clock now = 2026-07-16.
  const fixedStart = Math.floor(new Date(2026, 5, 17).getTime() / 1000);
  const profileCalls = (ledger: ReturnType<typeof makeFakeLedger>) =>
    ledger.calls.summary.filter((c) => (c[0] as { startTs?: number }).startTs === fixedStart);

  it('counts Sessions over a fixed 30-day window that no range change can move', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger({
      summary: { ...makeFakeLedger().data.summary, convs: 37 },
    });
    const store = await boot(ledger, clock);

    // Two on boot: the provisional first paint and the post-scan reconcile.
    expect(profileCalls(ledger)).toHaveLength(2);
    expect(store.getSnapshot().profileSessions).toBe(37);

    store.setRange('day'); // reload runs with day filters — the Profile stays put
    clock.advance(0);
    await flush();
    expect(profileCalls(ledger)).toHaveLength(2);
    expect(store.getSnapshot().profileSessions).toBe(37);
  });

  it('leaves the count unknown when its fetch fails, without failing the series', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger({ dayPoints: [pt({ totalTokens: 500 })] });
    const store = await boot(ledger, clock);

    // A non-idle rescan refetches series + Profile; only the Profile fails.
    ledger.data.scan = {
      scannedAt: 0,
      sources: [{ source: 'claude', eventsInserted: 3, linesSkipped: 0, limitReadings: 0, artifactsUnreadable: 0, unreadableMaxMtime: null, error: null }],
    };
    ledger.failNext('summary', 'no sessions for you');
    await store.refresh();
    clock.advance(0);
    await flush();

    const snap = store.getSnapshot();
    expect(snap.profileSessions).toBeNull();
    expect(snap.allPoints).toHaveLength(1); // the series it travelled with still landed
  });
});

// ---- the window report's input ----

// The same instant fakeClock freezes, so a window derived here is the window
// the store loaded against — the report must be a function of rendered state,
// and a second clock would quietly break that.
const NOW = new Date(2026, 6, 16);

const SETTINGS: Settings = {
  theme: 'system', language: 'en', currency: 'USD', usdRate: 1,
  launchAtLogin: false, autoCheckUpdates: true, firstRunDone: true,
  menuBarRefreshSec: 60,
};

// One row of breakdown('tool') — the Source block's shape. Priced and
// attributed by default so a test overrides only the field it is about.
function sourceRow(key: string, over: Partial<BreakdownRow> = {}): BreakdownRow {
  return {
    key, source: key,
    inputTokens: 10, outputTokens: 5, cacheReadTokens: 20, cacheWriteTokens: 3,
    totalTokens: 38, requests: 1, cost: 0.25, reasoningTokens: null, convs: 1,
    cacheEstimated: false, hasUnpriced: false, unattributedTokens: 0,
    ...over,
  };
}

// A booted store over two priced days of Claude usage, with a window Summary
// worth more than zero so a Cost assertion can tell "carried" from "empty".
async function mountStoreWithUsage() {
  const clock = fakeClock();
  const ledger = makeFakeLedger({
    dayPoints: [pt({ bucket: '2026-07-15', cost: 1.25 }), pt({ bucket: '2026-07-16', cost: 0.5 })],
    toolRows: [sourceRow('claude')],
    summary: { ...makeFakeLedger().data.summary, totalTokens: 76, requests: 2, convs: 2, cost: 1.75 },
  });
  return { store: await boot(ledger, clock), ledger };
}

// Sources in `ctxSources` report usage AND Context; `alsoSeedUsageFor` names
// Sources that appear in the window with usage alone, the way a Source without
// the capability does. Reasoning and Agents stay null throughout: an
// unattributable category must be omitted, not written as 0.
async function mountStoreWithCtxFor(
  ctxSources: string[],
  opts: { alsoSeedUsageFor?: string[] } = {},
) {
  const usageOnly = opts.alsoSeedUsageFor ?? [];
  const clock = fakeClock();
  const ledger = makeFakeLedger({
    dayPoints: [
      ...ctxSources.map((source) =>
        pt({ source, ctxMessages: 8000, ctxSystem: 1200, ctxToolcalls: 900, ctxMcp: 300, ctxSkills: 150 }),
      ),
      ...usageOnly.map((source) => pt({ source })),
    ],
    toolRows: [...ctxSources, ...usageOnly].map((source) => sourceRow(source)),
    // A Bash tool row rides with the exec rows below, the way a real scan
    // writes them: both come from the same `tool_use`, and the Bash leaf is
    // what the exec rows are allocated against.
    ctxTools: ctxSources.flatMap((source) => [
      { source, name: 'Read', estTokens: 600, calls: 4 },
      { source, name: 'mcp__github__list_issues', estTokens: 300, calls: 2 },
      { source, name: 'Bash', estTokens: 300, calls: 3 },
    ]),
    ctxSkills: ctxSources.map((source) => ({ source, name: 'brainstorming', estTokens: 150, uses: 1 })),
    // exe/cmd as the scanner stores them: cmd is already the two-word signature.
    ctxExec: ctxSources.map((source) => ({
      source, kind: 'bash', exe: 'git', cmd: 'git commit', estTokens: 40, calls: 3,
    })),
  });
  return { store: await boot(ledger, clock), ledger };
}

// One Source with more skills than any card would show. The Skills panel folds
// everything past ten into a single "N more" row; the report must not, so the
// count has to cross that boundary or the difference is invisible.
async function mountStoreWithManySkills(count: number) {
  const clock = fakeClock();
  const ledger = makeFakeLedger({
    dayPoints: [pt({ source: 'claude', ctxSkills: 150 })],
    toolRows: [sourceRow('claude')],
    ctxSkills: Array.from({ length: count }, (_, i) => ({
      source: 'claude',
      name: `skill-${String(i).padStart(2, '0')}`,
      estTokens: 1000 - i,
      uses: count - i,
    })),
  });
  return { store: await boot(ledger, clock), ledger };
}

// One day of Unpriced usage — cost 0 with hasUnpriced, the only shape a
// SeriesPoint has for "no priced tokens" — beside a day that really is priced.
async function mountStoreWithUnpricedDay() {
  const unpricedDay = '2026-07-15';
  const clock = fakeClock();
  const ledger = makeFakeLedger({
    dayPoints: [
      pt({ bucket: unpricedDay, cost: 0, hasUnpriced: true }),
      pt({ bucket: '2026-07-16', cost: 2.5 }),
    ],
    toolRows: [sourceRow('claude')],
  });
  return { store: await boot(ledger, clock), ledger, unpricedDay };
}

// A single-day window, where the store holds the very hours on screen.
async function mountStoreOnHourlyDay() {
  const clock = fakeClock();
  const ledger = makeFakeLedger({
    dayPoints: [pt({ bucket: '2026-07-15' }), pt({ bucket: '2026-07-16' })],
    hourPoints: [pt({ bucket: '2026-07-16 14:00' }), pt({ bucket: '2026-07-16 09:00' })],
    toolRows: [sourceRow('claude')],
  });
  const store = await boot(ledger, clock);
  store.setRange('day');
  clock.advance(0);
  await flush();
  return { store, ledger };
}

// A bounded window sitting inside a wider Ledger, with usage outside it on
// both sides: neither reported bound can be the Ledger's own extent.
async function mountStoreOnCustomWindow() {
  const clock = fakeClock();
  const ledger = makeFakeLedger({
    dayPoints: [pt({ bucket: '2026-07-10' }), pt({ bucket: '2026-07-13' }), pt({ bucket: '2026-07-16' })],
    toolRows: [sourceRow('claude')],
  });
  const store = await boot(ledger, clock);
  store.setRange('custom');
  store.setCustomRange('2026-07-12', '2026-07-14');
  clock.advance(300); // custom debounces at 250ms
  await flush();
  return { store, ledger };
}

// A window long enough that the Trend aggregates it to months (>120 days).
async function mountStoreOverManyMonths() {
  const clock = fakeClock();
  const ledger = makeFakeLedger({
    dayPoints: [pt({ bucket: '2026-01-05' }), pt({ bucket: '2026-07-16' })],
    toolRows: [sourceRow('claude')],
  });
  return { store: await boot(ledger, clock), ledger };
}

// A Source holding Unreadable Artifacts of unknown age: nothing bounds their
// content downward, so they could hold usage in any window (ADR-0017).
async function mountStoreWithUnreadableArtifact() {
  const clock = fakeClock();
  const ledger = makeFakeLedger({
    dayPoints: [pt({})],
    toolRows: [sourceRow('claude')],
    scan: {
      scannedAt: 0,
      sources: [{
        source: 'claude', eventsInserted: 0, linesSkipped: 0, limitReadings: 0,
        artifactsUnreadable: 2, unreadableMaxMtime: null, error: null,
      }],
    },
  });
  return { store: await boot(ledger, clock), ledger };
}

describe('selectReportInput', () => {
  it('loads per-Source usage rows with the rest of the window', async () => {
    const { store, ledger } = await mountStoreWithUsage();
    expect(ledger.calls.breakdown.map(([by]) => by)).toContain('tool');
    expect(store.getSnapshot().sourceRows.length).toBeGreaterThan(0);
  });

  it('derives Context for every reporting Source, not only the selected one', async () => {
    const { store } = await mountStoreWithCtxFor(['claude', 'codex']);
    const s = { ...store.getSnapshot(), selected: 'claude' as const };
    const input = selectReportInput(s, selectView(s, NOW), SETTINGS, NOW);
    expect(new Set(input.ctxCategories.map((c) => c.source))).toEqual(new Set(['claude', 'codex']));
    // cmd carries the whole signature already, so both name columns pass
    // through untouched — nothing here splits, strips or re-joins them.
    // est_tokens is allocated, which the next test is about.
    expect(input.ctxExec).toContainEqual(
      expect.objectContaining({ source: 'codex', exe: 'git', cmd: 'git commit', calls: 3 }),
    );
  });

  // The Bash block is the one Context block whose rows arrive as raw content
  // weights: ctx_exec stores sizes, while the panel shows each row's share of
  // the Bash leaf's allocated tokens. Writing the raw figures would put this
  // block on a scale nothing else in the file — or on the screen — is on.
  it('allocates Bash rows against the Bash leaf, as the drill-down does', async () => {
    const { store } = await mountStoreWithCtxFor(['claude']);
    const s = store.getSnapshot();
    const view = selectView(s, NOW);
    const input = selectReportInput(s, view, SETTINGS, NOW);

    const bashLeaf = view.ctxTree.flatMap((c) => c.tools).find((leaf) => leaf.name === 'Bash')!;
    const mine = input.ctxExec.filter((e) => e.source === 'claude');
    expect(mine.reduce((a, e) => a + e.estTokens, 0)).toBe(bashLeaf.tokens);
    // Raw would have been 40 — the stored content size, on the wrong scale.
    expect(mine[0].estTokens).not.toBe(40);

    // And it matches the very rows the panel renders for the same leaf.
    expect(mine.map((e) => e.estTokens)).toEqual(
      execFacets(view.selExecRows, bashLeaf.tokens)!.byCommand.map((f) => f.tokens),
    );
  });

  // ctx_exec groups by kind as well as cmd, and kind reads the raw command
  // line while cmd is the two-word signature — so `npm run build` and
  // `npm run dev` arrive as two rows that both reduce to `npm run`. The block
  // has no kind column, so unmerged they would be two rows under one key with
  // nothing telling them apart.
  it('merges signatures that differ only by the kind column it does not carry', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger({
      dayPoints: [pt({ source: 'claude', ctxToolcalls: 900 })],
      toolRows: [sourceRow('claude')],
      ctxTools: [{ source: 'claude', name: 'Bash', estTokens: 300, calls: 5 }],
      ctxExec: [
        { source: 'claude', kind: 'build', exe: 'npm', cmd: 'npm run', estTokens: 60, calls: 2 },
        { source: 'claude', kind: 'dev_server', exe: 'npm', cmd: 'npm run', estTokens: 40, calls: 3 },
      ],
    });
    const store = await boot(ledger, clock);
    const s = store.getSnapshot();
    const input = selectReportInput(s, selectView(s, NOW), SETTINGS, NOW);

    expect(input.ctxExec).toHaveLength(1);
    expect(input.ctxExec[0]).toMatchObject({ exe: 'npm', cmd: 'npm run', calls: 5 });
  });

  it('leaves out a Source present in the window that reports no Context', async () => {
    // grok has usage in the window but attributes no Context category.
    const { store } = await mountStoreWithCtxFor(['claude'], { alsoSeedUsageFor: ['grok'] });
    const s = store.getSnapshot();
    const input = selectReportInput(s, selectView(s, NOW), SETTINGS, NOW);
    expect(input.ctxCategories.some((c) => c.source === 'grok')).toBe(false);
    expect(input.sources.some((r) => r.key === 'grok')).toBe(true);
    // Same rule one level down: claude reports no Reasoning, so that category
    // is absent rather than present at 0.
    expect(input.ctxCategories.filter((c) => c.source === 'claude').map((c) => c.category))
      .not.toContain('reasoning');
  });

  // The Skills card keeps ten and folds the rest into one nameless row. A
  // report is the one place that fold is wrong: it would drop skills 11..N and
  // write their sum as a row whose first cell is empty, which everywhere else
  // in this file means "unknown".
  it('carries every skill a Source used, past the point the card folds them', async () => {
    const { store } = await mountStoreWithManySkills(12);
    const s = store.getSnapshot();
    const input = selectReportInput(s, selectView(s, NOW), SETTINGS, NOW);
    const mine = input.ctxSkills.filter((sk) => sk.source === 'claude');
    expect(mine).toHaveLength(12);
    expect(mine.map((sk) => sk.name)).toContain('skill-11');
    expect(mine.some((sk) => sk.name === '')).toBe(false);
    // Rows pass through as stored, not summed into a remainder.
    expect(mine).toContainEqual({ source: 'claude', name: 'skill-11', estTokens: 989, uses: 1 });
  });

  it('resolves a fully-Unpriced bucket to an unavailable cost rather than zero', async () => {
    const { store, unpricedDay } = await mountStoreWithUnpricedDay();
    const s = store.getSnapshot();
    const input = selectReportInput(s, selectView(s, NOW), SETTINGS, NOW);
    expect(input.time.find((r) => r.key === unpricedDay)?.cost).toBe(null);
    // Not a blanket null: the priced day keeps its figure.
    expect(input.time.find((r) => r.key === '2026-07-16')?.cost).toBe(2.5);
  });

  // SeriesPoint carries no cache-estimated flag, so a time row cannot know and
  // says so with null, which the file writes as an empty cell. A Source row
  // reads BreakdownRow, which does carry it, and keeps the boolean.
  it('leaves a time row\'s cache_estimated unknown while a Source row states it', async () => {
    const { store } = await mountStoreWithUsage();
    const s = store.getSnapshot();
    const input = selectReportInput(s, selectView(s, NOW), SETTINGS, NOW);
    expect(input.time.every((r) => r.cacheEstimated === null)).toBe(true);
    expect(input.sources.map((r) => r.cacheEstimated)).toEqual([false]);
  });

  // The block's first column and the header's window_grain are the same value,
  // so a test that only read the name would pass against rows of another size:
  // both of these pin the grain AGAINST the keys of the rows beneath it.
  it('reports a single-day window by the hour, the grain the screen is showing', async () => {
    const { store } = await mountStoreOnHourlyDay();
    const s = store.getSnapshot();
    const input = selectReportInput(s, selectView(s, NOW), SETTINGS, NOW);
    expect(input.grain).toBe('hour');
    expect(input.time.map((r) => r.key)).toEqual(['2026-07-16 09:00', '2026-07-16 14:00']);
    // Day is a bounded range: today, not the Ledger's first record.
    expect(input.fromIso).toBe('2026-07-16');

    // Hours that have not landed fall back to the day beneath them rather than
    // naming a grain the block has no rows for: this window is also one day,
    // and its hourly series is still empty.
    const { store: noHours } = await mountStoreWithCtxFor(['claude']);
    const n = noHours.getSnapshot();
    const fallback = selectReportInput(n, selectView(n, NOW), SETTINGS, NOW);
    expect(fallback.grain).toBe('day');
    expect(fallback.time.map((r) => r.key)).toEqual(['2026-07-16']);
  });

  // runReload drops hourPoints that belong to another day, but that runs after
  // the debounce — 250ms for a custom range. In the gap the snapshot carries
  // the new bounds beside the old day's hours, and a report taken then would
  // have named a window of one day over rows keyed to another.
  it('will not report the previous day\'s hours under a window that has moved', async () => {
    const clock = fakeClock();
    const ledger = makeFakeLedger({
      dayPoints: [pt({ bucket: '2026-07-13' }), pt({ bucket: '2026-07-16' })],
      hourPoints: [pt({ bucket: '2026-07-16 09:00' })],
      toolRows: [sourceRow('claude')],
    });
    const store = await boot(ledger, clock);
    store.setRange('day');
    clock.advance(0);
    await flush();
    expect(store.getSnapshot().hourPoints.map((p) => p.bucket)).toEqual(['2026-07-16 09:00']);

    // Move to a different single day and read the snapshot mid-debounce: the
    // hours held are still the 16th's.
    store.setRange('custom');
    store.setCustomRange('2026-07-13', '2026-07-13');
    const s = store.getSnapshot();
    expect(s.hourPoints[0].bucket.slice(0, 10)).toBe('2026-07-16');

    const input = selectReportInput(s, selectView(s, NOW), SETTINGS, NOW);
    expect(input.fromIso).toBe('2026-07-13');
    expect(input.grain).toBe('day');
    expect(input.time.map((r) => r.key)).toEqual(['2026-07-13']);
  });

  it('never names the time block week or month: the rows are the days the file keeps', async () => {
    const { store } = await mountStoreOverManyMonths();
    const s = store.getSnapshot();
    const view = selectView(s, NOW);
    // The chart aggregates a window this long into months...
    expect(view.per).toBe('month');
    // ...the file does not: it keeps the finest grain it holds and says so, so
    // a spreadsheet can roll the days up and nothing is destroyed on the way out.
    const input = selectReportInput(s, view, SETTINGS, NOW);
    expect(input.grain).toBe('day');
    expect(input.time.map((r) => r.key)).toEqual(['2026-01-05', '2026-07-16']);
  });

  it('carries the Display Currency without moving figures off USD', async () => {
    const { store } = await mountStoreWithUsage();
    const s = store.getSnapshot();
    const view = selectView(s, NOW);
    const aud = selectReportInput(s, view, { ...SETTINGS, currency: 'AUD', usdRate: 1.52 }, NOW);
    expect(aud.displayCurrency).toBe('AUD');
    expect(aud.usdRate).toBe(1.52);

    const usd = selectReportInput(s, view, SETTINGS, NOW);
    expect(usd.displayCurrency).toBe(null);
    expect(usd.usdRate).toBe(null);
    expect(usd.summary.cost).toBe(aud.summary.cost);
  });

  it('takes a bounded window from the range, not the Ledger extent around it', async () => {
    const { store } = await mountStoreOnCustomWindow();
    const s = store.getSnapshot();
    const input = selectReportInput(s, selectView(s, NOW), SETTINGS, NOW);
    // The Ledger runs wider on both sides, so neither reported bound can be
    // the extent falling through: this is the file's primary claim about what
    // it covers, and Total alone would only ever exercise the fallback.
    expect([s.firstIso, s.lastIso]).toEqual(['2026-07-10', '2026-07-16']);
    expect(input.fromIso).toBe('2026-07-12');
    expect(input.toIso).toBe('2026-07-14');
    // The Summary row is labelled from the same pair, so the headline can
    // never name a window other than the one the header states.
    expect(input.summary.key).toBe('2026-07-12 .. 2026-07-14');
    // And the rows sit inside it: the day either side is excluded.
    expect(input.time.map((r) => r.key)).toEqual(['2026-07-13']);
  });

  it("takes a Total window's dates from the Ledger's own extent", async () => {
    const { store } = await mountStoreWithUsage();
    const s = { ...store.getSnapshot(), range: 'total' as const };
    const input = selectReportInput(s, selectView(s, NOW), SETTINGS, NOW);
    expect(input.fromIso).toBe(s.firstIso);
    expect(input.toIso).toBe(s.lastIso);
  });

  it('marks the window a floor when an Unreadable Artifact could reach it', async () => {
    const { store } = await mountStoreWithUnreadableArtifact();
    const s = store.getSnapshot();
    const input = selectReportInput(s, selectView(s, NOW), SETTINGS, NOW);
    expect(input.tokensBasis).toBe('floor');

    // The marker is earned, not constant: the same window with nothing
    // unreadable reports its tokens exact.
    const { store: clean } = await mountStoreWithUsage();
    const c = clean.getSnapshot();
    expect(selectReportInput(c, selectView(c, NOW), SETTINGS, NOW).tokensBasis).toBe('exact');
  });
});
