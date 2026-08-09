import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';

export type RefreshSec = number;

export const REFRESH_PRESETS: ReadonlyArray<{ label: string; sec: RefreshSec }> = [
  { label: '10s', sec: 10 },
  { label: '30s', sec: 30 },
  { label: '60s', sec: 60 },
  { label: '5m', sec: 300 },
];

export const STORAGE_KEY = 'tokenledger.refreshSec';

export const MIN_REFRESH_SEC = 5;
export const MAX_REFRESH_SEC = 86_400;

export function parseRefreshSec(raw: string | null): RefreshSec {
  if (raw == null || raw === '') return 30;
  const n = Number(raw);
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
// spends CPU without changing anything the person can see.
function useWindowFocused(): boolean {
  const [focused, setFocused] = useState(() =>
    typeof document === 'undefined' || document.hasFocus(),
  );
  useEffect(() => {
    const onFocus = () => setFocused(true);
    const onBlur = () => setFocused(false);
    window.addEventListener('focus', onFocus);
    window.addEventListener('blur', onBlur);
    return () => {
      window.removeEventListener('focus', onFocus);
      window.removeEventListener('blur', onBlur);
    };
  }, []);
  return focused;
}

export function useAutoRefresh(onRefresh: () => Promise<void>): {
  refresh: () => Promise<void>;
  refreshing: boolean;
} {
  const [refreshSec] = useRefreshSec();
  const focused = useWindowFocused();
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

  const previouslyFocused = useRef(focused);
  useEffect(() => {
    const returnedToWindow = focused && !previouslyFocused.current;
    previouslyFocused.current = focused;
    if (!focused) return;
    if (returnedToWindow) void refresh();
    return scheduleAutoRefresh(refreshSec, () => {
      void refresh();
    });
  }, [focused, refreshSec, refresh]);

  return { refresh, refreshing };
}
