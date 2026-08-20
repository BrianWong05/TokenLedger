/** @vitest-environment jsdom */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MIN_SPIN_MS, useAutoRefresh } from './useAutoRefresh';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let root: Root | null = null;
afterEach(() => {
  act(() => root?.unmount());
  root = null;
  localStorage.clear();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

function Probe({ work }: { work: () => Promise<void> }) {
  useAutoRefresh(work);
  return null;
}

describe('useAutoRefresh window activity', () => {
  it('pauses scans while unfocused and refreshes once on return', async () => {
    vi.useFakeTimers();
    let focused = true;
    vi.spyOn(document, 'hasFocus').mockImplementation(() => focused);
    const work = vi.fn(async () => {});

    const host = document.createElement('div');
    await act(async () => {
      root = createRoot(host);
      root.render(<Probe work={work} />);
    });

    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000);
    });
    expect(work).toHaveBeenCalledTimes(1);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(MIN_SPIN_MS);
    });

    focused = false;
    act(() => window.dispatchEvent(new Event('blur')));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });
    expect(work).toHaveBeenCalledTimes(1);

    focused = true;
    await act(async () => {
      window.dispatchEvent(new Event('focus'));
      await Promise.resolve();
    });
    expect(work).toHaveBeenCalledTimes(2);
  });

  it('pauses while hidden (minimized) even if JS focus never dropped, and refreshes on restore', async () => {
    vi.useFakeTimers();
    vi.spyOn(document, 'hasFocus').mockReturnValue(true);
    let hidden = false;
    Object.defineProperty(document, 'visibilityState', {
      configurable: true,
      get: () => (hidden ? 'hidden' : 'visible'),
    });
    const work = vi.fn(async () => {});

    const host = document.createElement('div');
    await act(async () => {
      root = createRoot(host);
      root.render(<Probe work={work} />);
    });

    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000);
    });
    expect(work).toHaveBeenCalledTimes(1);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(MIN_SPIN_MS);
    });

    hidden = true;
    act(() => document.dispatchEvent(new Event('visibilitychange')));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });
    expect(work).toHaveBeenCalledTimes(1);

    hidden = false;
    await act(async () => {
      document.dispatchEvent(new Event('visibilitychange'));
      await Promise.resolve();
    });
    expect(work).toHaveBeenCalledTimes(2);
  });
});
