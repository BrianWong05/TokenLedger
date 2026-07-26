/** @vitest-environment jsdom */

// The gate must survive StrictMode's mount → cleanup → remount cycle. The
// cleanup aborts the in-flight spin hold, and the abort must not strand the
// busy gate: a stranded gate means every later tick returns early, the scan
// never runs again, and the header reads "Scanning…" forever on a process
// that is doing nothing.
import { StrictMode, createElement, useEffect } from 'react';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { useAutoRefresh, MIN_SPIN_MS } from './useAutoRefresh';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

let root: Root | null = null;
afterEach(() => {
  act(() => root?.unmount());
  root = null;
  vi.useRealTimers();
});

// Mirrors useOverview: one refresh kicked from an effect on mount.
function Probe({
  work,
  seen,
}: {
  work: () => Promise<void>;
  seen: { refreshing: boolean; refresh: () => Promise<void> };
}) {
  const { refresh, refreshing } = useAutoRefresh(work);
  seen.refreshing = refreshing;
  seen.refresh = refresh;
  useEffect(() => {
    void refresh();
  }, [refresh]);
  return null;
}

describe('useAutoRefresh under StrictMode', () => {
  it('releases the gate when the mount cycle aborts an in-flight refresh', async () => {
    vi.useFakeTimers();
    let release!: () => void;
    const pending = new Promise<void>((r) => {
      release = r;
    });
    const work = vi.fn(() => pending);
    const seen = { refreshing: false, refresh: async () => {} };

    const host = document.createElement('div');
    await act(async () => {
      root = createRoot(host);
      root.render(createElement(StrictMode, null, createElement(Probe, { work, seen })));
    });

    // The scan resolves after the cycle's cleanup has already aborted it.
    await act(async () => {
      release();
      await Promise.resolve();
      vi.advanceTimersByTime(MIN_SPIN_MS);
    });

    // The user's exact symptom: the header still says "Scanning…".
    expect(seen.refreshing).toBe(false);

    // And the consequence: the gate is stranded, so no later tick ever scans.
    // The spin hold parks on a timer, so start the refresh, advance, then await.
    const before = work.mock.calls.length;
    await act(async () => {
      const p = seen.refresh();
      await Promise.resolve();
      vi.advanceTimersByTime(MIN_SPIN_MS);
      await p;
    });
    expect(work.mock.calls.length).toBeGreaterThan(before);
  });
});
