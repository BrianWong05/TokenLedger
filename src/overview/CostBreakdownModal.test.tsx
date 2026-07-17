/** @vitest-environment jsdom */

// The pinned group head is a fixed overlay OUTSIDE the scroller (sticky inside
// a WebKit overflow container flashes during momentum scroll), driven by the
// scroll offset. jsdom has no layout, so section offsetTop is mocked.
import { describe, expect, it, afterEach } from 'vitest';
import React, { createRef } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import CostBreakdownModal from './CostBreakdownModal';
import { I18nProvider } from '../lib/i18n';
import { SettingsProvider } from '../settings/SettingsContext';
import { makeFakeSettings } from '../settings/settings.fake';
import type { BreakdownRow, Summary } from '../types';

const roots: Root[] = [];
afterEach(() => {
  roots.forEach((r) => act(() => r.unmount()));
  roots.length = 0;
  document.body.innerHTML = '';
});

function render(node: React.ReactNode): HTMLElement {
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  roots.push(root);
  act(() => root.render(node));
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

function mountModal(): HTMLElement {
  const c = render(
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
  // fake layout: claude group at 0, codex group at 200
  const sections = c.querySelectorAll<HTMLElement>('.tt-cost-group');
  Object.defineProperty(sections[0], 'offsetTop', { value: 0 });
  Object.defineProperty(sections[1], 'offsetTop', { value: 200 });
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
  it('is hidden at rest', () => {
    const c = mountModal();
    expect(c.querySelector('.tt-cost-pinned-head')).toBeNull();
  });

  it('pins the current group once its real head scrolls past, and swaps at boundaries', () => {
    const c = mountModal();

    scrollTo(c, 50); // inside the claude group
    let pinnedEl = c.querySelector('.tt-cost-pinned-head');
    expect(pinnedEl?.textContent).toContain('Claude Code');

    scrollTo(c, 250); // past the codex group's head
    pinnedEl = c.querySelector('.tt-cost-pinned-head');
    expect(pinnedEl?.textContent).toContain('Codex');

    scrollTo(c, 0); // back to rest
    expect(c.querySelector('.tt-cost-pinned-head')).toBeNull();
  });
});
