import { useCallback, useEffect, useRef, useState } from 'react';

export type RefreshSec = 0 | 30 | 60 | 300;

export const REFRESH_PRESETS: ReadonlyArray<{ label: string; sec: RefreshSec }> = [
  { label: 'Off', sec: 0 },
  { label: '30s', sec: 30 },
  { label: '1m', sec: 60 },
  { label: '5m', sec: 300 },
];

export const STORAGE_KEY = 'tokenledger.refreshSec';

const ALLOWED = new Set<number>([0, 30, 60, 300]);

export function parseRefreshSec(raw: string | null): RefreshSec {
  if (raw == null || raw === '') return 0;
  const n = Number(raw);
  return ALLOWED.has(n) ? (n as RefreshSec) : 0;
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
  if (sec === 0) return () => {};
  const id = timers.setInterval(tick, sec * 1000);
  return () => timers.clearInterval(id);
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
  refreshSec: RefreshSec;
  setRefreshSec: (sec: RefreshSec) => void;
  refresh: () => Promise<void>;
  refreshing: boolean;
} {
  const [refreshSec, setRefreshSecState] = useState<RefreshSec>(() => loadRefreshSec());
  const [refreshing, setRefreshing] = useState(false);
  const onRefreshRef = useRef(onRefresh);
  onRefreshRef.current = onRefresh;
  const busyRef = useRef(false);

  const setRefreshSec = useCallback((sec: RefreshSec) => {
    setRefreshSecState(sec);
    saveRefreshSec(sec);
  }, []);

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

  return { refreshSec, setRefreshSec, refresh, refreshing };
}
