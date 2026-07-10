# Manual + Auto Refresh Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Refresh button and preset auto-refresh (Off · 30s · 1m · 5m, default Off, persisted) to the live Overview, where each refresh re-scans Source logs then reloads dashboard data.

**Architecture:** Pure helpers + a thin `useAutoRefresh` hook own interval state, `localStorage`, timer lifecycle, and a busy-guarded `refresh()`. `Overview8b` supplies `onRefresh` (scan → series reload) and renders the interval select + button in `tt-top-right`. Existing range `useEffect` still re-derives summary/breakdowns when `allPoints` changes.

**Tech Stack:** React 18, TypeScript, Vitest (fake timers, no new deps), existing Tauri `scan` / `fetchSeries` IPC.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-11-auto-refresh-design.md`
- Live UI only: `src/overview/Overview8b.tsx` (mounted from `main.tsx`). Do not wire the unmounted `App.tsx` / `FilterBar`.
- Refresh = **scan + reload series**, not reload-only.
- Presets only: `0 | 30 | 60 | 300`. Default **Off** (`0`).
- Persist key: `tokenledger.refreshSec` in `localStorage`.
- No overlapping refreshes (busy guard skips; no queue).
- After first paint: no full-page loading wipe on refresh; keep last good data if series fails.
- No new npm/cargo dependencies.
- Frontend tests: from repo root with `npm test`. Commit style `feat(scope):` / `test(scope):`.

## File map

| File | Role |
|---|---|
| `src/overview/useAutoRefresh.ts` | Types, presets, persist helpers, interval scheduler, busy gate, React hook |
| `src/overview/useAutoRefresh.test.ts` | Unit tests (fake timers + memory storage) |
| `src/overview/Overview8b.tsx` | `onRefresh` body, mount uses same path, top-bar controls |
| `src/overview/overview.css` | Compact select + button styles in top bar |

---

### Task 1: `useAutoRefresh` module + unit tests

**Files:**
- Create: `src/overview/useAutoRefresh.ts`
- Create: `src/overview/useAutoRefresh.test.ts`

**Interfaces:**
- Produces:
  - `export type RefreshSec = 0 | 30 | 60 | 300`
  - `export const REFRESH_PRESETS: ReadonlyArray<{ label: string; sec: RefreshSec }>`
  - `export const STORAGE_KEY = 'tokenledger.refreshSec'`
  - `export function parseRefreshSec(raw: string | null): RefreshSec`
  - `export function loadRefreshSec(storage?: Pick<Storage, 'getItem'>): RefreshSec`
  - `export function saveRefreshSec(sec: RefreshSec, storage?: Pick<Storage, 'setItem'>): void`
  - `export function scheduleAutoRefresh(sec: RefreshSec, tick: () => void, timers?: { setInterval: typeof setInterval; clearInterval: typeof clearInterval }): () => void`
  - `export function createRefreshGate(onRefresh: () => Promise<void>): { refresh: () => Promise<void>; isBusy: () => boolean }`
  - `export function useAutoRefresh(onRefresh: () => Promise<void>): { refreshSec: RefreshSec; setRefreshSec: (sec: RefreshSec) => void; refresh: () => Promise<void>; refreshing: boolean }`

- [ ] **Step 1: Write the failing tests**

Create `src/overview/useAutoRefresh.test.ts`:

```ts
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `npm test -- src/overview/useAutoRefresh.test.ts`

Expected: FAIL — module `./useAutoRefresh` not found or exports missing.

- [ ] **Step 3: Implement the module**

Create `src/overview/useAutoRefresh.ts`:

```ts
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
```

Notes:
- Pure helpers are exported for unit tests; the hook composes the same busy rules with React state for the button.
- `onRefresh` is read via ref so the timer effect does not reschedule every render when the callback identity changes.
- If `onRefresh` throws, the hook still clears `refreshing` (same as gate). Overview's `onRefresh` should catch scan/series errors internally so the promise usually resolves; do not rely on the hook to set UI errors.

- [ ] **Step 4: Run tests to verify they pass**

Run: `npm test -- src/overview/useAutoRefresh.test.ts`

Expected: all tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/overview/useAutoRefresh.ts src/overview/useAutoRefresh.test.ts
git commit -m "feat(overview): add useAutoRefresh hook with presets and tests"
```

---

### Task 2: Wire Overview8b — refresh action + top-bar UI

**Files:**
- Modify: `src/overview/Overview8b.tsx`
- Modify: `src/overview/overview.css`

**Interfaces:**
- Consumes: `useAutoRefresh`, `REFRESH_PRESETS`, `RefreshSec` from `./useAutoRefresh`; existing `scan`, `fetchSeries`, `EMPTY_FILTERS`
- Produces: top-bar interval `<select>` + Refresh `<button>`; initial load and manual/auto refresh share one `onRefresh` path

- [ ] **Step 1: Add `onRefresh` + hook; replace mount-only load**

In `Overview8b.tsx`:

1. Add imports:

```ts
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useAutoRefresh, REFRESH_PRESETS, type RefreshSec } from './useAutoRefresh';
```

(Keep existing imports; merge `useCallback` into the react import — file currently imports `useEffect, useMemo, useState` only.)

2. Replace the mount `useEffect` that scans + loads series with a shared callback and hook. **Remove** the old mount-only IIFE effect. Add:

```ts
  const onRefresh = useCallback(async () => {
    try {
      const status = await scan();
      const errs = status.sources
        .filter((s) => s.error)
        .map((s) => `${s.source}: ${s.error}`);
      setScanError(errs.length ? errs.join(' · ') : null);
    } catch (e) {
      setScanError(String(e));
      // Spec: if scan() itself throws, do not proceed to series reload.
      return;
    }
    try {
      const pts = await fetchSeries(EMPTY_FILTERS, 'day');
      setAllPoints(pts);
    } catch (e) {
      setError(String(e));
      // Keep prior data after first paint; first load still settles to [] so loading ends.
      setAllPoints((prev) => (prev === null ? [] : prev));
    }
  }, []);

  const { refreshSec, setRefreshSec, refresh, refreshing } = useAutoRefresh(onRefresh);

  // Initial scan + load once on mount (same path as manual/auto refresh).
  useEffect(() => {
    void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
```

Leave the `prices-rebuilt` listener and the per-range `useEffect` unchanged. When `setAllPoints` updates the series, the existing range effect re-fetches summary/breakdowns/ctx/hour.

- [ ] **Step 2: Add top-bar controls**

Replace the `tt-top-right` block so controls sit between the range segment and the avatar:

```tsx
          <div className="tt-top-right">
            <div className="tt-seg">
              {RANGES_8B.map((r) => (
                <button key={r.key} className={range === r.key ? 'active' : ''} onClick={() => setRange(r.key)}>
                  {r.label}
                </button>
              ))}
            </div>
            <div className="tt-refresh">
              <select
                className="tt-refresh-select"
                aria-label="Auto-refresh interval"
                value={refreshSec}
                onChange={(e) => setRefreshSec(Number(e.target.value) as RefreshSec)}
              >
                {REFRESH_PRESETS.map((p) => (
                  <option key={p.sec} value={p.sec}>
                    {p.sec === 0 ? 'Refresh off' : `Every ${p.label}`}
                  </option>
                ))}
              </select>
              <button
                type="button"
                className="tt-refresh-btn"
                onClick={() => void refresh()}
                disabled={refreshing}
                aria-busy={refreshing}
              >
                {refreshing ? 'Refreshing…' : 'Refresh'}
              </button>
            </div>
            <span className="tt-avatar">BW</span>
          </div>
```

Option labels: `Refresh off` / `Every 30s` / `Every 1m` / `Every 5m` (clear in a compact control).

- [ ] **Step 3: Add CSS**

Append to `src/overview/overview.css` (after `.tt-avatar` block is fine):

```css
.tt-refresh {
  display: inline-flex;
  align-items: center;
  gap: 8px;
}
.tt-refresh-select {
  font: inherit;
  font-size: 12px;
  font-weight: 600;
  color: #cfd6e6;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--tt-line);
  border-radius: 9px;
  padding: 6px 10px;
  outline: none;
  cursor: pointer;
}
.tt-refresh-select:focus {
  border-color: rgba(55, 201, 139, 0.45);
}
.tt-refresh-btn {
  font: inherit;
  font-size: 12px;
  font-weight: 600;
  color: #cfd6e6;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid var(--tt-line);
  border-radius: 9px;
  padding: 6px 12px;
  cursor: pointer;
}
.tt-refresh-btn:hover:not(:disabled) {
  background: rgba(255, 255, 255, 0.09);
}
.tt-refresh-btn:disabled {
  opacity: 0.55;
  cursor: default;
}
```

- [ ] **Step 4: Typecheck + unit tests**

Run:

```bash
npm test
npx tsc --noEmit
```

Expected: all tests pass; `tsc` clean.

- [ ] **Step 5: Manual smoke (optional but recommended)**

Run: `npm run tauri dev` (or `npm run dev` if frontend-only mocks apply — prefer Tauri for real `scan`).

Check:
1. Fresh profile: select shows **Refresh off**; no periodic rescan.
2. Click **Refresh** → button shows Refreshing… briefly; data updates; no blank full-page wipe if data already shown.
3. Set **Every 30s** → after ~30s another scan occurs; restart app → still **Every 30s**.
4. Spam-click Refresh while busy → no stacked concurrent scans (button disabled).

- [ ] **Step 6: Commit**

```bash
git add src/overview/Overview8b.tsx src/overview/overview.css
git commit -m "feat(overview): refresh button and auto-refresh interval controls"
```

---

## Spec coverage checklist

| Spec requirement | Task |
|---|---|
| Manual Refresh button | Task 2 |
| Auto-refresh presets Off / 30s / 1m / 5m | Task 1 + 2 |
| Default Off | Task 1 (`parseRefreshSec` / `loadRefreshSec`) |
| Persist `tokenledger.refreshSec` | Task 1 |
| Refresh = scan then series reload | Task 2 `onRefresh` |
| Range pipeline via `allPoints` dep | Task 2 (unchanged effect) |
| Busy guard / no queue | Task 1 gate + hook |
| No full-page wipe after first paint | Task 2 (no `setAllPoints(null)` on refresh) |
| Series fail keeps prior data | Task 2 `setAllPoints(prev => …)` |
| Scan throw skips series | Task 2 early `return` |
| Timer clear on interval change | Task 1 `scheduleAutoRefresh` + hook effect cleanup |
| Unit tests listed in spec | Task 1 |
| No Settings page / no free-form / no visibility pause | Out of scope — not in plan |

## Self-review notes

- No placeholders; exact code and commands.
- Types consistent: `RefreshSec = 0 | 30 | 60 | 300` everywhere.
- Hook uses the same busy semantics as `createRefreshGate` (ref + finally); pure gate is what tests assert for overlap.
- Mount and manual/auto share `onRefresh` via `refresh()` once on mount.
