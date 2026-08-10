// The reader's own Custom-range shortcuts: configured in Settings, read by the
// picker. Browser storage rather than the Settings row for the reason the
// refresh interval uses it (useAutoRefresh) — it is per-window UI state, the
// picker only exists in the main window, and the settings table is a
// fixed-column single row that a list would cost a migration and a binding
// regeneration. Failure is silent, like rangeMemory: storage can be unavailable
// or full, and a forgotten shortcut is not an error worth surfacing.
import { useSyncExternalStore } from 'react';
import { CALENDAR_PRESETS } from './data';
import type { CalendarPresetKey, PresetSlot, PresetSlots } from './data';

export const CUSTOM_PRESETS_KEY = 'tokenledger.customPresets';

// The picker's shortcut column is a plain flex column beside a two-month
// calendar — four shipped plus four of these is the height it can hold without
// becoming something to scroll.
export const MAX_CUSTOM_PRESETS = 4;

// 1 day would be today alone, sitting directly under a Yesterday button that
// means something else. The ceiling is only a sanity guard: anything past the
// first record clamps to it anyway.
const MIN_PRESET_DAYS = 2;
const MAX_PRESET_DAYS = 1825;

// Day counts the shipped shortcuts already own, so a configured one cannot
// duplicate them. 7 and 30 are absent on purpose: they shadow the Week and
// Month segments, but that no-repeat rule governs the set we ship, not the set
// a reader builds for themselves.
export const SHIPPED_DAYS: readonly number[] = [90];

export function validDays(n: number): boolean {
  return Number.isInteger(n) && n >= MIN_PRESET_DAYS && n <= MAX_PRESET_DAYS;
}

// Storage is hand-editable and survives across versions, so anything that is
// not a usable shortcut becomes an empty slot rather than a render-time crash.
function slotOf(v: unknown): PresetSlot | null {
  if (!v || typeof v !== 'object') return null;
  const { key, days } = v as { key?: unknown; days?: unknown };
  if (key === 'rolling') {
    return typeof days === 'number' && validDays(days) ? { key, days } : null;
  }
  return CALENDAR_PRESETS.includes(key as CalendarPresetKey)
    ? { key: key as CalendarPresetKey }
    : null;
}

function parseSlots(raw: string | null): PresetSlots {
  const empty: PresetSlots = Array(MAX_CUSTOM_PRESETS).fill(null);
  if (!raw) return empty;
  try {
    const v: unknown = JSON.parse(raw);
    // Fixed length either way, so slot positions and holes survive a round trip.
    return Array.isArray(v) ? empty.map((_, i) => slotOf(v[i])) : empty;
  } catch {
    return empty;
  }
}

function read(): string | null {
  try {
    return localStorage.getItem(CUSTOM_PRESETS_KEY);
  } catch {
    return null;
  }
}

// useSyncExternalStore compares snapshots by identity, so parsing afresh on
// every read would hand it a new array each time and spin forever. Re-parse
// only when the stored text actually changed.
let cached: { raw: string | null; slots: PresetSlots } | null = null;
function snapshot(): PresetSlots {
  const raw = read();
  if (!cached || cached.raw !== raw) cached = { raw, slots: parseSlots(raw) };
  return cached.slots;
}

// Shared cross-component store (localStorage is the store) so a slot edited on
// the Settings tab reaches a picker inside an Overview that stayed mounted
// across the tab switch.
const listeners = new Set<() => void>();
function subscribe(cb: () => void): () => void {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
}

export function setCustomPresets(slots: PresetSlots): void {
  try {
    localStorage.setItem(CUSTOM_PRESETS_KEY, JSON.stringify(slots));
  } catch {
    // storage unavailable — the shortcuts simply are not remembered
  }
  listeners.forEach((l) => l());
}

export function useCustomPresets(): [PresetSlots, (slots: PresetSlots) => void] {
  return [useSyncExternalStore(subscribe, snapshot), setCustomPresets];
}
