/** @vitest-environment jsdom */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import TokenTotalHeadline from './TokenTotalHeadline';
import './overview.css';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const mountedRoots: Root[] = [];

function renderHeadline(total: number): HTMLButtonElement {
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  mountedRoots.push(root);
  act(() => root.render(<TokenTotalHeadline total={total} />));
  return container.querySelector('button')!;
}

describe('TokenTotalHeadline', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  afterEach(() => {
    for (const root of mountedRoots.splice(0)) {
      act(() => root.unmount());
    }
    document.body.replaceChildren();
  });

  it('lets the user switch between compact and exact totals and remembers the choice', () => {
    const button = renderHeadline(4_500_000_000);

    expect(button.textContent).toBe('4.5B');
    expect(button.type).toBe('button');
    expect(button.title).toBe('Show exact token count');
    expect(button.getAttribute('aria-label')).toBe(
      '4,500,000,000 total tokens. Show exact token count',
    );

    act(() => button.click());

    expect(button.textContent).toBe('4,500,000,000');
    expect(button.title).toBe('Show compact token count');
    expect(button.getAttribute('aria-label')).toBe(
      '4,500,000,000 total tokens. Show compact token count',
    );

    const restored = renderHeadline(4_500_000_000);
    expect(restored.textContent).toBe('4,500,000,000');
  });

  it.each([
    [950, '950'],
    [1_500, '1.5K'],
    [1_140_000, '1.14M'],
    [114_000_000, '114M'],
    [114_200_000, '114.2M'],
    [4_560_000_000, '4.56B'],
    [999_995_000, '1B'],
  ])('renders %i tokens compactly as %s', (total, expected) => {
    expect(renderHeadline(total).textContent).toBe(expected);
  });

  it('keeps a long exact total on one line within the available width', () => {
    const button = renderHeadline(4_500_123_456);
    act(() => button.click());

    expect(button.textContent).toBe('4,500,123,456');
    const style = getComputedStyle(button);
    expect(style.whiteSpace).toBe('nowrap');
    expect(style.maxWidth).toBe('100%');
  });

  it('remembers a mode change when compact and exact text are identical', () => {
    const button = renderHeadline(950);
    act(() => button.click());

    expect(button.textContent).toBe('950');
    expect(button.title).toBe('Show compact token count');
    expect(renderHeadline(1_500).textContent).toBe('1,500');
  });

  it('occupies its own row so estimated Cost renders underneath', () => {
    const button = renderHeadline(4_500_000_000);

    expect(getComputedStyle(button).display).toBe('block');
  });
});
