/** @vitest-environment jsdom */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it } from 'vitest';
import OverrideEditor from './OverrideEditor';
import { makeFakePricing, seedPricing } from './pricing.fake';
import { makeFakeSettings } from '../settings/settings.fake';
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

const model = (name: string): ModelPricing => seedPricing().find((m) => m.model === name)!;

async function open(m: ModelPricing, opts: { pricing?: ReturnType<typeof makeFakePricing>; settingsCurrency?: string } = {}) {
  const pricing = opts.pricing ?? makeFakePricing();
  const settings = makeFakeSettings({ currency: opts.settingsCurrency ?? 'USD', usdRate: opts.settingsCurrency ? 7.8 : 1 });
  let closedWith: boolean | undefined;
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  roots.push(root);
  await act(async () => root.render(
    <OverrideEditor model={m} pricing={pricing} settings={settings} onClose={(changed) => { closedWith = changed; }} />,
  ));
  await settle();
  return { container, pricing, get closedWith() { return closedWith; } };
}

function typeInto(el: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')!.set!;
  setter.call(el, value);
  el.dispatchEvent(new Event('input', { bubbles: true }));
}

const field = (c: HTMLElement, key: string) => c.querySelector(`#tl-pr-f-${key}`) as HTMLInputElement;
const saveBtn = (c: HTMLElement) => c.querySelector('.tl-pr-save') as HTMLButtonElement;

describe('OverrideEditor', () => {
  it('loads an existing override as per-1M values (×1e6)', async () => {
    const { container } = await open(model('hermes-4-405b'));
    expect(field(container, 'input').value).toBe('1.2');
    expect(field(container, 'output').value).toBe('3.6');
    expect(field(container, 'cacheRead').value).toBe('0.12');
    expect(field(container, 'cacheWrite').value).toBe('1.5');
    // has an override -> Remove button + "Save override"
    expect(container.querySelector('.tl-pr-remove')).not.toBeNull();
    expect(saveBtn(container).textContent).toBe('Save override');
  });

  it('saves per-1M entry as per-token rates, empty fields as null', async () => {
    const r = await open(model('hermes-4-70b')); // unpriced
    await act(async () => typeInto(field(r.container, 'input'), '2'));
    await act(async () => typeInto(field(r.container, 'output'), '8'));
    await act(async () => saveBtn(r.container).click());
    await settle();

    expect(r.pricing.calls.setOverride).toEqual([
      ['hermes-4-70b', { input: 2 / 1e6, output: 8 / 1e6, cacheRead: null, cacheWrite: null }],
    ]);
    expect(r.closedWith).toBe(true);
  });

  it('removes an override', async () => {
    const r = await open(model('hermes-4-405b'));
    await act(async () => (r.container.querySelector('.tl-pr-remove') as HTMLButtonElement).click());
    await settle();
    expect(r.pricing.calls.deleteOverride).toEqual(['hermes-4-405b']);
    expect(r.closedWith).toBe(true);
  });

  it('disables Save until at least one field is filled', async () => {
    const { container } = await open(model('hermes-4-70b'));
    expect(saveBtn(container).disabled).toBe(true);
    expect(saveBtn(container).textContent).toBe('Save rate'); // no override
    await act(async () => typeInto(field(container, 'input'), '1.5'));
    expect(saveBtn(container).disabled).toBe(false);
  });

  it('rejects negative / non-numeric input inline and blocks Save', async () => {
    const { container } = await open(model('hermes-4-70b'));
    await act(async () => typeInto(field(container, 'input'), '-5'));
    expect(field(container, 'input').closest('.tl-pr-input')!.classList.contains('invalid')).toBe(true);
    expect(saveBtn(container).disabled).toBe(true);

    await act(async () => typeInto(field(container, 'input'), 'abc'));
    expect(saveBtn(container).disabled).toBe(true);
  });

  it('shows the USD currency note only when display currency ≠ USD', async () => {
    const usd = await open(model('hermes-4-70b'));
    expect(usd.container.querySelector('.tl-pr-currency')).toBeNull();

    const hkd = await open(model('hermes-4-70b'), { settingsCurrency: 'HKD' });
    const note = hkd.container.querySelector('.tl-pr-currency');
    expect(note).not.toBeNull();
    expect(note!.textContent).toContain('HKD');
  });

  it('captions each field with the catalog rate it would replace', async () => {
    const { container } = await open(model('hermes-4-405b')); // OpenRouter catalog present
    const caption = container.querySelector('.tl-pr-field .caption')!.textContent;
    expect(caption).toContain('Replaces OpenRouter');
    // unpriced model -> no catalog rate
    const unp = await open(model('hermes-4-70b'));
    expect(unp.container.querySelector('.tl-pr-field .caption')!.textContent).toBe('No catalog rate');
  });
});
