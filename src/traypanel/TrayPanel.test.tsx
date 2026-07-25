/** @vitest-environment jsdom */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it } from 'vitest';
import TrayPanel from './TrayPanel';
import { makeFakeLedger } from '../overview/ledger.fake';
import { makeFakeSettings } from '../settings/settings.fake';
import type { BreakdownRow, Summary } from '../types';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const summary: Summary = {
  inputTokens: 10, outputTokens: 5, cacheReadTokens: 20, cacheWriteTokens: 3,
  totalTokens: 3_400_000, requests: 1912, cost: 12.84, hasUnpriced: false,
  unattributedTokens: 0, unpricedModels: [], cacheEstimatedModels: [], cacheHitRate: 0, convs: 0,
};

const toolRows: BreakdownRow[] = [
  { key: 'claude', source: null, inputTokens: 0, outputTokens: 0, cacheReadTokens: 0,
    cacheWriteTokens: 0, totalTokens: 1_800_000, requests: 0, cost: 6.12,
    reasoningTokens: null, convs: 0, cacheEstimated: false, hasUnpriced: false,
    unattributedTokens: 0 },
  { key: 'codex', source: null, inputTokens: 0, outputTokens: 0, cacheReadTokens: 0,
    cacheWriteTokens: 0, totalTokens: 238_100, requests: 0, cost: 1.11,
    reasoningTokens: null, convs: 0, cacheEstimated: false, hasUnpriced: false,
    unattributedTokens: 0 },
];

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

describe('TrayPanel', () => {
  it('renders the 2b panel from the Ledger: header, source rows, actions', async () => {
    const ledger = makeFakeLedger({ summary, modelRows: toolRows });
    const settings = makeFakeSettings();

    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => {
      root.render(<TrayPanel ports={{ ledger, settings }} />);
    });
    await settle();

    // Header: big cost + tokens/req sub. Both summary calls hit the ledger
    // (today + yesterday-so-far); the fake serves the same canned summary,
    // so the pace delta computes to +0.0% and stays shown.
    expect(container.querySelector('.tp-cost')?.textContent).toBe('$12.84');
    expect(container.querySelector('.tp-sub')?.textContent).toBe('3.4M tok · 1,912 req');
    expect(ledger.calls.summary.length).toBe(2);

    // Source rows from breakdown('tool'), cost desc, columns split.
    const rows = Array.from(container.querySelectorAll('.tp-sources .tp-row')).map((r) => [
      r.querySelector('.tp-row-label')?.textContent,
      r.querySelector('.tp-row-tokens')?.textContent,
      r.querySelector('.tp-row-cost')?.textContent,
    ]);
    expect(rows).toEqual([
      ['Claude', '1.8M', '$6.12'],
      ['Codex', '238.1K', '$1.11'],
    ]);
    expect(ledger.calls.breakdown[0]?.[0]).toBe('tool');

    // The four actions, in 2b's order.
    const actions = Array.from(container.querySelectorAll('.tp-action')).map((b) =>
      b.textContent?.replace(/[⇧⌘,QR]+$/, '').trim(),
    );
    expect(actions).toEqual(['Open TokenLedger', 'Rescan now', 'Settings…', 'Quit TokenLedger']);
  });

  it('renders the Models section and the stats strip from the extra reads', async () => {
    const ledger = makeFakeLedger({
      summary: { ...summary, cacheHitRate: 0.9124 },
      toolRows,
      modelRows: [
        { ...toolRows[0], key: 'claude-sonnet-4-5', source: 'claude', cost: 6.12 },
        { ...toolRows[1], key: 'gpt-5-codex', source: 'codex', cost: 1.11 },
      ],
      projectRows: [{ ...toolRows[0], key: '/Users/b/Project/usage', cost: 4.4 }],
      lastScan: Math.floor(Date.now() / 1000) - 132,
    });
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => root.render(<TrayPanel ports={{ ledger, settings: makeFakeSettings() }} />));
    await settle();

    // Sources stay their own section; Models come from breakdown('model').
    expect(
      Array.from(container.querySelectorAll('.tp-sources .tp-row .tp-row-label')).map(
        (e) => e.textContent,
      ),
    ).toEqual(['Claude', 'Codex']);
    const models = Array.from(container.querySelectorAll('.tp-models .tp-row')).map((r) => [
      r.querySelector('.tp-row-label')?.textContent,
      r.querySelector('.tp-row-cost')?.textContent,
      (r.querySelector('.tp-dot') as HTMLElement | null)?.style.background,
    ]);
    expect(models).toEqual([
      ['claude-sonnet-4-5', '$6.12', 'rgb(217, 119, 87)'], // Claude's brand colour
      ['gpt-5-codex', '$1.11', 'rgb(110, 80, 242)'],
    ]);

    const stats = Array.from(container.querySelectorAll('.tp-stat')).map((s) => [
      s.querySelector('.tp-stat-k')?.textContent,
      s.querySelector('.tp-stat-v')?.textContent,
    ]);
    expect(stats).toEqual([
      ['Cache hit rate', '91.2%'],
      ['Top project', 'usage · $4.40'],
      ['Scanned', '2 min ago'],
    ]);

    // The extra reads: Models, Projects, the period's series, the Scan time.
    expect(ledger.calls.breakdown.map((c) => c[0])).toEqual(['tool', 'model', 'project']);
    expect(ledger.calls.series[0]?.[1]).toBe('hour'); // Today buckets hourly
    expect(ledger.calls.lastScan.length).toBe(1);
  });

  it('draws the sparkline for the selected period and rebuckets on a switch', async () => {
    const day = (back: number) => {
      const d = new Date();
      d.setDate(d.getDate() - back);
      const p = (n: number) => String(n).padStart(2, '0');
      return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
    };
    const point = (bucket: string, cost: number) => ({
      bucket, source: 'claude', byModel: {}, unattributedTokens: 0, hasUnpriced: false,
      inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0,
      totalTokens: 1_000, reasoningTokens: null, cost, requests: 0, convs: 0,
      ctxMessages: null, ctxSystem: null, ctxReasoning: null, ctxToolcalls: null,
      ctxAgents: null, ctxMcp: null, ctxSkills: null,
    });
    const ledger = makeFakeLedger({
      summary,
      toolRows,
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
    const cap = Array.from(container.querySelectorAll('.tp-spark-cap span')).map((s) => s.textContent);
    expect(cap).toEqual(['daily', `peak ${day(0).slice(5)} · $9.00`]);
    expect(container.querySelector('.tp-spark path')?.getAttribute('d')).toMatch(/^M0\.0 /);
  });

  it('renders lowercase pi with the official mark in the Menu Bar Extra', async () => {
    const piRow: BreakdownRow = {
      ...toolRows[0],
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

    const row = container.querySelector('.tp-row')!;
    expect(row.querySelector('.tp-row-label')?.textContent).toBe('pi');
    expect(row.querySelector('img')?.getAttribute('src')).toMatch(/^data:image\/svg\+xml/);
  });

  it('renders all-Unattributed headline and Source Cost as unavailable', async () => {
    const ledger = makeFakeLedger({
      summary: { ...summary, totalTokens: 50, requests: 1, cost: null, unattributedTokens: 50 },
      modelRows: [{ ...toolRows[0], totalTokens: 50, cost: null, unattributedTokens: 50 }],
    });
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => root.render(<TrayPanel ports={{ ledger, settings: makeFakeSettings() }} />));
    await settle();

    expect(container.querySelector('.tp-cost')?.textContent).toBe('unavailable');
    expect(container.querySelector('.tp-row-cost')?.textContent).toBe('unavailable');
  });

  it('shows the empty state alone — no sections, no fabricated 0.0% cache hit', async () => {
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

    expect(container.querySelector('.tp-empty')?.textContent).toBe('No usage yet');
    expect(container.querySelector('.tp-stats')).toBeNull();
    expect(container.querySelector('.tp-models')).toBeNull();
    expect(container.querySelector('.tp-spark')).toBeNull();
  });

  it('Rescan runs the scan through the ledger port and refetches', async () => {
    const ledger = makeFakeLedger({ summary, modelRows: toolRows });
    const settings = makeFakeSettings();
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => {
      root.render(<TrayPanel ports={{ ledger, settings }} />);
    });
    await settle();

    const rescan = Array.from(container.querySelectorAll('.tp-action')).find((b) =>
      b.textContent?.startsWith('Rescan'),
    ) as HTMLButtonElement;
    const summariesBefore = ledger.calls.summary.length;
    await act(async () => rescan.click());
    await settle();

    expect(ledger.calls.scan.length).toBe(1);
    expect(ledger.calls.summary.length).toBe(summariesBefore + 2); // refetched
  });

  it('Rescan spins while the scan runs and settles when it lands', async () => {
    const ledger = makeFakeLedger({ summary, modelRows: toolRows });
    const settings = makeFakeSettings();
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => {
      root.render(<TrayPanel ports={{ ledger, settings }} />);
    });
    await settle();

    ledger.hold('scan');
    const rescan = Array.from(container.querySelectorAll('.tp-action')).find((b) =>
      b.textContent?.startsWith('Rescan'),
    ) as HTMLButtonElement;
    await act(async () => rescan.click());

    // In flight: spinner shown, row disabled, second click coalesced, and
    // the Today figures pulse so an unchanged total still reads as a
    // refresh happening.
    expect(container.querySelector('.tp-spin')).not.toBeNull();
    expect(container.querySelector('.tp-figures.tp-pulse')).not.toBeNull();
    expect(rescan.disabled).toBe(true);
    await act(async () => rescan.click());
    expect(ledger.calls.scan.length).toBe(1);

    await act(async () => ledger.resolveHeld('scan', 0));
    await settle();
    expect(container.querySelector('.tp-spin')).toBeNull();
    expect(container.querySelector('.tp-pulse')).toBeNull();
    expect(rescan.disabled).toBe(false);
  });

  it('period segments switch the fetched window and refetch without the skeleton', async () => {
    const ledger = makeFakeLedger({ summary, modelRows: toolRows });
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
    const ledger = makeFakeLedger({ summary, modelRows: toolRows });
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
    expect(container.querySelectorAll('.tp-action').length).toBe(4);

    await act(async () => {
      ledger.resolveHeld('summary', 0);
      ledger.resolveHeld('summary', 1);
    });
    await settle();
    expect(container.querySelector('.tp-skel')).toBeNull();
    expect(container.querySelector('.tp-cost')?.textContent).toBe('$12.84');
  });
});
