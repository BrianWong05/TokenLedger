// The Limits seam: "remote but owned". Thin adapter over the Tauri IPC fns in
// src/api.ts, mirroring pricing.ts so the page depends on this port rather than
// @tauri-apps directly (and tests can hand it a fake). No logic here.
//
// `checkLive` is the one call that crosses ADR-0019's boundary, and it does so
// the only sanctioned way: a person pressed something, and a separate Companion
// process — never the app — presents the credential. It resolves with the
// Readings the Companion fetched and rejects with the Companion's own failure
// line, so the page can tell "not signed in" from "could not check".
import { fetchLimits, checkClaudeLimits, scan } from '../api';
import type { SourceLimits } from '../types';

// The stored preference keys this page owns. Two booleans-worth of state, so
// they live in web storage rather than growing the Settings contract: neither is
// read by Rust, the tray, or any other page.
export const LIVE_ENABLED_KEY = 'tl.limits.liveEnabled';
export const MODE_KEY = 'tl.limits.mode';

export interface LimitsPort {
  /** The current state of every Limit the Ledger holds Readings for. */
  list(): Promise<SourceLimits[]>;
  /** Run the Companion for one `live` Source. Rejects with its failure line. */
  checkLive(source: string): Promise<SourceLimits[]>;
  /** An ordinary scan — how a `logs` Source refreshes. */
  scan(): Promise<unknown>;
  /** Persisted page preferences. Swallows a storage that refuses to answer. */
  read(key: string): string | null;
  write(key: string, value: string): void;
}

export const tauriLimits: LimitsPort = {
  list: fetchLimits,
  checkLive: checkClaudeLimits,
  scan,
  read(key) {
    try {
      return localStorage.getItem(key);
    } catch {
      return null;
    }
  },
  write(key, value) {
    try {
      localStorage.setItem(key, value);
    } catch {
      /* storage disabled: the choice just does not survive the session */
    }
  },
};
