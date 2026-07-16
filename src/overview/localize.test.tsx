/** @vitest-environment jsdom */

// Overview localisation: the same components read Traditional Chinese when the
// I18nProvider language is zh-Hant, and route Cost through the Display Currency.
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it } from 'vitest';
import TokenBreakdown from './TokenBreakdown';
import ModelsList from './ModelsList';
import { I18nProvider } from '../lib/i18n';
import { TOOLS } from './meta';
import type { CatTotals, ModelBar } from './data';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const roots: Root[] = [];
afterEach(() => {
  for (const r of roots.splice(0)) act(() => r.unmount());
  document.body.replaceChildren();
});

function render(node: React.ReactNode): HTMLElement {
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  roots.push(root);
  act(() => root.render(node));
  return container;
}

const cats: CatTotals = { input: 10, output: 5, cacheRead: 20, cacheWrite: 3 };

describe('overview localisation', () => {
  it('renders a known heading in Traditional Chinese under zh-Hant', () => {
    const c = render(
      <I18nProvider lang="zh-Hant">
        <TokenBreakdown tool={TOOLS[0]} cats={cats} />
      </I18nProvider>,
    );
    expect(c.textContent).toContain('快取命中率'); // "Cache hit rate"
    expect(c.textContent).toContain('Token 明細'); // "… Token Breakdown"
  });

  it('renders per-Model costs in the Display Currency', () => {
    const bar: ModelBar = { name: 'claude-opus-4-8', tokens: 100, cost: 5, share: 1, segs: [], cacheEstimated: false };
    const c = render(
      <I18nProvider lang="en">
        <ModelsList tool={TOOLS[0]} toolTokens={100} models={[bar]} settings={{ currency: 'HKD', usdRate: 7.8 }} />
      </I18nProvider>,
    );
    expect(c.querySelector('.cost')!.textContent).toBe('HK$39.00'); // 5 USD × 7.8
  });
});
