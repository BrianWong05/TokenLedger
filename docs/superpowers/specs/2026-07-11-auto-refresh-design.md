# Manual refresh + auto-refresh — design

Date: 2026-07-11
Status: approved for planning

## Goal

Add a **Refresh** control and a user-settable **auto-refresh** interval to
the mounted Overview (`src/overview/Overview8b.tsx`), so the dashboard can
pick up new Source log usage without restarting the app.

## Non-goals (deferred)

- **Reload-only** (query Ledger without scanning Source logs).
- **Free-form intervals** or values outside the preset list.
- **Settings page** placement for the interval (Settings nav remains dead).
- **Pause when backgrounded** (Page Visibility / app focus).
- Touching the unmounted legacy dashboard (`src/App.tsx`, `FilterBar`)
  beyond optional later cleanup; that code already has a similar pattern
  but is not the live UI.

## Context

- Live root is `Overview8b` (see `main.tsx`). On mount it runs `scan()`
  once, then loads an unbounded daily `series` and re-fetches range-scoped
  data when `allPoints` / range / prices change. There is no manual or
  periodic refresh today.
- The unmounted `App.tsx` already implements Rescan + interval select
  (`0 | 30 | 60`) with `setInterval` calling `runScan()`. This design
  ports the *idea* into Overview8b with a cleaner boundary and a safer
  default.

## Domain notes

Terms per CONTEXT.md.

- **Refresh** (this feature) means: re-scan Source logs into the **Ledger**,
  then re-query the Ledger so the Overview shows updated Usage Records.
  It is not a pure UI redraw and not a Settings-only preference with no
  scan.

## Architecture

Two units:

### 1. `useAutoRefresh` hook

Location: `src/overview/useAutoRefresh.ts`.

Owns:

| Concern | Behavior |
|---|---|
| Interval state | `0 \| 30 \| 60 \| 300` seconds |
| Default | `0` (Off) when nothing valid is stored |
| Persistence | `localStorage` key `tokenledger.refreshSec`; write on user change; invalid/missing → `0` |
| Timer | `setInterval` when interval &gt; 0; clear/recreate on interval change or unmount |
| `refresh()` | Invokes caller-supplied async work once; busy-guard skips overlapping calls |
| Return API | `{ refreshSec, setRefreshSec, refresh, refreshing }` |

The hook does **not** call `scan` or fetch APIs itself. The host passes
`onRefresh: () => Promise<void>` so Overview keeps ownership of data state
and error fields.

### 2. Overview8b integration

- Build `onRefresh` that:
  1. Calls `scan()`; updates `scanError` (clear on clean scan; join
     per-source errors when present; set message if `scan()` throws).
  2. Re-fetches unbounded daily `fetchSeries(EMPTY_FILTERS, 'day')` and
     `setAllPoints` on success.
  3. On series failure after first paint: set `error`, **keep** previous
     `allPoints` so the dashboard does not blank. First-load empty
     fallback remains as today.
- Existing range `useEffect` (deps include `allPoints`) re-runs summary,
  breakdowns, context, and hourly series — no second orchestration path.
- Render interval select + Refresh button in `tt-top-right` (left of
  avatar). Disable or non-interact the button while `refreshing`; optional
  subtle busy affordance (spin / muted label). **No** full-page loading
  wipe after first paint.

## UI

Presets (labels → seconds):

| Label | Seconds |
|---|---|
| Off | 0 |
| 30s | 30 |
| 1m | 60 |
| 5m | 300 |

Controls sit in the existing top bar next to the range segment and avatar.
Styling should match Overview chrome (compact select + button); no new
design system.

## Behavior details

- **Manual Refresh** and **timer ticks** both call the same `refresh()`.
- While a refresh is in flight, further clicks and ticks are **no-ops**
  (not queued).
- Changing the interval clears and recreates the timer; an in-flight
  refresh is allowed to finish; the busy guard still applies.
- Manual refresh does not reset the timer phase; ticks stay on the fixed
  schedule after the last interval set.
- Custom date range selection is preserved across refresh; range effect
  re-runs with current `cf` / `ct`.
- App backgrounded: timer continues (desktop Tauri; no visibility pause).

## Error handling

| Failure | UI |
|---|---|
| `scan()` throws | `scanError` set; do not treat as success |
| Scan returns source errors | `scanError` = joined `source: error` strings (same as mount) |
| Clean scan | `scanError` cleared |
| Series fetch fails, data already shown | `error` set; keep prior `allPoints` |
| Series fetch fails, first load | existing empty-array / error path |
| Range fetches fail | unchanged stale-flag behavior |

## Testing

Unit tests for `useAutoRefresh` with fake timers:

1. Off → `onRefresh` never called by timer.
2. 30s → `onRefresh` invoked on interval.
3. Changing interval clears the previous timer (no double fire from old id).
4. While `onRefresh` is pending, a second `refresh()` is a no-op.
5. Invalid stored value → interval `0`; user change writes a valid value.

No Overview8b snapshot tests required for this feature.

## Success criteria

- User can refresh once and see new Source usage without restarting.
- User can set Off / 30s / 1m / 5m; choice survives app restart.
- Default is Off on a fresh profile.
- Overlapping refresh does not stack concurrent scans.
- After first paint, refresh does not wipe the whole UI to a blank loading
  state.
