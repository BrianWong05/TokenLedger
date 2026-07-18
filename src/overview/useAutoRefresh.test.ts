import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  parseRefreshSec,
  loadRefreshSec,
  saveRefreshSec,
  scheduleAutoRefresh,
  createRefreshGate,
  holdSpin,
  MIN_SPIN_MS,
  STORAGE_KEY,
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
  it('accepts any integer in [5, 86400]; everything else → 30', () => {
    // Presets still parse unchanged.
    expect(parseRefreshSec('10')).toBe(10);
    expect(parseRefreshSec('30')).toBe(30);
    expect(parseRefreshSec('60')).toBe(60);
    expect(parseRefreshSec('300')).toBe(300);
    // Arbitrary integers within bounds are accepted verbatim.
    expect(parseRefreshSec('90')).toBe(90);
    expect(parseRefreshSec('5')).toBe(5);
    expect(parseRefreshSec('86400')).toBe(86400);
    expect(parseRefreshSec('1e3')).toBe(1000); // Number('1e3') === 1000, an integer
    // Out of range, non-integer, or unparseable → 30.
    expect(parseRefreshSec('4')).toBe(30);
    expect(parseRefreshSec('0')).toBe(30);
    expect(parseRefreshSec('-30')).toBe(30);
    expect(parseRefreshSec('86401')).toBe(30);
    expect(parseRefreshSec('45.5')).toBe(30);
    expect(parseRefreshSec('nope')).toBe(30);
    expect(parseRefreshSec(null)).toBe(30);
    expect(parseRefreshSec('')).toBe(30);
  });

  it('loadRefreshSec reads storage; invalid → 30', () => {
    expect(loadRefreshSec(memoryStorage())).toBe(30);
    expect(loadRefreshSec(memoryStorage({ [STORAGE_KEY]: '60' }))).toBe(60);
    expect(loadRefreshSec(memoryStorage({ [STORAGE_KEY]: '90' }))).toBe(90);
    expect(loadRefreshSec(memoryStorage({ [STORAGE_KEY]: '4' }))).toBe(30);
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

  it('10s → ticks once per interval', () => {
    const tick = vi.fn();
    const stop = scheduleAutoRefresh(10, tick);
    vi.advanceTimersByTime(9_999);
    expect(tick).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(tick).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(10_000);
    expect(tick).toHaveBeenCalledTimes(2);
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

describe('holdSpin', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('holds at least MIN_SPIN_MS when the work finishes instantly', async () => {
    let done = false;
    const p = holdSpin(async () => 'ok').then((v) => {
      done = true;
      return v;
    });
    await vi.advanceTimersByTimeAsync(MIN_SPIN_MS - 1);
    expect(done).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(await p).toBe('ok');
    expect(done).toBe(true);
  });

  it('adds no delay when the work already ran longer than the floor', async () => {
    let done = false;
    const p = holdSpin(
      () => new Promise<void>((r) => setTimeout(r, MIN_SPIN_MS + 500)),
    ).then(() => {
      done = true;
    });
    await vi.advanceTimersByTimeAsync(MIN_SPIN_MS + 500);
    await p;
    expect(done).toBe(true);
  });

  it('holds the floor even when the work rejects', async () => {
    let settled = false;
    const p = holdSpin(async () => {
      throw new Error('boom');
    }).catch((e) => {
      settled = true;
      return e;
    });
    await vi.advanceTimersByTimeAsync(MIN_SPIN_MS - 1);
    expect(settled).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(((await p) as Error).message).toBe('boom');
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
