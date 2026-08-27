/** @vitest-environment jsdom */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';
import TrayPanel, { barRect } from './TrayPanel';
import { SOURCE_ICONS } from '../overview/icons';
import type { Platform } from '../lib/platform';
import { makeFakeLedger } from '../overview/ledger.fake';
import { makeFakeSettings } from '../settings/settings.fake';
import { makeFakeEstimate } from '../limits/limits.fake';
import {
  LIVE_ENABLED_KEY,
  lastCheckKey,
  lastFailureKey,
  PANEL_WINDOWS_KEY,
  type LimitsPort,
} from '../limits/limits';
import type { BreakdownRow, SourceLimits, Summary } from '../types';
import { seriesPoint } from '../overview/seriesPoint';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

// The tray's "I just showed the panel" event is Tauri glue, so jsdom never
// delivers it; this stands in for the IPC and hands the callbacks back so a
// test can fire them. `listen` is all anything here imports from the module.
const { panelShown } = vi.hoisted(() => ({ panelShown: new Set<() => void>() }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: (name: string, cb: () => void) => {
    if (name === 'panel-shown') panelShown.add(cb);
    return Promise.resolve(() => panelShown.delete(cb));
  },
}));

// The action rows' fire-and-forget ipc() goes through invoke; recording the
// command names is how a test sees Settings…, Quit and Escape leave the panel.
// The data ports don't pass through here — tests hand in fakes.
const { invoked } = vi.hoisted(() => ({ invoked: [] as string[] }));
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string) => {
    invoked.push(cmd);
    return Promise.resolve();
  },
}));

const summary: Summary = {
  inputTokens: 10, outputTokens: 5, cacheReadTokens: 20, cacheWriteTokens: 3,
  totalTokens: 3_400_000, requests: 1912, cost: 12.84, hasUnpriced: false,
  unattributedTokens: 0, unpricedModels: [], cacheEstimatedModels: [], cacheHitRate: 0, convs: 0,
};

const sourceRows: BreakdownRow[] = [
  { key: 'claude', source: null, inputTokens: 0, outputTokens: 0, cacheReadTokens: 0,
    cacheWriteTokens: 0, totalTokens: 1_800_000, requests: 0, cost: 6.12,
    reasoningTokens: null, convs: 0, cacheEstimated: false, hasUnpriced: false,
    unattributedTokens: 0 },
  { key: 'codex', source: null, inputTokens: 0, outputTokens: 0, cacheReadTokens: 0,
    cacheWriteTokens: 0, totalTokens: 238_100, requests: 0, cost: 1.11,
    reasoningTokens: null, convs: 0, cacheEstimated: false, hasUnpriced: false,
    unattributedTokens: 0 },
];

// Series helpers for the chart tests: a local calendar day `back` days
// ago, and one (bucket, Source) series point.
const day = (back: number) => {
  const d = new Date();
  d.setDate(d.getDate() - back);
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
};
const point = (bucket: string, cost: number) => seriesPoint({ bucket, cost, totalTokens: 1_000 });

// A silent limits port: tests that are not about the Limit cards pass one so the
// default port's IPC read never lands in `invoked`, and the limits tests seed
// it with stored Readings. `checked` records live checks the way the ledger
// fake records scans.
function makeFakeLimits(
  stored: SourceLimits[] = [],
  store: Record<string, string> = {},
): LimitsPort & { checked: string[] } {
  const map = new Map(Object.entries(store));
  const checked: string[] = [];
  return {
    checked,
    list: () => Promise.resolve(stored),
    checkLive: (s: string) => {
      checked.push(s);
      return Promise.resolve();
    },
    scan: () => Promise.resolve(),
    onLimitsChanged: () => () => {},
    read: (k) => map.get(k) ?? null,
    write: (k, v) => {
      map.set(k, v);
    },
  };
}

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
  invoked.length = 0;
});

describe('TrayPanel', () => {
  it('renders the panel from the Ledger: header, source bar and legend, actions', async () => {
    const ledger = makeFakeLedger({ summary, modelRows: sourceRows });
    const settings = makeFakeSettings();

    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => {
      root.render(<TrayPanel ports={{ ledger, settings }} platform="macos" />);
    });
    await settle();

    // Header: big cost + tokens/requests sub. Both summary calls hit the
    // ledger (today + yesterday-so-far); the fake serves the same canned
    // summary, so the pace delta computes to +0.0% and stays shown. Twice
    // over, because an open paints from the Ledger and then re-reads behind
    // its own scan.
    expect(container.querySelector('.tp-cost')?.textContent).toBe('$12.84');
    // Tokens only: requests live in their stat tile, not twice on the header.
    expect(container.querySelector('.tp-sub')?.textContent).toBe('3.4M tokens');
    expect(ledger.calls.summary.length).toBe(4);

    // Sources from breakdown('tool'), cost desc: the stacked bar splits the
    // priced Cost by brand colour, and the legend names each slice.
    const slices = Array.from(container.querySelectorAll('.tp-srcbar span')).map(
      (s) => (s as HTMLElement).style.width,
    );
    expect(slices.length).toBe(2);
    expect(parseFloat(slices[0])).toBeCloseTo((6.12 / 7.23) * 100, 3);
    const legend = Array.from(container.querySelectorAll('.tp-leg')).map((r) => [
      r.querySelector('.tp-leg-name')?.textContent,
      r.querySelector('.tp-leg-cost')?.textContent,
    ]);
    expect(legend).toEqual([
      ['Claude', '$6.12'],
      ['Codex', '$1.11'],
    ]);
    expect(ledger.calls.breakdown[0]?.[0]).toBe('tool');

    // The three icon actions (Rescan lives in the top-right refresh button).
    const actions = Array.from(container.querySelectorAll('.tp-action')).map((b) => [
      b.querySelector('.tp-act-k')?.textContent,
      b.getAttribute('title'),
    ]);
    expect(actions).toEqual([
      ['Open', 'Open TokenLedger'],
      ['Settings', 'Settings (⌘,)'],
      ['Quit', 'Quit TokenLedger (⌘Q)'],
    ]);
    expect(container.querySelector('.tp-refresh')?.getAttribute('title')).toBe('Rescan now (⇧⌘R)');
  });

  // Unreadable Artifacts (ADR-0017): the panel's token figure is a floor when
  // one could hold usage in the selected window — the same ≥ the bar title and
  // the Overview carry, read from the persisted per-scan state (see api.ts), so
  // it is honest from launch rather than only after a scan has run.
  it('marks the token figure ≥ when an Unreadable Artifact could touch the window', async () => {
    const ledger = makeFakeLedger({
      summary,
      modelRows: sourceRows,
      scan: {
        scannedAt: 0,
        ingestRev: 0,
        sources: [{
          source: 'antigravity', eventsInserted: 0, linesSkipped: 0, limitReadings: 0,
          artifactsUnreadable: 100,
          unreadableMaxMtime: Math.floor(Date.now() / 1000),
          error: null,
        }],
      },
    });
    const settings = makeFakeSettings();

    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => {
      root.render(<TrayPanel ports={{ ledger, settings }} />);
    });
    await settle();

    const sub = container.querySelector('.tp-sub')!;
    expect(sub.textContent).toBe('≥ 3.4M tokens');
    expect(sub.getAttribute('title')).toBe('Antigravity: 100 sessions unreadable');
  });

  // TOKL-25's settled shape: Unbooked Requests mark NOTHING here — the panel
  // no longer reads them at all. The count lives in the Overview's scan
  // footer; the Menu Bar Extra's figures stay bare.
  //
  // Mutation pin: wire unbookedRequests back into tokensFloor (the one-release
  // floor this replaces) and both bare-figure assertions fail.
  it('leaves every figure bare however many Requests are unbooked', async () => {
    const now = Math.floor(Date.now() / 1000);
    const ledger = makeFakeLedger({
      summary,
      modelRows: sourceRows,
      unbookedRequests: [{ source: 'qoder', requests: 628, firstAt: now - 60, lastAt: now }],
    });
    const settings = makeFakeSettings();

    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => {
      root.render(<TrayPanel ports={{ ledger, settings }} />);
    });
    await settle();

    const sub = container.querySelector('.tp-sub')!;
    expect(sub.textContent).toBe('3.4M tokens');
    expect(sub.getAttribute('title')).toBeNull();

    const tiles = Array.from(container.querySelectorAll('.tp-tile')).map((t) => [
      t.querySelector('.tp-tile-v')?.textContent,
      t.querySelector('.tp-tile-k')?.textContent,
    ]);
    expect(tiles).toEqual([
      ['0.0%', 'cache hit'],
      ['1,912', 'requests'],
    ]);
  });

  it('leaves the Requests tile bare when nothing is unbooked', async () => {
    const ledger = makeFakeLedger({ summary, modelRows: sourceRows, unbookedRequests: [] });
    const settings = makeFakeSettings();

    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => {
      root.render(<TrayPanel ports={{ ledger, settings }} />);
    });
    await settle();

    const tiles = Array.from(container.querySelectorAll('.tp-tile')).map(
      (t) => t.querySelector('.tp-tile-v')?.textContent,
    );
    expect(tiles).toEqual(['0.0%', '1,912']);
  });

  it('renders the Models section and the stat tiles from the extra reads', async () => {
    const ledger = makeFakeLedger({
      summary: { ...summary, cacheHitRate: 0.9124 },
      sourceRows,
      modelRows: [
        { ...sourceRows[0], key: 'claude-sonnet-4-5', source: 'claude', cost: 6.12 },
        { ...sourceRows[1], key: 'gpt-5-codex', source: 'codex', cost: 1.11 },
      ],
      lastScan: Math.floor(Date.now() / 1000) - 132,
    });
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => root.render(<TrayPanel ports={{ ledger, settings: makeFakeSettings() }} />));
    await settle();

    // Sources stay their own section; Models come from breakdown('model'),
    // each row led by its owning Source's mark.
    expect(
      Array.from(container.querySelectorAll('.tp-leg-name')).map((e) => e.textContent),
    ).toEqual(['Claude', 'Codex']);
    const models = Array.from(container.querySelectorAll('.tp-models .tp-row')).map((r) => [
      r.querySelector('.tp-row-label')?.textContent,
      r.querySelector('.tp-row-cost')?.textContent,
      r.querySelector('img')?.getAttribute('src'),
    ]);
    expect(models).toEqual([
      ['claude-sonnet-4-5', '$6.12', SOURCE_ICONS.claude],
      ['gpt-5-codex', '$1.11', SOURCE_ICONS.codex],
    ]);

    const tiles = Array.from(container.querySelectorAll('.tp-tile')).map((s) => [
      s.querySelector('.tp-tile-v')?.textContent,
      s.querySelector('.tp-tile-k')?.textContent,
    ]);
    expect(tiles).toEqual([
      ['91.2%', 'cache hit'],
      ['1,912', 'requests'],
    ]);
    // The scan time moved beside Rescan (TOKL-19) — the same string the third
    // tile used to carry, now quiet mono in the top bar.
    expect(container.querySelector('.tp-top-right .tp-scanned')?.textContent).toBe('2 min ago');

    // The extra reads: Models, the period's series, the Scan time — each
    // twice over, because an open paints from the Ledger and then re-reads
    // behind its own scan.
    expect(ledger.calls.breakdown.map((c) => c[0])).toEqual(['tool', 'model', 'tool', 'model']);
    expect(ledger.calls.series[0]?.[1]).toBe('hour'); // Today buckets hourly
    expect(ledger.calls.lastScan.length).toBe(2);
  });

  it('draws the bar chart for the selected period and rebuckets on a switch', async () => {
    const ledger = makeFakeLedger({
      summary,
      sourceRows,
      dayPoints: [point(day(1), 3), point(day(0), 9)],
    });
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => root.render(<TrayPanel ports={{ ledger, settings: makeFakeSettings() }} />));
    await settle();

    const days30 = Array.from(container.querySelectorAll('.tp-seg-btn')).find(
      (b) => b.textContent === '30 days',
    ) as HTMLButtonElement;
    await act(async () => days30.click());
    await settle();

    const lastSeries = ledger.calls.series[ledger.calls.series.length - 1];
    expect(lastSeries?.[1]).toBe('day'); // 30 days buckets daily
    const ticks = Array.from(container.querySelectorAll('.tp-chart-cap span')).map((s) => s.textContent);
    expect(ticks).toEqual([day(29).slice(5), day(15).slice(5), day(0).slice(5)]); // the axis
    expect(container.querySelector('.tp-chart-peak')?.textContent).toBe(`peak ${day(0).slice(5)} · $9.00`);
    // Two buckets carry Cost: the peak bucket wears the brighter fill, the
    // other draws in the base one — one column each.
    expect(container.querySelector('.tp-bars')?.getAttribute('d')).toMatch(/^M\d/);
    expect((container.querySelector('.tp-bars')?.getAttribute('d')?.match(/M/g) ?? []).length).toBe(1);
    expect((container.querySelector('.tp-bar-peak')?.getAttribute('d')?.match(/M/g) ?? []).length).toBe(1);
  });

  // A day 40 minutes old has one bucket; its column must stay a column
  // rather than flooding the whole 288px box (uncapped it would be 286 wide).
  it('clamps a lone bucket to a modest column', () => {
    expect(barRect([1], 0)).toContain('h28.0');
  });

  it('hovering the chart reads out that bucket; leaving clears the read-out', async () => {
    const ledger = makeFakeLedger({
      summary,
      sourceRows,
      dayPoints: [point(day(1), 3), point(day(0), 9)],
    });
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => root.render(<TrayPanel ports={{ ledger, settings: makeFakeSettings() }} />));
    await settle();
    const days30 = Array.from(container.querySelectorAll('.tp-seg-btn')).find(
      (b) => b.textContent === '30 days',
    ) as HTMLButtonElement;
    await act(async () => days30.click());
    await settle();

    // Idle: the reserved line stays empty; the peak caption holds the row.
    expect(container.querySelector('.tp-chart-read')?.textContent).toBe('');
    expect(container.querySelector('.tp-chart-hover')).toBeNull();

    // jsdom boxes have no size; give the svg the viewBox's width so the
    // pointer maths has geometry to invert.
    const svg = container.querySelector('.tp-chart svg') as SVGSVGElement;
    svg.getBoundingClientRect = () =>
      ({ left: 0, top: 0, width: 288, height: 56, right: 288, bottom: 56, x: 0, y: 0 }) as DOMRect;

    // The right edge is the latest bucket (today, $9); the left edge day 29.
    await act(async () => {
      svg.dispatchEvent(new MouseEvent('mousemove', { bubbles: true, clientX: 287 }));
    });
    expect(container.querySelector('.tp-chart-read')?.textContent).toBe(
      `${day(0).slice(5)} · $9.00 · 1K tok`,
    );
    // The peak caption yields the row to the read-out — side by side it
    // squeezed the detail into an ellipsis.
    expect(container.querySelector('.tp-chart-peak')).toBeNull();
    expect(container.querySelector('.tp-chart-hover')).not.toBeNull();
    await act(async () => {
      svg.dispatchEvent(new MouseEvent('mousemove', { bubbles: true, clientX: 3 }));
    });
    expect(container.querySelector('.tp-chart-read')?.textContent).toBe(
      `${day(29).slice(5)} · $0.00 · 0 tok`, // an idle day reads its zero
    );

    // React synthesizes onMouseLeave from bubbling mouseout whose
    // relatedTarget lies outside the element.
    await act(async () => {
      svg.dispatchEvent(
        new MouseEvent('mouseout', { bubbles: true, relatedTarget: document.body }),
      );
    });
    expect(container.querySelector('.tp-chart-read')?.textContent).toBe('');
    expect(container.querySelector('.tp-chart-hover')).toBeNull();
    expect(container.querySelector('.tp-chart-peak')).not.toBeNull(); // the caption returns
  });

  // The cap is on the panel, not on the Ledger: the bar keeps every slice, the
  // legend names three, and the count says where the rest are. A month with
  // eight Sources is what grew the card past a laptop's work area.
  it('names three Sources in the legend and counts the rest, bar still whole', async () => {
    const many = ['claude', 'codex', 'grok', 'gemini', 'qoder'].map((key, i) => ({
      ...sourceRows[0],
      key,
      totalTokens: 1_000_000 - i * 1_000,
      cost: 6 - i,
    }));
    const ledger = makeFakeLedger({ summary, modelRows: many });
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => root.render(<TrayPanel ports={{ ledger, settings: makeFakeSettings() }} />));
    await settle();

    expect(
      Array.from(container.querySelectorAll('.tp-leg-name')).map((e) => e.textContent),
    ).toEqual(['Claude', 'Codex', 'Grok']);
    expect(container.querySelector('.tp-sources .tp-more')?.textContent).toBe(
      '+2 more Sources in the app',
    );
    // Five slices for five Sources — capping the bar would misstate the split.
    expect(container.querySelectorAll('.tp-srcbar span').length).toBe(5);
  });

  it('renders lowercase pi with the official mark in the Menu Bar Extra', async () => {
    const piRow: BreakdownRow = {
      ...sourceRows[0],
      key: 'pi',
      totalTokens: 239,
      requests: 3,
      cost: 0.000805,
    };
    const ledger = makeFakeLedger({
      summary: { ...summary, totalTokens: 239, requests: 3, cost: 0.000805 },
      modelRows: [piRow],
    });
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => root.render(<TrayPanel ports={{ ledger, settings: makeFakeSettings() }} />));
    await settle();

    const leg = container.querySelector('.tp-leg')!;
    expect(leg.querySelector('.tp-leg-name')?.textContent).toBe('pi');
    expect(leg.querySelector('img')?.getAttribute('src')).toMatch(/^data:image\/svg\+xml/);
  });

  it('renders all-Unattributed headline and Source Cost as unavailable', async () => {
    const ledger = makeFakeLedger({
      summary: { ...summary, totalTokens: 50, requests: 1, cost: null, unattributedTokens: 50 },
      modelRows: [{ ...sourceRows[0], totalTokens: 50, cost: null, unattributedTokens: 50 }],
    });
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => root.render(<TrayPanel ports={{ ledger, settings: makeFakeSettings() }} />));
    await settle();

    expect(container.querySelector('.tp-cost')?.textContent).toBe('unavailable');
    expect(container.querySelector('.tp-leg-cost')?.textContent).toBe('unavailable');
    // No priced Cost anywhere: the stacked bar has nothing to split.
    expect(container.querySelector('.tp-srcbar')).toBeNull();
  });

  it('reads out its zero — no sections, no fabricated 0.0% cache hit', async () => {
    const ledger = makeFakeLedger({
      summary: { ...summary, totalTokens: 0, requests: 0, cost: 0 },
      lastScan: Math.floor(Date.now() / 1000) - 60,
    });
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => root.render(<TrayPanel ports={{ ledger, settings: makeFakeSettings() }} />));
    await settle();

    expect(container.querySelector('.tp-cost')?.textContent).toBe('$0.00');
    expect(container.querySelector('.tp-sub')?.textContent).toBe('0 tokens');
    expect(container.querySelector('.tp-delta')).toBeNull();
    expect(container.querySelector('.tp-tiles')).toBeNull();
    expect(container.querySelector('.tp-models')).toBeNull();
    expect(container.querySelector('.tp-chart')).toBeNull();
  });

  // The panel is created on demand and destroyed on dismissal (ADR-0007), so
  // its mount is an open — and an open rescans, or the figures stay as old as
  // the last scan and Rescan is the only way to current ones. Holding the scan
  // pins the other half: the paint does not wait on it.
  it('rescans on open, without holding the paint behind the scan', async () => {
    const ledger = makeFakeLedger({ summary, modelRows: sourceRows });
    ledger.hold('scan');

    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => {
      root.render(<TrayPanel ports={{ ledger, settings: makeFakeSettings() }} />);
    });
    await settle();

    // Painted from what the Ledger already held, with the scan still in flight
    // — and the figures pulse to say so.
    expect(ledger.calls.scan.length).toBe(1);
    expect(container.querySelector('.tp-cost')?.textContent).toBe('$12.84');
    expect(container.querySelector('.tp-skel')).toBeNull();
    expect(ledger.calls.summary.length).toBe(2);
    expect(container.querySelector('.tp-figures.tp-pulse')).not.toBeNull();

    // The scan lands: re-read, so anything it added shows without a press.
    await act(async () => ledger.resolveHeld('scan', 0));
    await settle();

    expect(ledger.calls.summary.length).toBe(4);
    expect(container.querySelector('.tp-pulse')).toBeNull();
  });

  // A scan that fails must not take the panel with it: the figures still land,
  // and the gate still releases — a stranded one would leave Rescan disabled
  // for the life of the window and swallow every later open.
  it('survives a failed scan — still paints, and Rescan still works after', async () => {
    const ledger = makeFakeLedger({ summary, modelRows: sourceRows });
    ledger.failNext('scan', new Error('scan blew up'));
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => {
      root.render(<TrayPanel ports={{ ledger, settings: makeFakeSettings() }} />);
    });
    await settle();

    expect(container.querySelector('.tp-cost')?.textContent).toBe('$12.84');
    const rescanBtn = container.querySelector('.tp-refresh') as HTMLButtonElement;
    expect(rescanBtn.getAttribute('aria-disabled')).toBeNull();

    await act(async () => rescanBtn.click());
    await settle();
    expect(ledger.calls.scan.length).toBe(2); // the gate let a second one through
  });

  // A window the tray reshows instead of rebuilding gets no mount, so the
  // scan-on-open has to hang off panel-shown too.
  it('rescans again when the tray reshows an existing panel', async () => {
    const ledger = makeFakeLedger({ summary, modelRows: sourceRows });
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => {
      root.render(<TrayPanel ports={{ ledger, settings: makeFakeSettings() }} />);
    });
    await settle();

    const scansBefore = ledger.calls.scan.length;
    expect(panelShown.size).toBe(1); // the panel is listening at all
    await act(async () => {
      for (const cb of panelShown) cb();
    });
    await settle();

    expect(ledger.calls.scan.length).toBe(scansBefore + 1);
  });

  it('Rescan runs the scan through the ledger port and refetches', async () => {
    const ledger = makeFakeLedger({ summary, modelRows: sourceRows });
    const settings = makeFakeSettings();
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => {
      root.render(<TrayPanel ports={{ ledger, settings }} />);
    });
    await settle();

    const rescan = container.querySelector('.tp-refresh') as HTMLButtonElement;
    const summariesBefore = ledger.calls.summary.length;
    const scansBefore = ledger.calls.scan.length; // the open scanned already
    await act(async () => rescan.click());
    await settle();

    expect(ledger.calls.scan.length).toBe(scansBefore + 1);
    expect(ledger.calls.summary.length).toBe(summariesBefore + 2); // refetched
  });

  it('Rescan spins while the scan runs and settles when it lands', async () => {
    const ledger = makeFakeLedger({ summary, modelRows: sourceRows });
    const settings = makeFakeSettings();
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => {
      root.render(<TrayPanel ports={{ ledger, settings }} />);
    });
    await settle();

    // Hold only from here, so the open's own scan has already landed and the
    // held one is the button's.
    ledger.hold('scan');
    const scansBefore = ledger.calls.scan.length;
    const rescan = container.querySelector('.tp-refresh') as HTMLButtonElement;
    await act(async () => rescan.click());

    // In flight: the icon spins, the button reads disabled (aria, so its
    // hotkey-hint tooltip survives — a truly disabled control shows none),
    // a second click coalesces, and the Today figures pulse so an unchanged
    // total still reads as a refresh happening.
    expect(container.querySelector('.tp-spin')).not.toBeNull();
    expect(container.querySelector('.tp-figures.tp-pulse')).not.toBeNull();
    expect(rescan.getAttribute('aria-disabled')).toBe('true');
    await act(async () => rescan.click());
    expect(ledger.calls.scan.length).toBe(scansBefore + 1);

    await act(async () => ledger.resolveHeld('scan', 0));
    await settle();
    expect(container.querySelector('.tp-spin')).toBeNull();
    expect(container.querySelector('.tp-pulse')).toBeNull();
    expect(rescan.getAttribute('aria-disabled')).toBeNull();
  });

  it('period segments switch the fetched window and refetch without the skeleton', async () => {
    const ledger = makeFakeLedger({ summary, modelRows: sourceRows });
    const settings = makeFakeSettings();
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => {
      root.render(<TrayPanel ports={{ ledger, settings }} />);
    });
    await settle();

    const segs = () => Array.from(container.querySelectorAll('.tp-seg-btn')) as HTMLButtonElement[];
    expect(segs().map((b) => b.textContent)).toEqual(['Today', 'Yesterday', '30 days']);
    expect(segs()[0].classList.contains('active')).toBe(true);

    const before = ledger.calls.summary.length;
    await act(async () => segs()[1].click());
    await settle();

    expect(segs()[1].classList.contains('active')).toBe(true);
    expect(container.querySelector('.tp-skel')).toBeNull(); // snappy, no beat
    expect(ledger.calls.summary.length).toBe(before + 2);

    // The current window is yesterday's full local day, end-exclusive.
    const now = new Date();
    const mid = (o: number) =>
      Math.floor(new Date(now.getFullYear(), now.getMonth(), now.getDate() + o).getTime() / 1000);
    const filters = ledger.calls.summary[before][0] as { startTs: number; endTs: number };
    expect(filters.startTs).toBe(mid(-1));
    expect(filters.endTs).toBe(mid(0));
  });

  it('shows the loading skeleton while the panel load is in flight', async () => {
    const ledger = makeFakeLedger({ summary, modelRows: sourceRows });
    const settings = makeFakeSettings();
    ledger.hold('summary');

    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => {
      root.render(<TrayPanel ports={{ ledger, settings }} />);
    });
    await settle();

    // In flight: shimmer blocks instead of figures; actions stay usable.
    expect(container.querySelectorAll('.tp-skel').length).toBeGreaterThan(0);
    expect(container.querySelector('.tp-cost')).toBeNull();
    expect(container.querySelectorAll('.tp-action').length).toBe(3);
    expect(container.querySelector('.tp-refresh')).not.toBeNull();

    await act(async () => {
      ledger.resolveHeld('summary', 0);
      ledger.resolveHeld('summary', 1);
    });
    await settle();
    expect(container.querySelector('.tp-skel')).toBeNull();
    expect(container.querySelector('.tp-cost')?.textContent).toBe('$12.84');
  });

  // The panel opens on macOS and Windows (ADR-0010), and ⌘ is not a key a
  // Windows keyboard has. The hints ride the buttons' tooltips now that the
  // actions are icons.
  it('spells the action shortcuts the way the platform does', async () => {
    const keys = async (platform: Platform) => {
      const container = document.createElement('div');
      document.body.append(container);
      const root = createRoot(container);
      mountedRoots.push(root);
      await act(async () => {
        root.render(
          <TrayPanel
            ports={{ ledger: makeFakeLedger({ summary, modelRows: sourceRows }), settings: makeFakeSettings() }}
            platform={platform}
          />,
        );
      });
      await settle();
      return Array.from(container.querySelectorAll('.tp-refresh, .tp-action'))
        .map((b) => b.getAttribute('title')?.match(/\(([^)]+)\)$/)?.[1])
        .filter(Boolean);
    };

    expect(await keys('macos')).toEqual(['⇧⌘R', '⌘,', '⌘Q']);
    expect(await keys('windows')).toEqual(['Ctrl+Shift+R', 'Ctrl+,', 'Ctrl+Q']);
    expect(await keys('linux')).toEqual(['Ctrl+Shift+R', 'Ctrl+,', 'Ctrl+Q']);
  });

  // The translucent card is scoped to body.tp-macos, because only macOS gets a
  // material painted behind it (see TrayPanel.css's header). Without this class
  // the panel is either solid everywhere or see-through everywhere.
  it('names the platform on the body, so the card lightens only where a material backs it', async () => {
    const mount = async (platform: Platform) => {
      const container = document.createElement('div');
      document.body.append(container);
      const root = createRoot(container);
      await act(async () => {
        root.render(
          <TrayPanel
            ports={{ ledger: makeFakeLedger({ summary, modelRows: sourceRows }), settings: makeFakeSettings() }}
            platform={platform}
          />,
        );
      });
      await settle();
      const classes = Array.from(document.body.classList);
      act(() => root.unmount());
      return classes;
    };

    expect(await mount('macos')).toContain('tp-macos');
    expect(await mount('windows')).not.toContain('tp-macos');
    // and the class comes back off with the panel
    expect(Array.from(document.body.classList)).not.toContain('tp-macos');
  });

  // A shortcut the panel prints beside an action is a working promise
  // (CONTEXT.md): each key fires the very handler its button does. The panel
  // has no focused element, so the keys land on the document.
  describe('keyboard', () => {
    const mount = async (
      platform: Platform,
      ledger = makeFakeLedger({ summary, modelRows: sourceRows }),
    ) => {
      const container = document.createElement('div');
      document.body.append(container);
      const root = createRoot(container);
      mountedRoots.push(root);
      await act(async () => {
        root.render(
          <TrayPanel
            ports={{ ledger, settings: makeFakeSettings(), limits: makeFakeLimits() }}
            platform={platform}
          />,
        );
      });
      await settle();
      return ledger;
    };
    const key = async (init: KeyboardEventInit) => {
      await act(async () => {
        document.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, ...init }));
      });
      await settle();
    };

    it('⇧⌘R rescans, exactly like the button', async () => {
      const ledger = await mount('macos');
      const before = ledger.calls.scan.length;
      await key({ key: 'R', metaKey: true, shiftKey: true });
      expect(ledger.calls.scan.length).toBe(before + 1);
    });

    // The key is the button, so it meets the same gate: held on the key it
    // coalesces instead of stacking scans, and the panel stays open saying so.
    it('⇧⌘R held down coalesces, like the button does', async () => {
      const ledger = await mount('macos');
      ledger.hold('scan');
      const before = ledger.calls.scan.length;
      await key({ key: 'R', metaKey: true, shiftKey: true });
      await key({ key: 'R', metaKey: true, shiftKey: true });

      expect(ledger.calls.scan.length).toBe(before + 1);
      expect(document.querySelector('.tp-figures.tp-pulse')).not.toBeNull();
      expect(invoked).not.toContain('close_panel'); // rescanning never dismisses
    });

    it('⌘, opens Settings through the button\'s own IPC', async () => {
      await mount('macos');
      await key({ key: ',', metaKey: true });
      expect(invoked).toContain('open_settings');
    });

    it('⌘Q quits through the button\'s own IPC', async () => {
      await mount('macos');
      await key({ key: 'q', metaKey: true });
      expect(invoked).toContain('quit_app');
    });

    it('Escape dismisses the panel the way focus loss does', async () => {
      await mount('macos');
      await key({ key: 'Escape' });
      expect(invoked).toContain('close_panel');
    });

    // Windows and Linux have no ⌘, so the keys are the Ctrl ones the hints
    // there spell — and each platform ignores the other's modifier, or a
    // Super/Win ⌘Q would quit the app from under a window-switching keystroke.
    it.each([['windows' as Platform], ['linux' as Platform]])(
      'binds the Ctrl keys on %s, and ignores ⌘',
      async (platform) => {
        const ledger = await mount(platform);
        const scans = ledger.calls.scan.length;
        await key({ key: 'R', ctrlKey: true, shiftKey: true });
        await key({ key: ',', ctrlKey: true });
        await key({ key: 'q', ctrlKey: true });
        expect(ledger.calls.scan.length).toBe(scans + 1);
        expect(invoked).toContain('open_settings');
        expect(invoked).toContain('quit_app');

        // The macOS spelling does nothing here.
        invoked.length = 0;
        const after = ledger.calls.scan.length;
        await key({ key: 'R', metaKey: true, shiftKey: true });
        await key({ key: ',', metaKey: true });
        await key({ key: 'q', metaKey: true });
        expect(ledger.calls.scan.length).toBe(after);
        expect(invoked).toEqual([]);
      },
    );

    // The promise itself (CONTEXT.md): whatever the panel *prints* beside an
    // action is what fires it. Read back from the rendered hints rather than
    // from a list in this file, so a hint changed or added without a binding
    // fails here — the shape of the original bug, where three hints were
    // printed and none was bound.
    const parseHint = (hint: string): KeyboardEventInit => ({
      metaKey: hint.includes('⌘'),
      ctrlKey: hint.includes('Ctrl'),
      shiftKey: hint.includes('⇧') || hint.includes('Shift'),
      key: hint.replace(/⇧|⌘|Ctrl\+|Shift\+/g, ''),
    });

    it.each([
      ['macos' as Platform],
      ['windows' as Platform],
      ['linux' as Platform],
    ])('every shortcut it prints on %s is one that works', async (platform) => {
      const ledger = await mount(platform);
      // The label comes from the button's own text (or aria-label for the
      // icon-only refresh), NEVER from the title the hint rides — reading
      // both from one attribute would let a hint on the wrong button pass.
      const printed = Array.from(document.querySelectorAll('.tp-refresh, .tp-action')).map((b) => ({
        label: b.getAttribute('aria-label') ?? b.querySelector('.tp-act-k')?.textContent ?? '',
        hint: b.getAttribute('title')?.match(/\(([^)]+)\)$/)?.[1] ?? '',
      }));
      const hints = printed.filter((p) => p.hint);
      expect(hints.length).toBe(3); // Rescan, Settings, Quit — Open has none

      for (const { label, hint } of hints) {
        const scans = ledger.calls.scan.length;
        invoked.length = 0;
        await key(parseHint(hint));

        // Each printed hint must move the very thing its button is for.
        if (label.startsWith('Rescan')) {
          expect(ledger.calls.scan.length, `${hint} must rescan`).toBe(scans + 1);
        } else if (label.startsWith('Settings')) {
          expect(invoked, `${hint} must open Settings`).toContain('open_settings');
        } else if (label.startsWith('Quit')) {
          expect(invoked, `${hint} must quit`).toContain('quit_app');
        } else {
          throw new Error(`a hint on an unrecognised action: ${label}`);
        }
      }
    });

    // Exact modifiers only: a key the labels don't claim must stay the app's
    // (⌘R alone is a reload elsewhere) and never be hijacked into an action.
    it('ignores near-misses — wrong modifier, extra modifier, bare key', async () => {
      const ledger = await mount('macos');
      const scans = ledger.calls.scan.length;
      await key({ key: 'R', metaKey: true }); // ⌘R — no Shift, not the Rescan key
      await key({ key: 'R', shiftKey: true }); // ⇧R — no modifier at all
      await key({ key: 'R', ctrlKey: true, shiftKey: true }); // Ctrl on macOS
      await key({ key: 'R', metaKey: true, shiftKey: true, altKey: true }); // ⌥⇧⌘R
      await key({ key: ',', metaKey: true, shiftKey: true }); // ⇧⌘,
      await key({ key: ',', metaKey: true, altKey: true }); // ⌥⌘,
      await key({ key: 'q', metaKey: true, shiftKey: true }); // ⇧⌘Q
      await key({ key: 'q' }); // bare q
      // Both modifiers at once is neither platform's spelling.
      await key({ key: 'R', metaKey: true, ctrlKey: true, shiftKey: true });
      await key({ key: ',', metaKey: true, ctrlKey: true });
      await key({ key: 'q', metaKey: true, ctrlKey: true });

      expect(ledger.calls.scan.length).toBe(scans);
      expect(invoked).toEqual([]);
    });
  });
});

// The Limit cards (TOKL-19; the one-line form is reverted, ADR-0007's third
// amendment): one per live Source, collapsed to its Session + Weekly meters,
// the header a disclosure to every window. Fixtures seed the
// real provenance mix — a plan string the label derives from, a per-model
// window beside the named lanes — because an impossible fixture is how a walk
// bug hides.
describe('TrayPanel limits', () => {
  // Readings as the store would hold them, reset instants relative to the real
  // clock the panel derives against. 38% used session (62% left, ok, 3h10m),
  // 69% used weekly (31% left, low, 2d4h), and a per-model weekly at 46%.
  function claudeStored(): SourceLimits {
    const now = Math.floor(Date.now() / 1000);
    return {
      source: 'claude',
      plan: 'default_claude_max_5x',
      usageResetsAvailable: null,
      windows: [
        { windowKey: 'five_hour', windowMinutes: 300, usedPct: 38, resetsAt: now + 190 * 60, observedAt: now - 60, estimate: makeFakeEstimate() },
        { windowKey: 'seven_day', windowMinutes: 10080, usedPct: 69, resetsAt: now + (2 * 1440 + 245) * 60, observedAt: now - 60, estimate: makeFakeEstimate() },
        { windowKey: 'seven_day_fable', windowMinutes: 10080, usedPct: 46, resetsAt: now + (2 * 1440 + 245) * 60, observedAt: now - 60, estimate: makeFakeEstimate() },
      ],
    };
  }

  function codexStored(): SourceLimits {
    const now = Math.floor(Date.now() / 1000);
    return {
      source: 'codex',
      plan: 'pro',
      usageResetsAvailable: null,
      windows: [
        { windowKey: 'w300', windowMinutes: 300, usedPct: 29, resetsAt: now + 125 * 60, observedAt: now - 60, estimate: makeFakeEstimate() },
        { windowKey: 'w10080', windowMinutes: 10080, usedPct: 86, resetsAt: now + (5 * 1440 + 725) * 60, observedAt: now - 60, estimate: makeFakeEstimate() },
      ],
    };
  }

  async function mountWithLimits(limits: LimitsPort) {
    const ledger = makeFakeLedger({ summary, modelRows: sourceRows });
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => {
      root.render(
        <TrayPanel ports={{ ledger, settings: makeFakeSettings(), limits }} platform="macos" />,
      );
    });
    await settle();
    return container;
  }

  it('renders one collapsed strip per live Source: name, plan, Session + Weekly meters', async () => {
    const container = await mountWithLimits(makeFakeLimits([claudeStored(), codexStored()]));

    // Two strips — Sources with no Readings (copilot, grok, …) get none; their
    // trouble states live on the app's Limits page.
    const cardEls = Array.from(container.querySelectorAll('.tp-limcard'));
    expect(cardEls.length).toBe(2);
    expect(
      cardEls.map((c) => [
        c.querySelector('.tp-limcard-name')?.textContent,
        c.querySelector('.tp-limcard-plan')?.textContent,
        c.querySelector('img')?.getAttribute('src'),
      ]),
    ).toEqual([
      ['Claude', 'Max 5x', SOURCE_ICONS.claude],
      ['Codex', 'Pro', SOURCE_ICONS.codex],
    ]);

    // Collapsed: the two named lanes only — the per-model window waits behind
    // the disclosure. Label, toned numeral, countdown, and the fill geometry.
    const meters = (el: Element) =>
      Array.from(el.querySelectorAll('.tp-limmeter')).map((m) => [
        m.querySelector('.tp-limmeter-k')?.textContent,
        m.querySelector('.tp-limmeter-v')?.textContent,
        m.querySelector('.tp-limmeter-v')?.className,
        m.querySelector('.tp-limmeter-t')?.textContent,
      ]);
    expect(meters(cardEls[0])).toEqual([
      ['Session', '62%', 'tp-limmeter-v tp-t-ok', '· 3h 10m'],
      ['Weekly', '31%', 'tp-limmeter-v tp-t-low', '· 2d 4h'],
    ]);
    expect(meters(cardEls[1])).toEqual([
      ['Session', '71%', 'tp-limmeter-v tp-t-ok', '· 2h 5m'],
      ['Weekly', '14%', 'tp-limmeter-v tp-t-dry', '· 5d 12h'],
    ]);
    expect(container.textContent).not.toContain('Fable');

    const claudeFills = Array.from(cardEls[0].querySelectorAll('.tp-limbar .fill')).map((f) => [
      (f as HTMLElement).style.width,
      f.className,
    ]);
    expect(claudeFills).toEqual([
      ['62%', 'fill ok'],
      ['31%', 'fill low'],
    ]);
    // The tick is time: 190 of 300 minutes left ≈ 63.3% under Left framing.
    const tick = cardEls[0].querySelector('.tp-limbar .tick') as HTMLElement;
    expect(parseFloat(tick.style.left)).toBeCloseTo((190 / 300) * 100, 1);
  });

  // Which windows a card collapses to is a Settings choice (stored per Source),
  // and the default pair is only what an untouched Source falls back to.
  it('collapses to the windows Settings picked, per Source', async () => {
    const container = await mountWithLimits(
      makeFakeLimits([claudeStored(), codexStored()], {
        [PANEL_WINDOWS_KEY]: JSON.stringify({ claude: ['seven_day', 'seven_day_fable'] }),
      }),
    );
    const cardEls = Array.from(container.querySelectorAll('.tp-limcard'));
    const meters = (el: Element) =>
      Array.from(el.querySelectorAll('.tp-limmeter')).map(
        (m) => m.querySelector('.tp-limmeter-k')?.textContent,
      );

    // Claude's pick replaces the default pair outright: no Session, and the
    // per-model window promoted out from behind the disclosure.
    expect(meters(cardEls[0])).toEqual(['Weekly', 'Fable']);
    // Codex was never picked for, so it keeps the default.
    expect(meters(cardEls[1])).toEqual(['Session', 'Weekly']);
  });

  // The pick is an order as well as a set: Settings writes the running order,
  // and the meters are drawn in it rather than in the query's.
  it('draws the meters in the picked order, not the stored one', async () => {
    const container = await mountWithLimits(
      makeFakeLimits([claudeStored()], {
        [PANEL_WINDOWS_KEY]: JSON.stringify({ claude: ['seven_day_fable', 'five_hour'] }),
      }),
    );
    expect(
      Array.from(container.querySelectorAll('.tp-limmeter')).map(
        (m) => m.querySelector('.tp-limmeter-k')?.textContent,
      ),
    ).toEqual(['Fable', 'Session']);
  });

  it('shows no meters for a Source whose every window was switched off', async () => {
    const container = await mountWithLimits(
      makeFakeLimits([claudeStored()], { [PANEL_WINDOWS_KEY]: '{"claude":[]}' }),
    );
    // The card stays — its header is still the way to every window.
    expect(container.querySelectorAll('.tp-limcard').length).toBe(1);
    expect(container.querySelectorAll('.tp-limmeter').length).toBe(0);
  });

  it('expands a Source to every window on header click, and collapses back', async () => {
    const container = await mountWithLimits(makeFakeLimits([claudeStored(), codexStored()]));
    const head = container.querySelector('.tp-limcard-head') as HTMLButtonElement;

    await act(async () => head.click());
    expect(head.getAttribute('aria-expanded')).toBe('true');
    const rows = Array.from(container.querySelectorAll('.tp-limwin')).map((r) => [
      r.querySelector('.tp-limwin-k')?.textContent,
      r.querySelector('.tp-limwin-pct')?.textContent,
      r.querySelector('.tp-limwin-resets')?.textContent,
    ]);
    expect(rows).toEqual([
      ['Session', '62%', 'resets in 3h 10m'],
      ['Weekly', '31%', 'resets in 2d 4h'],
      // The per-model weekly, discovered from the key's own tail — the "·"
      // rides the sub, so the row reads "Fable · Weekly".
      ['Fable· Weekly', '54%', 'resets in 2d 4h'],
    ]);
    expect(
      container.querySelector('.tp-limwin:last-of-type .tp-limwin-k .sub')?.textContent,
    ).toBe('· Weekly');
    // Only the clicked Source expanded; Codex keeps its meters.
    const codexCard = container.querySelectorAll('.tp-limcard')[1];
    expect(codexCard.querySelectorAll('.tp-limmeter').length).toBe(2);
    expect(codexCard.querySelectorAll('.tp-limwin').length).toBe(0);

    await act(async () => head.click());
    expect(head.getAttribute('aria-expanded')).toBe('false');
    expect(container.querySelectorAll('.tp-limwin').length).toBe(0);
    expect(container.querySelectorAll('.tp-limmeter').length).toBe(4);
  });

  // The meters pick one window per lane, and a lane can hold more than one:
  // Antigravity records a Session per pool (its companion reports
  // `gemini:w300` and `3p:w300`), handed over ordered by key, so a bare
  // first-match let the alphabet decide which pool the Session meter spoke
  // for. The nearer window to its wall wins — the one a glance is for.
  it('shows the Session nearest its wall when a Source has two', async () => {
    const now = Math.floor(Date.now() / 1000);
    const twoPools: SourceLimits = {
      source: 'antigravity',
      plan: null,
      usageResetsAvailable: null,
      windows: [
        // Stored order, alphabetical by key: 3p first, with plenty left.
        { windowKey: '3p:w300', windowMinutes: 300, usedPct: 20, resetsAt: now + 90 * 60, observedAt: now - 60, estimate: makeFakeEstimate() },
        { windowKey: 'gemini:w300', windowMinutes: 300, usedPct: 90, resetsAt: now + 90 * 60, observedAt: now - 60, estimate: makeFakeEstimate() },
      ],
    };
    const container = await mountWithLimits(makeFakeLimits([twoPools]));
    expect(container.querySelector('.tp-limmeter-k')?.textContent).toBe('Gemini · Session');
    expect(container.querySelector('.tp-limmeter-v')?.textContent).toBe('10%');
  });

  // The tie the displayed figure cannot break: both pools round to 20% used,
  // so the printed numerals are identical and a pick made on them falls back
  // to stored order — the alphabet again, 3p first. Only the fraction
  // separates them, and Gemini is the one actually nearer its wall.
  it('breaks a same-numeral tie on the unrounded figure, not stored order', async () => {
    const now = Math.floor(Date.now() / 1000);
    const hairsBreadth: SourceLimits = {
      source: 'antigravity',
      plan: null,
      usageResetsAvailable: null,
      windows: [
        { windowKey: '3p:w300', windowMinutes: 300, usedPct: 20.1, resetsAt: now + 90 * 60, observedAt: now - 60, estimate: makeFakeEstimate() },
        { windowKey: 'gemini:w300', windowMinutes: 300, usedPct: 20.4, resetsAt: now + 90 * 60, observedAt: now - 60, estimate: makeFakeEstimate() },
      ],
    };
    const container = await mountWithLimits(makeFakeLimits([hairsBreadth]));
    expect(container.querySelector('.tp-limmeter-k')?.textContent).toBe('Gemini · Session');
    // Both would print 80%: the numeral cannot show why this one won.
    expect(container.querySelector('.tp-limmeter-v')?.textContent).toBe('80%');
  });

  // What the card costs, and it is deliberate (ADR-0007's third amendment): the
  // meters only speak the two named lanes, so a Source that recorded neither —
  // credits, or a raw duration — collapses to a bare name. Its figures are one
  // click away, and the one-line form that showed them without the click was
  // the thing reverted.
  it('leaves a Source with neither lane a header and no meters', async () => {
    const now = Math.floor(Date.now() / 1000);
    const creditsOnly: SourceLimits = {
      source: 'grok',
      plan: 'super',
      usageResetsAvailable: null,
      windows: [
        { windowKey: 'w43200', windowMinutes: 43200, usedPct: 41, resetsAt: now + 9 * 1440 * 60, observedAt: now - 60, estimate: makeFakeEstimate() },
      ],
    };
    const container = await mountWithLimits(makeFakeLimits([creditsOnly]));
    expect(container.querySelector('.tp-limcard-name')?.textContent).toBe('Grok');
    expect(container.querySelectorAll('.tp-limmeter').length).toBe(0);
    // The header still discloses it, so nothing is unreachable.
    await act(async () =>
      (container.querySelector('.tp-limcard-head') as HTMLButtonElement).click(),
    );
    expect(container.querySelector('.tp-limwin-k')?.textContent).toBe('Monthly credits');
    expect(container.querySelector('.tp-limwin-pct')?.textContent).toBe('59%');
  });

  it('keeps a Source with a stored failure verdict off the panel while the floor holds it', async () => {
    // Held Readings do not earn a card past a failure verdict the floor is
    // still holding: a signed-out Claude must not show yesterday's
    // percentages as a current fact. Its trouble state renders on the app's
    // Limits page, not here.
    const withFresh = await mountWithLimits(
      makeFakeLimits([claudeStored(), codexStored()], {
        [lastFailureKey('claude')]: 'signed-out',
        [lastCheckKey('claude')]: String(Date.now()),
      }),
    );
    expect(
      Array.from(withFresh.querySelectorAll('.tp-limcard-name')).map((e) => e.textContent),
    ).toEqual(['Codex']);

    // A verdict older than the floor says nothing about now — the card
    // returns, same replay rule as the page.
    document.body.replaceChildren();
    const withStale = await mountWithLimits(
      makeFakeLimits([claudeStored(), codexStored()], {
        [lastFailureKey('claude')]: 'signed-out',
        [lastCheckKey('claude')]: String(Date.now() - 24 * 3600 * 1000),
      }),
    );
    expect(
      Array.from(withStale.querySelectorAll('.tp-limcard-name')).map((e) => e.textContent),
    ).toEqual(['Claude', 'Codex']);
  });

  it('keeps the card up through a couldn\'t-check verdict — the figures are held Readings', async () => {
    // The 429 story on this surface: the vendor refused a fresh answer, the
    // page carries the error text, and the panel keeps the last-known figures
    // rather than making the Source vanish for the floor's duration.
    const container = await mountWithLimits(
      makeFakeLimits([claudeStored(), codexStored()], {
        [lastFailureKey('claude')]:
          'error:the vendor rate-limited this check (429) — try again in a minute',
        [lastCheckKey('claude')]: String(Date.now()),
      }),
    );
    expect(
      Array.from(container.querySelectorAll('.tp-limcard-name')).map((e) => e.textContent),
    ).toEqual(['Claude', 'Codex']);
  });

  it('frames the numerals by the stored Left/Used mode', async () => {
    const container = await mountWithLimits(
      makeFakeLimits([claudeStored()], { 'tl.limits.mode': 'used' }),
    );
    expect(
      Array.from(container.querySelectorAll('.tp-limmeter-v')).map((v) => v.textContent),
    ).toEqual(['38%', '69%']);
  });

  it('checks live Sources on open only behind the opt-in, at most once per floor', async () => {
    // Not opted in: opening the panel must read stored Readings only.
    const silent = makeFakeLimits([claudeStored()]);
    await mountWithLimits(silent);
    expect(silent.checked).toEqual([]);

    // Opted in: the open checks each live Source once; the Rescan press inside
    // the floor adds nothing — the floor is a promise to the vendor, and a
    // person asking is what permits a call, never what exempts it.
    const port = makeFakeLimits([claudeStored()], { [LIVE_ENABLED_KEY]: 'true' });
    const container = await mountWithLimits(port);
    expect(port.checked.length).toBeGreaterThan(0);
    expect(port.checked).toContain('claude');
    const perSource = new Map<string, number>();
    for (const s of port.checked) perSource.set(s, (perSource.get(s) ?? 0) + 1);

    await act(async () => (container.querySelector('.tp-refresh') as HTMLButtonElement).click());
    await settle();
    expect(port.checked.length).toBe([...perSource.values()].reduce((a, b) => a + b, 0));
    for (const n of perSource.values()) expect(n).toBe(1);
  });
});
