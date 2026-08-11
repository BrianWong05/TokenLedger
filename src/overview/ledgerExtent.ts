// The Ledger's first record, published by the Overview for the one other
// surface that needs it: Settings, where each configured Preset is captioned
// with the window it resolves to. Resolving a Preset needs the extent — one
// whose window falls entirely before the first record is not offered by the
// picker at all — and reading it in Settings directly would mean a second full
// daily-series load for four captions. The Overview loads that series anyway
// and stays mounted across tab switches, so its answer is the fresh one.
//
// In memory rather than storage (unlike rangeMemory or customPresets): this is
// derived from the Ledger, not a preference, and a stale value read back at
// launch would caption Presets against a history that has since grown.
import { useSyncExternalStore } from 'react';

let firstIso = '';
const listeners = new Set<() => void>();

/** Empty until the Overview's first load lands. */
export function firstRecordIso(): string {
  return firstIso;
}

export function publishFirstRecord(iso: string): void {
  if (iso === firstIso) return;
  firstIso = iso;
  listeners.forEach((l) => l());
}

function subscribe(cb: () => void): () => void {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
}

export function useFirstRecord(): string {
  return useSyncExternalStore(subscribe, firstRecordIso);
}
