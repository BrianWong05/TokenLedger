/** @vitest-environment jsdom */

// The pinned group head is a normal-flow row OUTSIDE the scroller (overlapping
// WebKit scroll/compositor layers flash during momentum scroll), driven by the
// scroll position. jsdom has no layout, so the relevant rectangles are mocked.
import { describe, expect, it, afterEach } from 'vitest';
import React, { createRef } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import CostBreakdownModal from './CostBreakdownModal';
import { I18nProvider } from '../lib/i18n';
import { SettingsProvider } from '../settings/SettingsContext';
import { makeFakeSettings } from '../settings/settings.fake';
import type { BreakdownRow, Summary } from '../types';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const roots: Root[] = [];
afterEach(() => {
  roots.forEach((r) => act(() => r.unmount()));
  roots.length = 0;
  document.body.innerHTML = '';
  document.documentElement.style.overflow = '';
  document.body.style.overflow = '';
});

async function render(node: React.ReactNode): Promise<HTMLElement> {
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  roots.push(root);
  await act(async () => {
    root.render(node);
    await Promise.resolve();
  });
  return container;
}

function row(overrides: Partial<BreakdownRow>): BreakdownRow {
  return {
    key: 'model',
    inputTokens: 1,
    outputTokens: 0,
    cacheReadTokens: 0,
    cacheWriteTokens: 0,
    totalTokens: 1,
    requests: 1,
    cost: 1,
    source: 'claude',
    reasoningTokens: null,
    convs: 1,
    cacheEstimated: false,
    hasUnpriced: false,
    ...overrides,
  };
}

const SUMMARY: Summary = {
  inputTokens: 1,
  outputTokens: 0,
  cacheReadTokens: 0,
  cacheWriteTokens: 0,
  totalTokens: 1,
  requests: 1,
  cost: 3,
  hasUnpriced: false,
  unpricedModels: [],
  cacheEstimatedModels: [],
  cacheHitRate: 0,
};

async function mountModal(): Promise<HTMLElement> {
  const c = await render(
    <SettingsProvider port={makeFakeSettings()}>
      <I18nProvider lang="en">
        <CostBreakdownModal
          summary={SUMMARY}
          rows={[
            row({ key: 'claude-opus', source: 'claude', cost: 2 }),
            row({ key: 'gpt-5.5', source: 'codex', cost: 1 }),
          ]}
          returnFocusRef={createRef<HTMLElement>()}
          onClose={() => {}}
        />
      </I18nProvider>
    </SettingsProvider>,
  );
  // fake layout: claude group at 0, codex group at 200; each head is 24px tall
  const scroller = c.querySelector<HTMLElement>('.tt-cost-modal-scroll')!;
  const sections = c.querySelectorAll<HTMLElement>('.tt-cost-group');
  const groupTops = [0, 200];
  const pinnedHeight = () => c.querySelector('.tt-cost-pinned-head') ? 24 : 0;
  sections.forEach((section, i) => Object.defineProperty(section, 'offsetTop', {
    get: () => groupTops[i] + pinnedHeight(),
  }));
  Object.defineProperty(scroller, 'getBoundingClientRect', {
    value: () => ({ top: pinnedHeight() }) as DOMRect,
  });
  sections.forEach((section, i) => {
    const head = section.querySelector<HTMLElement>('.tt-cost-group-head');
    if (!head) return;
    Object.defineProperty(head, 'offsetHeight', { value: 24 });
    Object.defineProperty(head, 'getBoundingClientRect', {
      value: () => ({ bottom: pinnedHeight() + groupTops[i] + 24 - scroller.scrollTop }) as DOMRect,
    });
  });
  return c;
}

function scrollTo(c: HTMLElement, top: number) {
  const scroller = c.querySelector<HTMLElement>('.tt-cost-modal-scroll')!;
  Object.defineProperty(scroller, 'scrollTop', { value: top, configurable: true });
  act(() => {
    scroller.dispatchEvent(new Event('scroll'));
  });
}

describe('CostBreakdownModal pinned group head', () => {
  it('sticks the first group from rest with only one visible source head', async () => {
    const c = await mountModal();
    expect(c.querySelector('.tt-cost-pinned-head')?.textContent ?? '').toContain('Claude Code');
    expect(Array.from(c.querySelectorAll('.tt-cost-group-head'))
      .filter((head) => head.textContent?.includes('Claude Code'))).toHaveLength(0);
  });

  it('keeps Claude Code stuck throughout the first group', async () => {
    const c = await mountModal();

    scrollTo(c, 23);
    expect(c.querySelector('.tt-cost-pinned-head')?.textContent ?? '').toContain('Claude Code');

    scrollTo(c, 150);
    expect(c.querySelector('.tt-cost-pinned-head')?.textContent ?? '').toContain('Claude Code');
  });

  it('keeps the pinned head after its normal-flow row shifts the scroller', async () => {
    const c = await mountModal();

    scrollTo(c, 24);
    expect(c.querySelector('.tt-cost-pinned-head')?.textContent ?? '').toContain('Claude Code');

    scrollTo(c, 24);
    expect(c.querySelector('.tt-cost-pinned-head')?.textContent ?? '').toContain('Claude Code');
  });

  it('pins the current group once its real head scrolls past, and swaps at boundaries', async () => {
    const c = await mountModal();

    scrollTo(c, 50); // inside the claude group
    let pinnedEl = c.querySelector('.tt-cost-pinned-head');
    expect(pinnedEl?.textContent).toContain('Claude Code');

    scrollTo(c, 250); // past the codex group's head
    pinnedEl = c.querySelector('.tt-cost-pinned-head');
    expect(pinnedEl?.textContent).toContain('Codex');

    scrollTo(c, 0); // back to rest
    expect(c.querySelector('.tt-cost-pinned-head')?.textContent ?? '').toContain('Claude Code');
  });

  it('renders pinned chrome outside the scrolling element', async () => {
    const c = await mountModal();
    scrollTo(c, 50);

    const pinnedEl = c.querySelector<HTMLElement>('.tt-cost-pinned-head')!;
    const scroller = c.querySelector<HTMLElement>('.tt-cost-modal-scroll')!;

    expect(pinnedEl.nextElementSibling).toBe(scroller);
  });
});

describe('CostBreakdownModal page scroll lock', () => {
  it('locks both page scroll roots while open and restores their previous values', async () => {
    document.documentElement.style.overflow = 'clip';
    document.body.style.overflow = 'scroll';

    await mountModal();

    expect(document.documentElement.style.overflow).toBe('hidden');
    expect(document.body.style.overflow).toBe('hidden');

    const root = roots.pop()!;
    act(() => root.unmount());

    expect(document.documentElement.style.overflow).toBe('clip');
    expect(document.body.style.overflow).toBe('scroll');
  });
});
