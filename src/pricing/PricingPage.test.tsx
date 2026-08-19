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
    // Overall = sum of the four (15+75+1.5+18.75)
    expect(opus.querySelector('.tl-pr-overall')!.textContent).toBe('$110.25');
  });

  it('sorts by the Overall total when its header is clicked', async () => {
    const c = await mount(<PricingPage ports={{ pricing: makeFakePricing() }} />);
    const overallHead = c.querySelector('button[aria-label="Sort by Overall"]') as HTMLButtonElement;
    await act(async () => overallHead.click());
    expect(rows(c)[0].querySelector('.tl-pr-model .name')!.textContent).toBe('claude-opus-4-8'); // priciest total
  });

  it('sorts by a rate column on header click — highest first, flipping on a second click', async () => {
    const c = await mount(<PricingPage ports={{ pricing: makeFakePricing() }} />);
    const modelName = (r: HTMLElement) => r.querySelector('.tl-pr-model .name')!.textContent;
    const inputHead = c.querySelector('button[aria-label="Sort by Input"]') as HTMLButtonElement;
    const arrow = () => inputHead.querySelector('.tl-pr-sortarrow')!.textContent;

    await act(async () => inputHead.click());
    expect(arrow()).toBe('↓'); // descending
    expect(inputHead.getAttribute('aria-label')).toBe('Input, sorted descending');
    expect(modelName(rows(c)[0])).toBe('claude-opus-4-8'); // 15/1M, priciest input

    await act(async () => inputHead.click());
    expect(arrow()).toBe('↑'); // ascending
    expect(modelName(rows(c)[0])).toBe('grok-4-fast'); // cheapest input; unpriced still last
  });

  it('pins a window-drag toolbar at the window top so dragging never selects rows', async () => {
    const c = await mount(<PricingPage ports={{ pricing: makeFakePricing() }} />);
    expect(c.querySelector('.tl-pr-toolbar')?.hasAttribute('data-tauri-drag-region')).toBe(true);
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
    // Default sort is Model A–Z (TOKL-10), so search results read alphabetically.
    expect(rows(c).map((r) => r.querySelector('.name')?.textContent)).toEqual(['gemini-3-flash', 'gemini-3-pro']);
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

  it('names who set each rate, and marks a routed one as weaker', async () => {
    const c = await mount(<PricingPage ports={{ pricing: makeFakePricing() }} />);
    const source = (name: string) =>
      Array.from(rowByModel(c, name).querySelectorAll('.tl-pr-badge')).map((b) => b.textContent);

    // A publisher's own rate names the publisher, not the catalog that carried it.
    expect(source('claude-opus-4-8')).toEqual(['Anthropic']);
    // A catalog rate still names the catalog.
    expect(source('claude-sonnet-4-8')).toEqual(['LiteLLM']);
    // A Routed Rate is qualified, so it cannot pass for a published price.
    expect(source('gemini-3-flash')).toEqual(['OpenRouter', 'Routed']);

    // The qualifier is visible in the table, not hidden behind a hover.
    const routed = rowByModel(c, 'gemini-3-flash').querySelector('.tl-pr-badge.routed')!;
    expect(routed.textContent).toBe('Routed');
    expect(routed.getAttribute('title')).toContain('publisher');
    // ...and never appears on a published rate.
    expect(rowByModel(c, 'claude-opus-4-8').querySelector('.tl-pr-badge.routed')).toBeNull();
    expect(rowByModel(c, 'claude-sonnet-4-8').querySelector('.tl-pr-badge.routed')).toBeNull();
  });

  it('shows the Override, not the routed source, when a Model is overridden', async () => {
    const c = await mount(<PricingPage ports={{ pricing: makeFakePricing() }} />);
    // hermes-4-405b has an Override over an OpenRouter catalog entry: the active
    // rate is the Override, so the row must not advertise a Routed Rate.
    const badges = Array.from(rowByModel(c, 'hermes-4-405b').querySelectorAll('.tl-pr-badge'))
      .map((b) => b.textContent);
    expect(badges).toEqual(['Override']);
  });

  it('re-reads the catalogs on demand and reloads the rates', async () => {
    const pricing = makeFakePricing();
    const c = await mount(<PricingPage ports={{ pricing }} />);
    const before = pricing.calls.list;
    const btn = c.querySelector('.tl-pr-refresh') as HTMLButtonElement;

    await act(async () => btn.click());
    await settle();
    expect(pricing.calls.refresh).toBe(1);
    expect(pricing.calls.list).toBeGreaterThan(before); // rates re-read after
    expect(btn.disabled).toBe(false); // pending state cleared
  });

  it('keeps the refresh button usable after a failed catalog read', async () => {
    const pricing = makeFakePricing();
    const c = await mount(<PricingPage ports={{ pricing }} />);
    const btn = c.querySelector('.tl-pr-refresh') as HTMLButtonElement;
    pricing.failNext('refresh', new Error('offline'));

    await act(async () => btn.click());
    await settle();
    expect(pricing.calls.refresh).toBe(0); // never reached the port body
    expect(btn.disabled).toBe(false); // and the button is not stuck pending
  });

  it('shows a pulse skeleton while the list is pending', async () => {
    const pending: PricingPort = {
      list: () => new Promise(() => {}),
      setOverride: () => Promise.resolve(),
      deleteOverride: () => Promise.resolve(),
      refresh: () => Promise.resolve(),
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
