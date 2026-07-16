/** @vitest-environment jsdom */

// The Overview -> Pricing entry point: a Model row in ModelsList opens the SAME
// OverrideEditor, after fetching a fresh ModelPricing list on open. Exercised
// through a tiny harness that mirrors Overview.openPricing, so we don't boot the
// whole Overview to prove the wiring.
import { act, useState } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it } from 'vitest';
import ModelsList from '../overview/ModelsList';
import OverrideEditor from './OverrideEditor';
import { TOOLS } from '../overview/meta';
import { makeFakePricing } from './pricing.fake';
import { makeFakeSettings } from '../settings/settings.fake';
import type { ModelBar } from '../overview/data';
import type { ModelPricing } from '../types';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const roots: Root[] = [];
afterEach(() => {
  for (const r of roots.splice(0)) act(() => r.unmount());
  document.body.replaceChildren();
});

async function settle(times = 3) {
  for (let i = 0; i < times; i++) await act(async () => { await new Promise((r) => setTimeout(r, 0)); });
}

const bar = (name: string): ModelBar => ({ name, tokens: 100, cost: 1, share: 1, segs: [], cacheEstimated: false });

describe('ModelsList model-click entry point', () => {
  it('invokes onModelClick with the raw model name', async () => {
    const clicked: string[] = [];
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    roots.push(root);
    await act(async () => root.render(
      <ModelsList tool={TOOLS[0]} toolTokens={100} models={[bar('claude-opus-4-8')]} onModelClick={(n) => clicked.push(n)} />,
    ));
    const row = container.querySelector('.tt-model') as HTMLElement;
    expect(row.getAttribute('role')).toBe('button');
    await act(async () => row.click());
    expect(clicked).toEqual(['claude-opus-4-8']);
  });

  it('opens the shared OverrideEditor for the clicked model', async () => {
    const pricing = makeFakePricing();

    function Harness() {
      const [m, setM] = useState<ModelPricing | null>(null);
      const open = (name: string) =>
        pricing.list().then((list) => setM(list.find((x) => x.model === name) ?? null));
      return (
        <>
          <ModelsList tool={TOOLS[0]} toolTokens={100} models={[bar('claude-opus-4-8')]} onModelClick={open} />
          {m && <OverrideEditor model={m} pricing={pricing} settings={makeFakeSettings()} onClose={() => setM(null)} />}
        </>
      );
    }

    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    roots.push(root);
    await act(async () => root.render(<Harness />));

    expect(container.querySelector('.tl-pr-dialog')).toBeNull();
    await act(async () => (container.querySelector('.tt-model') as HTMLElement).click());
    await settle();

    const dialog = container.querySelector('.tl-pr-dialog');
    expect(dialog).not.toBeNull();
    expect(dialog!.querySelector('.name')!.textContent).toBe('claude-opus-4-8');
    expect(pricing.calls.list).toBe(1); // fetched fresh on open
  });
});
