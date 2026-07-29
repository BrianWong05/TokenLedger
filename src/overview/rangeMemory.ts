// The last custom window, remembered across launches. The store deliberately
// starts every session on Total (a range is a question you ask, not a setting),
// but re-picking the same two dates by hand each launch is a chore — so the
// dates are kept and pre-filled, while the active range is not. Clicking Custom
// lands back where you left off; not clicking it changes nothing.
//
// Browser storage rather than the Settings file: this is per-window UI state,
// not something anyone would look for in Settings. It is also allowed to fail —
// storage can be unavailable or full, and a forgotten window is not an error
// worth surfacing.
const KEY = 'tokenledger.customRange';

export function loadCustomRange(): [string, string] | null {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? 'null');
    if (!Array.isArray(raw) || raw.length !== 2) return null;
    const [from, to] = raw;
    if (typeof from !== 'string' || typeof to !== 'string') return null;
    return from <= to ? [from, to] : [to, from];
  } catch {
    return null;
  }
}

export function saveCustomRange(from: string, to: string): void {
  try {
    localStorage.setItem(KEY, JSON.stringify([from, to]));
  } catch {
    // storage unavailable — the window simply is not remembered
  }
}
