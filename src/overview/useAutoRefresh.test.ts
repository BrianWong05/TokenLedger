import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  parseRefreshSec,
  loadRefreshSec,
  saveRefreshSec,
  scheduleAutoRefresh,
  createRefreshGate,
  STORAGE_KEY,
  type RefreshSec,
} from './useAutoRefresh';

function memoryStorage(initial: Record<string, string> = {}) {
  const map = new Map(Object.entries(initial));
  return {
    getItem: (k: string) => (map.has(k) ? map.get(k)! : null),
    setItem: (k: string, v: string) => {
      map.set(k, v);
    },
  };
}

describe('parseRefreshSec / load / save', () => {
  it('accepts only 0, 30, 60, 300', () => {
    expect(parseRefreshSec('0')).toBe(0);
    expect(parseRefreshSec('30')).toBe(30);
    expect(parseRefreshSec('60')).toBe(60);
    expect(parseRefreshSec('300')).toBe(300);
    expect(parseRefreshSec(null)).toBe(0);
    expect(parseRefreshSec('')).toBe(0);
    expect(parseRefreshSec('15')).toBe(0);
    expect(parseRefreshSec('nope')).toBe(0);
  });

  it('loadRefreshSec reads storage; invalid → 0', () => {
    expect(loadRefreshSec(memoryStorage())).toBe(0);
    expect(loadRefreshSec(memoryStorage({ [STORAGE_KEY]: '60' }))).toBe(60);
    expect(loadRefreshSec(memoryStorage({ [STORAGE_KEY]: '99' }))).toBe(0);
  });

  it('saveRefreshSec writes the string value', () => {
    const s = memoryStorage();
    saveRefreshSec(300, s);
    expect(s.getItem(STORAGE_KEY)).toBe('300');
  });
});

describe('scheduleAutoRefresh', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('Off → never ticks', () => {
    const tick = vi.fn();
    const stop = scheduleAutoRefresh(0, tick);
    vi.advanceTimersByTime(60_000);
    expect(tick).not.toHaveBeenCalled();
    stop();
  });

  it('30s → ticks once per interval', () => {
    const tick = vi.fn();
    const stop = scheduleAutoRefresh(30, tick);
    vi.advanceTimersByTime(29_999);
    expect(tick).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(tick).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(30_000);
    expect(tick).toHaveBeenCalledTimes(2);
    stop();
  });

  it('stop clears the timer; changing interval is stop + new schedule', () => {
    const tick = vi.fn();
    const stop30 = scheduleAutoRefresh(30, tick);
    stop30();
    const stop60 = scheduleAutoRefresh(60, tick);
    vi.advanceTimersByTime(30_000);
    expect(tick).not.toHaveBeenCalled();
    vi.advanceTimersByTime(30_000);
    expect(tick).toHaveBeenCalledTimes(1);
    stop60();
    vi.advanceTimersByTime(60_000);
    expect(tick).toHaveBeenCalledTimes(1);
  });
});

describe('createRefreshGate', () => {
  it('skips overlapping refresh while the first is in flight', async () => {
    let release!: () => void;
    const pending = new Promise<void>((r) => {
      release = r;
    });
    const onRefresh = vi.fn(() => pending);
    const gate = createRefreshGate(onRefresh);

    const first = gate.refresh();
    const second = gate.refresh();
    expect(gate.isBusy()).toBe(true);
    expect(onRefresh).toHaveBeenCalledTimes(1);

    release();
    await first;
    await second;
    expect(gate.isBusy()).toBe(false);
    expect(onRefresh).toHaveBeenCalledTimes(1);

    await gate.refresh();
    expect(onRefresh).toHaveBeenCalledTimes(2);
  });

  it('clears busy even if onRefresh rejects', async () => {
    const gate = createRefreshGate(async () => {
      throw new Error('boom');
    });
    await expect(gate.refresh()).rejects.toThrow('boom');
    expect(gate.isBusy()).toBe(false);
    await expect(gate.refresh()).rejects.toThrow('boom');
  });
});
