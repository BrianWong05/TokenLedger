/** @vitest-environment jsdom */

// The Overview's Model-row entry point: a Model row in ModelsList opens the
// Model BREAKDOWN dialog (tokens + per-bucket cost at the active rate), and
// its "Set rate…" button hands off to the SAME OverrideEditor — the Pricing
// fix stays one step away instead of ambushing the click. Exercised through a
// tiny harness that mirrors Overview's wiring, so we don't boot the whole
// Overview to prove it.
import { act, useState } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it } from 'vitest';
import ModelsList from '../overview/ModelsList';
import ModelBreakdownModal from '../overview/ModelBreakdownModal';
import OverrideEditor from './OverrideEditor';
import { SOURCES } from '../overview/meta';
import { makeFakePricing } from './pricing.fake';
import { makeFakeSettings } from '../settings/settings.fake';
import type { ModelBar } from '../overview/data';
import type { BreakdownRow, ModelPricing } from '../types';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const roots: Root[] = [];
afterEach(() => {
  for (const r of roots.splice(0)) act(() => r.unmount());
  document.body.replaceChildren();
});

async function settle(times = 3) {
  for (let i = 0; i < times; i++) await act(async () => { await new Promise((r) => setTimeout(r, 0)); });
}

// 2M input at the fake's $15/1M ⇒ $30.00; the absent-cache case comes from the
// seed's cache-estimated gpt-5.5-codex.
const row = (key: string): BreakdownRow => ({
  key, source: 'claude', inputTokens: 2_000_000, outputTokens: 0,
  cacheReadTokens: 0, cacheWriteTokens: 0, totalTokens: 2_000_000, requests: 1,
  cost: 30, reasoningTokens: null, convs: 1, cacheEstimated: false,
  hasUnpriced: false, unattributedTokens: 0,
});

const bar = (name: string): ModelBar => ({
  name, tokens: 100, cost: 1, share: 1, segs: [], cacheEstimated: false, row: row(name),
});

describe('ModelsList model-click entry point', () => {
  it('invokes onModelClick with the raw model name and the row behind the bar', async () => {
    const clicked: [string, BreakdownRow][] = [];
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    roots.push(root);
    await act(async () => root.render(
      <ModelsList tool={SOURCES[0]} toolTokens={100} models={[bar('claude-opus-4-8')]} onModelClick={(n, r) => clicked.push([n, r])} />,
    ));
    const rowEl = container.querySelector('.tt-model') as HTMLElement;
    expect(rowEl.getAttribute('role')).toBe('button');
    await act(async () => rowEl.click());
    expect(clicked.map(([n]) => n)).toEqual(['claude-opus-4-8']);
    // The figures ride along, so the caller never looks the Model back up.
    expect(clicked[0][1].key).toBe('claude-opus-4-8');
    expect(clicked[0][1].totalTokens).toBe(2_000_000);
  });

  it('opens the breakdown dialog, and its Set rate… opens the shared OverrideEditor', async () => {
    const pricing = makeFakePricing();

    function Harness() {
      const [detail, setDetail] = useState<{ row: BreakdownRow; model: ModelPricing } | null>(null);
      const [editor, setEditor] = useState<ModelPricing | null>(null);
      // Mirrors Overview.onModelClick: the row comes from the click, only the
      // pricing is fetched.
      const open = (name: string, clicked: BreakdownRow) =>
        pricing.list().then((list) => setDetail({ row: clicked, model: list.find((x) => x.model === name)! }));
      return (
        <>
          <ModelsList tool={SOURCES[0]} toolTokens={100} models={[bar('claude-opus-4-8')]} onModelClick={open} />
          {detail && (
            <ModelBreakdownModal
              row={detail.row}
              model={detail.model}
              onSetRate={() => { setEditor(detail.model); setDetail(null); }}
              onClose={() => setDetail(null)}
            />
          )}
          {editor && <OverrideEditor model={editor} pricing={pricing} settings={makeFakeSettings()} onClose={() => setEditor(null)} />}
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

    // The click answers with the breakdown, not the pricing form.
    const dialog = container.querySelector('.tl-pr-dialog')!;
    expect(dialog).not.toBeNull();
    expect(dialog.querySelector('.name')!.textContent).toBe('claude-opus-4-8');
    expect(dialog.querySelector('input')).toBeNull();
    expect(pricing.calls.list).toBe(1); // fetched fresh on open

    // Per-bucket figures: 2M input at the seeded $15/1M rate.
    const input = dialog.querySelector('.tl-pr-bd-rows .tt-ctx-row')!;
    expect(input.querySelector('.val')!.textContent).toBe('2.00M');
    expect(input.querySelector('.rcost')!.textContent).toBe('$30.00');
    expect(input.querySelector('.rpct')!.textContent).toBe('100%');

    // Set rate… swaps to the same OverrideEditor the Pricing tab uses.
    const setRate = Array.from(dialog.querySelectorAll('button')).find((b) => b.textContent === 'Set rate…')!;
    await act(async () => setRate.click());
    await settle();
    const editor = container.querySelector('.tl-pr-dialog')!;
    expect(editor.querySelector('.name')!.textContent).toBe('claude-opus-4-8');
    expect(editor.querySelector('input')).not.toBeNull();
    expect(pricing.calls.list).toBe(1); // the handoff reuses the fetched entry
  });

  it('renders an em dash for a bucket whose rate is absent', async () => {
    const pricing = makeFakePricing();
    const model = (await pricing.list()).find((m) => m.model === 'gpt-5.5-codex')!; // cache rates null
    const cached: BreakdownRow = {
      ...row('gpt-5.5-codex'), source: 'codex',
      // Cost covers only the priced buckets (2M input at $1.75/1M), exactly as
      // the backend omits absent-rate buckets — so the decomposition reconciles.
      cacheReadTokens: 500_000, totalTokens: 2_500_000, cacheEstimated: true, cost: 3.5,
    };

    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    roots.push(root);
    await act(async () => root.render(
      <ModelBreakdownModal row={cached} model={model} onSetRate={() => {}} onClose={() => {}} />,
    ));

    const costs = Array.from(container.querySelectorAll('.tl-pr-bd-rows .rcost'), (e) => e.textContent);
    // input, output, cacheRead, cacheWrite — the unpriced cache buckets show
    // no invented $0, matching the backend Cost that omits them.
    expect(costs).toEqual(['$3.50', '$0.00', '—', '—']);
    expect(container.querySelector('.tt-tag')).not.toBeNull(); // cache est. marker
  });

  it('withholds every bucket cost when the decomposition does not sum to the row Cost', async () => {
    const pricing = makeFakePricing();
    const model = (await pricing.list()).find((m) => m.model === 'claude-opus-4-8')!;
    // 2M input would price to $30 at today's $15/1M, but the row's Cost says
    // $36 — a distinct 1h cache-write rate or a day-captured rate this dialog
    // cannot see. Figures that disagree with the total above them must not show.
    const drifted: BreakdownRow = { ...row('claude-opus-4-8'), cost: 36 };

    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    roots.push(root);
    await act(async () => root.render(
      <ModelBreakdownModal row={drifted} model={model} onSetRate={() => {}} onClose={() => {}} />,
    ));

    const costs = Array.from(container.querySelectorAll('.tl-pr-bd-rows .rcost'), (e) => e.textContent);
    expect(costs).toEqual(['—', '—', '—', '—']);
    // Tokens and shares still show — only the unfaithful $ figures withhold.
    expect(container.querySelector('.tl-pr-bd-rows .val')!.textContent).toBe('2.00M');
  });

  it('marks a mixed-provenance row Cost as Partial with the ≥ prefix', async () => {
    const pricing = makeFakePricing();
    const model = (await pricing.list()).find((m) => m.model === 'claude-opus-4-8')!;
    const mixed: BreakdownRow = { ...row('claude-opus-4-8'), hasUnpriced: true };

    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    roots.push(root);
    await act(async () => root.render(
      <ModelBreakdownModal row={mixed} model={model} onSetRate={() => {}} onClose={() => {}} />,
    ));

    expect(container.querySelector('.tt-ctx-sub')!.textContent).toContain('≥ $30.00');
  });
});
