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

export function useAutoRefresh(onRefresh: () => Promise<void>): {
  refresh: () => Promise<void>;
  refreshing: boolean;
} {
  const [refreshSec] = useRefreshSec();
  const [refreshing, setRefreshing] = useState(false);
  const onRefreshRef = useRef(onRefresh);
  onRefreshRef.current = onRefresh;
  const busyRef = useRef(false);

  const refresh = useCallback(async () => {
    if (busyRef.current) return;
    busyRef.current = true;
    setRefreshing(true);
    try {
      await onRefreshRef.current();
    } finally {
      busyRef.current = false;
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    return scheduleAutoRefresh(refreshSec, () => {
      void refresh();
    });
  }, [refreshSec, refresh]);

  return { refresh, refreshing };
}
