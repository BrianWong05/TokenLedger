/** @vitest-environment jsdom */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it } from 'vitest';
import PricingPage from './PricingPage';
import { makeFakePricing } from './pricing.fake';
import { makeFakeLedger } from '../overview/ledger.fake';
import type { PricingPort } from './pricing';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const roots: Root[] = [];
afterEach(() => {
  for (const r of roots.splice(0)) act(() => r.unmount());
  document.body.replaceChildren();
});

async function settle(times = 4) {
  for (let i = 0; i < times; i++) await act(async () => { await new Promise((r) => setTimeout(r, 0)); });
}

async function mount(node: React.ReactElement) {
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  roots.push(root);
  await act(async () => root.render(node));
  await settle();
  return container;
}

function typeInto(el: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')!.set!;
  setter.call(el, value);
  el.dispatchEvent(new Event('input', { bubbles: true }));
}

const rows = (c: HTMLElement) => Array.from(c.querySelectorAll('.tl-pr-row')) as HTMLElement[];
const rowByModel = (c: HTMLElement, name: string) =>
  rows(c).find((r) => r.querySelector('.tl-pr-model .name')?.textContent === name)!;
const chip = (c: HTMLElement, label: string) =>
  Array.from(c.querySelectorAll('.tl-pr-chip')).find((b) => b.textContent?.startsWith(label)) as HTMLButtonElement;

describe('PricingPage', () => {
  it('renders every model with chip counts and per-1M formatting', async () => {
    const pricing = makeFakePricing();
    const c = await mount(<PricingPage ports={{ pricing }} />);

    expect(rows(c)).toHaveLength(12);
    expect(chip(c, 'All').textContent).toBe('All12');
    expect(chip(c, 'Unpriced').textContent).toBe('Unpriced2');
    expect(chip(c, 'Override').textContent).toBe('Override1');
    expect(chip(c, 'Cache est.').textContent).toBe('Cache est.1');

    // claude-opus-4-8 catalog input 15/1M, cache read 1.5/1M
    const opus = rowByModel(c, 'claude-opus-4-8');
    const cells = Array.from(opus.querySelectorAll('.tl-pr-rate')).map((s) => s.textContent);
    expect(cells).toEqual(['$15.00', '$75.00', '$1.50', '$18.75']);
  });

  it('renders pi Model ownership with the lowercase label and official mark', async () => {
    const pricing = makeFakePricing([{
      model: 'pi-response-model',
      tool: 'pi',
      overrideRates: null,
      catalog: null,
    }]);
    const c = await mount(<PricingPage ports={{ pricing }} />);
    const row = rowByModel(c, 'pi-response-model');
    expect(row.querySelector('.tool')?.textContent?.trim()).toBe('pi');
    expect(row.querySelector('img')?.getAttribute('src')).toMatch(/^data:image\/svg\+xml/);
  });

  it('shows ≈ on cache-estimated cache cells and — for unpriced rows', async () => {
    const c = await mount(<PricingPage ports={{ pricing: makeFakePricing() }} />);

    const est = Array.from(rowByModel(c, 'gpt-5.5-codex').querySelectorAll('.tl-pr-rate')).map((s) => s.textContent);
    expect(est[0]).toBe('$1.75');       // input
    expect(est[2]).toContain('≈');      // cache read — estimated marker
    expect(est[3]).toContain('≈');      // cache write

    const unpriced = Array.from(rowByModel(c, 'hermes-4-70b').querySelectorAll('.tl-pr-rate')).map((s) => s.textContent);
    expect(unpriced).toEqual(['—', '—', '—', '—']);
  });

  it('labels the row action per state', async () => {
    const c = await mount(<PricingPage ports={{ pricing: makeFakePricing() }} />);
    const act = (name: string) => rowByModel(c, name).querySelector('.tl-pr-act')!.textContent;
    expect(act('hermes-4-70b')).toBe('Set rate');       // unpriced
    expect(act('hermes-4-405b')).toBe('Edit override'); // override
    expect(act('claude-opus-4-8')).toBe('Override…');   // catalog
  });

  it('filters when a chip is clicked', async () => {
    const c = await mount(<PricingPage ports={{ pricing: makeFakePricing() }} />);
    await act(async () => chip(c, 'Unpriced').click());
    expect(rows(c)).toHaveLength(2);
    expect(chip(c, 'Unpriced').getAttribute('aria-pressed')).toBe('true');
  });

  it('searches by model name and by tool label', async () => {
    const c = await mount(<PricingPage ports={{ pricing: makeFakePricing() }} />);
    const input = c.querySelector('.tl-pr-search input') as HTMLInputElement;

    await act(async () => typeInto(input, 'claude'));
    expect(rows(c)).toHaveLength(3);

    await act(async () => typeInto(input, 'gemini cli'));
    expect(rows(c).map((r) => r.querySelector('.name')?.textContent)).toEqual(['gemini-3-pro', 'gemini-3-flash']);
  });

  it('lets the unpriced banner Review jump to the Unpriced filter', async () => {
    const c = await mount(<PricingPage ports={{ pricing: makeFakePricing() }} />);
    const review = c.querySelector('.tl-pr-banner-review') as HTMLButtonElement;
    expect(review).not.toBeNull();
    await act(async () => review.click());
    expect(rows(c)).toHaveLength(2);
    expect(chip(c, 'Unpriced').getAttribute('aria-pressed')).toBe('true');
  });

  it('shows a filtered-empty state that clears back', async () => {
    const c = await mount(<PricingPage ports={{ pricing: makeFakePricing() }} />);
    const input = c.querySelector('.tl-pr-search input') as HTMLInputElement;
    await act(async () => typeInto(input, 'zzz-nothing'));
    expect(rows(c)).toHaveLength(0);
    expect(c.querySelector('.tl-pr-none')).not.toBeNull();
    await act(async () => (c.querySelector('.tl-pr-none button') as HTMLButtonElement).click());
    expect(rows(c)).toHaveLength(12);
  });

  it('shows a pulse skeleton while the list is pending', async () => {
    const pending: PricingPort = {
      list: () => new Promise(() => {}),
      setOverride: () => Promise.resolve(),
      deleteOverride: () => Promise.resolve(),
      onPricesRebuilt: () => () => {},
    };
    const c = await mount(<PricingPage ports={{ pricing: pending }} />);
    expect(c.querySelector('.tl-pr-skel')).not.toBeNull();
    expect(rows(c).length).toBe(5); // skeleton rows, no data
  });

  it('shows the empty state and scans on demand', async () => {
    const pricing = makeFakePricing([]);
    const ledger = makeFakeLedger();
    const c = await mount(<PricingPage ports={{ pricing, ledger }} />);
    expect(c.querySelector('.tl-pr-empty')).not.toBeNull();
    await act(async () => (c.querySelector('.tl-pr-empty button') as HTMLButtonElement).click());
    await settle();
    expect(ledger.calls.scan.length).toBe(1);
  });

  it('re-lists on the prices-rebuilt event', async () => {
    const pricing = makeFakePricing();
    const c = await mount(<PricingPage ports={{ pricing }} />);
    expect(pricing.calls.list).toBe(1);
    await act(async () => pricing.emitPricesRebuilt());
    await settle();
    expect(pricing.calls.list).toBe(2);
    expect(c.querySelector('.tl-pr-card')).not.toBeNull();
  });
});
