/** @vitest-environment jsdom */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';
import TrayPanel from './TrayPanel';
import type { Platform } from '../lib/platform';
import { makeFakeLedger } from '../overview/ledger.fake';
import { makeFakeSettings } from '../settings/settings.fake';
import type { BreakdownRow, Summary } from '../types';

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

// Series helpers for the sparkline tests: a local calendar day `back` days
// ago, and one (bucket, Source) series point.
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
    // so the pace delta computes to +0.0% and stays shown. Twice over, because
    // an open paints from the Ledger and then re-reads behind its own scan.
    expect(container.querySelector('.tp-cost')?.textContent).toBe('$12.84');
    expect(container.querySelector('.tp-sub')?.textContent).toBe('3.4M tok · 1,912 req');
    expect(ledger.calls.summary.length).toBe(4);

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

    // The four actions, in 2b's order. The key hint is lifted out rather than
    // trimmed off: it is spelt per platform, and "Ctrl+Shift+R" ends in letters
    // a glyph-stripping regex would eat along with it.
    const actions = Array.from(container.querySelectorAll('.tp-action')).map((b) => {
      const label = b.cloneNode(true) as HTMLElement;
      label.querySelector('.tp-key')?.remove();
      return label.textContent?.trim();
    });
    expect(actions).toEqual(['Open TokenLedger', 'Rescan now', 'Settings…', 'Quit TokenLedger']);
  });

  // Unreadable Artifacts (ADR-0017): the panel's token figure is a floor when
  // one could hold usage in the selected window — the same ≥ the bar title and
  // the Overview carry, read from the persisted per-scan state (see api.ts), so
  // it is honest from launch rather than only after a scan has run.
  it('marks the token figure ≥ when an Unreadable Artifact could touch the window', async () => {
    const ledger = makeFakeLedger({
      summary,
      modelRows: toolRows,
      scan: {
        scannedAt: 0,
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
    expect(sub.textContent).toBe('≥ 3.4M tok · 1,912 req');
    expect(sub.getAttribute('title')).toBe('Antigravity: 100 sessions unreadable');
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

    // The extra reads: Models, Projects, the period's series, the Scan time —
    // each twice over, because an open paints from the Ledger and then
    // re-reads behind its own scan.
    expect(ledger.calls.breakdown.map((c) => c[0])).toEqual([
      'tool', 'model', 'project', 'tool', 'model', 'project',
    ]);
    expect(ledger.calls.series[0]?.[1]).toBe('hour'); // Today buckets hourly
    expect(ledger.calls.lastScan.length).toBe(2);
  });

  it('draws the sparkline for the selected period and rebuckets on a switch', async () => {
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
    const ticks = Array.from(container.querySelectorAll('.tp-spark-cap span')).map((s) => s.textContent);
    expect(ticks).toEqual([day(29).slice(5), day(15).slice(5), day(0).slice(5)]); // the axis
    expect(container.querySelector('.tp-spark-peak')?.textContent).toBe(`peak ${day(0).slice(5)} · $9.00`);
    expect(container.querySelector('.tp-spark-line')?.getAttribute('d')).toMatch(/^M3\.0 /);
    expect(container.querySelector('.tp-spark-now')).not.toBeNull(); // latest bucket marked
  });

  it('hovering the sparkline reads out that bucket; leaving restores the hint', async () => {
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

    // Idle: the reserved line hints instead of reading a bucket.
    expect(container.querySelector('.tp-spark-read')?.textContent).toBe('hover the chart');
    expect(container.querySelector('.tp-spark-hover-dot')).toBeNull();

    // jsdom boxes have no size; give the svg the viewBox's width so the
    // pointer maths has geometry to invert.
    const svg = container.querySelector('.tp-spark svg') as SVGSVGElement;
    svg.getBoundingClientRect = () =>
      ({ left: 0, top: 0, width: 260, height: 44, right: 260, bottom: 44, x: 0, y: 0 }) as DOMRect;

    // The right edge is the latest bucket (today, $9); the left edge day 29.
    await act(async () => {
      svg.dispatchEvent(new MouseEvent('mousemove', { bubbles: true, clientX: 259 }));
    });
    expect(container.querySelector('.tp-spark-read')?.textContent).toBe(
      `${day(0).slice(5)} · $9.00 · 1K tok`,
    );
    expect(container.querySelector('.tp-spark-hover-dot')).not.toBeNull();
    await act(async () => {
      svg.dispatchEvent(new MouseEvent('mousemove', { bubbles: true, clientX: 3 }));
    });
    expect(container.querySelector('.tp-spark-read')?.textContent).toBe(
      `${day(29).slice(5)} · $0.00 · 0 tok`, // an idle day reads its zero
    );

    // React synthesizes onMouseLeave from bubbling mouseout whose
    // relatedTarget lies outside the element.
    await act(async () => {
      svg.dispatchEvent(
        new MouseEvent('mouseout', { bubbles: true, relatedTarget: document.body }),
      );
    });
    expect(container.querySelector('.tp-spark-read')?.textContent).toBe('hover the chart');
    expect(container.querySelector('.tp-spark-hover-dot')).toBeNull();
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
    expect(container.querySelector('.tp-sub')?.textContent).toBe('0 tok · 0 req');
    expect(container.querySelector('.tp-delta')).toBeNull();
    expect(container.querySelector('.tp-stats')).toBeNull();
    expect(container.querySelector('.tp-models')).toBeNull();
    expect(container.querySelector('.tp-spark')).toBeNull();
  });

  // The panel is created on demand and destroyed on dismissal (ADR-0007), so
  // its mount is an open — and an open rescans, or the figures stay as old as
  // the last scan and Rescan is the only way to current ones. Holding the scan
  // pins the other half: the paint does not wait on it.
  it('rescans on open, without holding the paint behind the scan', async () => {
    const ledger = makeFakeLedger({ summary, modelRows: toolRows });
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
    const ledger = makeFakeLedger({ summary, modelRows: toolRows });
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
    const rescanBtn = Array.from(container.querySelectorAll('.tp-action')).find((b) =>
      b.textContent?.startsWith('Rescan'),
    ) as HTMLButtonElement;
    expect(rescanBtn.disabled).toBe(false);

    await act(async () => rescanBtn.click());
    await settle();
    expect(ledger.calls.scan.length).toBe(2); // the gate let a second one through
  });

  // A window the tray reshows instead of rebuilding gets no mount, so the
  // scan-on-open has to hang off panel-shown too.
  it('rescans again when the tray reshows an existing panel', async () => {
    const ledger = makeFakeLedger({ summary, modelRows: toolRows });
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
    const scansBefore = ledger.calls.scan.length; // the open scanned already
    await act(async () => rescan.click());
    await settle();

    expect(ledger.calls.scan.length).toBe(scansBefore + 1);
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

    // Hold only from here, so the open's own scan has already landed and the
    // held one is the button's.
    ledger.hold('scan');
    const scansBefore = ledger.calls.scan.length;
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
    expect(ledger.calls.scan.length).toBe(scansBefore + 1);

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

  // The panel opens on macOS and Windows (ADR-0010), and ⌘ is not a key a
  // Windows keyboard has.
  it('spells the action shortcuts the way the platform does', async () => {
    const keys = async (platform: Platform) => {
      const container = document.createElement('div');
      document.body.append(container);
      const root = createRoot(container);
      mountedRoots.push(root);
      await act(async () => {
        root.render(
          <TrayPanel
            ports={{ ledger: makeFakeLedger({ summary, modelRows: toolRows }), settings: makeFakeSettings() }}
            platform={platform}
          />,
        );
      });
      await settle();
      return Array.from(container.querySelectorAll('.tp-key')).map((e) => e.textContent);
    };

    expect(await keys('macos')).toEqual(['⇧⌘R', '⌘,', '⌘Q']);
    expect(await keys('windows')).toEqual(['Ctrl+Shift+R', 'Ctrl+,', 'Ctrl+Q']);
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
            ports={{ ledger: makeFakeLedger({ summary, modelRows: toolRows }), settings: makeFakeSettings() }}
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
});
