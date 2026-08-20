import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';

export type RefreshSec = number;

export const REFRESH_PRESETS: ReadonlyArray<{ label: string; sec: RefreshSec }> = [
  { label: '10s', sec: 10 },
  { label: '30s', sec: 30 },
  { label: '60s', sec: 60 },
  { label: '5m', sec: 300 },
];

export const STORAGE_KEY = 'tokenledger.refreshSec';

// Auto-refresh off. Not a duration, so it sits outside the MIN..MAX range the
// Custom field validates against — and it stops only what this window does on
// its own: the resident capture thread still scans every few hours (ADR-0005),
// and Rescan still scans on demand.
export const REFRESH_OFF = 0;

export const MIN_REFRESH_SEC = 5;
export const MAX_REFRESH_SEC = 86_400;

export function parseRefreshSec(raw: string | null): RefreshSec {
  if (raw == null || raw === '') return 30;
  const n = Number(raw);
  if (n === REFRESH_OFF) return REFRESH_OFF;
  if (Number.isInteger(n) && n >= MIN_REFRESH_SEC && n <= MAX_REFRESH_SEC) return n;
  return 30;
}

export function loadRefreshSec(
  storage: Pick<Storage, 'getItem'> = localStorage,
): RefreshSec {
  return parseRefreshSec(storage.getItem(STORAGE_KEY));
}

export function saveRefreshSec(
  sec: RefreshSec,
  storage: Pick<Storage, 'setItem'> = localStorage,
): void {
  storage.setItem(STORAGE_KEY, String(sec));
}

export function scheduleAutoRefresh(
  sec: RefreshSec,
  tick: () => void,
  timers: {
    setInterval: typeof setInterval;
    clearInterval: typeof clearInterval;
  } = globalThis,
): () => void {
  // Off schedules nothing. Guarded here as well as at the caller because a
  // 0-second interval is not a slow tick, it is a tick every event loop turn.
  if (sec === REFRESH_OFF) return () => {};
  const id = timers.setInterval(tick, sec * 1000);
  return () => timers.clearInterval(id);
}

// Shared cross-component store (localStorage is the store) so the Settings
// control and the Overview's schedule stay in sync while the Overview stays
// mounted across tab switches.
const listeners = new Set<() => void>();
function subscribeRefreshSec(cb: () => void): () => void {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
}
export function setRefreshSec(sec: RefreshSec): void {
  saveRefreshSec(sec);
  listeners.forEach((l) => l());
}
export function useRefreshSec(): [RefreshSec, (sec: RefreshSec) => void] {
  const sec = useSyncExternalStore(subscribeRefreshSec, () => loadRefreshSec());
  return [sec, setRefreshSec];
}

// A scan can finish in ~20ms, which would flash the Rescan spinner for a
// single frame; hold the refreshing state for at least one full rotation of
// the 1s spin so the button visibly responds.
export const MIN_SPIN_MS = 1_000;

// The hold is cosmetic, so `signal` (aborted on unmount) drops it and clears
// the timer — otherwise an unmounted caller leaves a timer running for up to a
// second that wakes to set state on a tree that is gone.
export async function holdSpin<T>(work: () => Promise<T>, signal?: AbortSignal): Promise<T> {
  const t0 = Date.now();
  try {
    return await work();
  } finally {
    const remain = MIN_SPIN_MS - (Date.now() - t0);
    if (remain > 0 && !signal?.aborted) {
      await new Promise<void>((r) => {
        const id = setTimeout(r, remain);
        signal?.addEventListener('abort', () => {
          clearTimeout(id);
          r();
        }, { once: true });
      });
    }
  }
}

export function createRefreshGate(onRefresh: () => Promise<void>): {
  refresh: () => Promise<void>;
  isBusy: () => boolean;
} {
  let busy = false;
  return {
    isBusy: () => busy,
    async refresh() {
      if (busy) return;
      busy = true;
      try {
        await onRefresh();
      } finally {
        busy = false;
      }
    },
  };
}

// Auto-refresh exists to keep the visible Overview current. The resident Rust
// loop owns background capture, so scanning while this window is unfocused
// or hidden spends CPU without changing anything the person can see.
// Minimizing (macOS yellow button) may not fire window blur in the webview;
// `visibilityState` does go `hidden`, and restoring must count as a return so
// the one-shot refresh on the way back actually runs.
function windowIsActive(): boolean {
  return document.hasFocus() && document.visibilityState !== 'hidden';
}

function useWindowActive(): boolean {
  const [active, setActive] = useState(() =>
    typeof document === 'undefined' || windowIsActive(),
  );
  useEffect(() => {
    const sync = () => setActive(windowIsActive());
    window.addEventListener('focus', sync);
    window.addEventListener('blur', sync);
    document.addEventListener('visibilitychange', sync);
    return () => {
      window.removeEventListener('focus', sync);
      window.removeEventListener('blur', sync);
      document.removeEventListener('visibilitychange', sync);
    };
  }, []);
  return active;
}

export function useAutoRefresh(onRefresh: () => Promise<void>): {
  refresh: () => Promise<void>;
  refreshing: boolean;
} {
  const [refreshSec] = useRefreshSec();
  const active = useWindowActive();
  const [refreshing, setRefreshing] = useState(false);
  const onRefreshRef = useRef(onRefresh);
  onRefreshRef.current = onRefresh;
  const busyRef = useRef(false);
  // Unmounting mid-refresh aborts the in-flight spin hold: its timer goes away
  // and the tail below is skipped, so nothing wakes up to touch a torn-down tree.
  const abortRef = useRef<AbortController | null>(null);
  useEffect(() => () => abortRef.current?.abort(), []);

  const refresh = useCallback(async () => {
    if (busyRef.current) return;
    busyRef.current = true;
    setRefreshing(true);
    const ac = (abortRef.current = new AbortController());
    try {
      await holdSpin(() => onRefreshRef.current(), ac.signal);
    } finally {
      // Always release, even when aborted. The abort's job is done inside
      // holdSpin (it drops the pending timer); gating the release on it
      // stranded the gate whenever the abort came from StrictMode's
      // mount → cleanup → remount rather than a real unmount, leaving the
      // header on "Scanning…" and skipping every later tick. Setting state
      // after a real unmount is a no-op in React 18.
      busyRef.current = false;
      setRefreshing(false);
    }
  }, []);

  const previouslyActive = useRef(active);
  useEffect(() => {
    const returnedToWindow = active && !previouslyActive.current;
    previouslyActive.current = active;
    // Off means this window scans only when asked: no tick, and no scan on the
    // way back to it either. The initial load is not auto-refresh and still runs.
    if (!active || refreshSec === REFRESH_OFF) return;
    if (returnedToWindow) void refresh();
    return scheduleAutoRefresh(refreshSec, () => {
      void refresh();
    });
  }, [active, refreshSec, refresh]);

  return { refresh, refreshing };
}
