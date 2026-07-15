/** @vitest-environment jsdom */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it } from 'vitest';
import { useOverview } from './useOverview';
import { systemClock } from './overviewStore';
import { makeFakeLedger } from './ledger.fake';
import type { SeriesPoint, Summary } from '../types';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

function pt(over: Partial<SeriesPoint>): SeriesPoint {
  return {
    bucket: '2026-07-16', source: 'claude', byModel: {},
    inputTokens: 10, outputTokens: 5, cacheReadTokens: 20, cacheWriteTokens: 3,
    totalTokens: 38, reasoningTokens: null, cost: 0, requests: 1, convs: 1,
    ctxMessages: null, ctxSystem: null, ctxReasoning: null, ctxToolcalls: null,
    ctxAgents: null, ctxMcp: null, ctxSkills: null, ...over,
  };
}

const summary: Summary = {
  inputTokens: 10, outputTokens: 5, cacheReadTokens: 20, cacheWriteTokens: 3,
  totalTokens: 100, requests: 2, cost: 1.5, hasUnpriced: false,
  unpricedModels: [], cacheEstimatedModels: [], cacheHitRate: 0,
};

const mountedRoots: Root[] = [];
let model: ReturnType<typeof useOverview> | null = null;

function Probe(props: Parameters<typeof useOverview>[0]) {
  model = useOverview(props);
  return null;
}

// Drive both the microtask chain (canned promises) and the delay-0 reload timer.
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
  model = null;
});

describe('useOverview', () => {
  it('loads the seeded model and keeps the heatmap stable across a range change', async () => {
    const ledger = makeFakeLedger({
      dayPoints: [
        pt({ source: 'claude', totalTokens: 100 }),
        pt({ source: 'codex', totalTokens: 200 }),
      ],
      summary,
    });

    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    await act(async () => {
      root.render(<Probe ledger={ledger} clock={systemClock} />);
    });
    await settle();

    expect(model!.loading).toBe(false);
    expect(model!.panels.heatmap.days.length).toBeGreaterThan(0);
    expect(model!.panels.context.tool.key).toBe('claude'); // first visible seeded source

    // Invariant 9: allPoints identity is stable across a range change, so the
    // heatmap memo returns the very same array.
    const daysBefore = model!.panels.heatmap.days;
    act(() => model!.setRange('week'));
    expect(Object.is(model!.panels.heatmap.days, daysBefore)).toBe(true);

    await settle();
  });
});
