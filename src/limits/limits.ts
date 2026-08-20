// The Limits seam: "remote but owned". Thin adapter over the Tauri IPC fns in
// src/api.ts, mirroring pricing.ts so the page depends on this port rather than
// @tauri-apps directly (and tests can hand it a fake). No logic here.
//
// `checkLive` is the one call that crosses ADR-0019's boundary, and it does so
// the only sanctioned way: a person pressed something, and a separate Companion
// process — never the app — presents the credential. Its Readings land in the
// durable series, so `list` is what renders them; it rejects with the Companion's
// own failure line, so the page can tell "not signed in" from "could not check".
import { listen } from '@tauri-apps/api/event';
import { fetchLimits, checkLiveLimits, scan } from '../api';
import type { SourceLimits } from '../types';

// The stored preference keys this page owns. Two booleans-worth of state, so
// they live in web storage rather than growing the Settings contract: neither is
// read by Rust, the tray, or any other page.
// The `.3` is a consent version, not clutter: the disclosure this boolean
// records acceptance of named only Claude Code's sign-in at `.1`, gained
// Codex's at `.2`, and now covers Antigravity's — whose Google sign-in works on
// hourly passes, so checking it means asking Google for a fresh one (ADR-0020).
// That broke the `.2` promise that a sign-in is "never refreshed", so the
// question changed and has to be asked again. The old key is deliberately
// orphaned — no migration — so nobody's earlier yes is stretched over a question
// they were never shown.
export const LIVE_ENABLED_KEY = 'tl.limits.liveEnabled.3';
export const MODE_KEY = 'tl.limits.mode';

// When a Source was last checked live, as epoch millis. Stored rather than kept
// in memory so the floor between calls survives the page being unmounted — which
// the shell does on every tab switch.
export function lastCheckKey(source: string): string {
  return `tl.limits.lastCheck.${source}`;
}

// The last classified outcome travels with the floor stamp, so a remount inside
// the floor cannot invent a different result without making another check.
export function lastFailureKey(source: string): string {
  return `tl.limits.lastFailure.${source}`;
}

export interface LimitsPort {
  /** The current state of every Limit the Ledger holds Readings for. */
  list(): Promise<SourceLimits[]>;
  /** Run the Companion for one `live` Source. Rejects with its failure line. */
  checkLive(source: string): Promise<void>;
  /** An ordinary scan — how a `logs` Source refreshes. */
  scan(): Promise<unknown>;
  /**
   * A scan somewhere else — the resident cadence, the tray's Scan now — landed
   * relevant Reading changes; the page reissues the ordinary query (spec:
   * "Evaluation timing"). Subscribe, returns unsubscribe.
   */
  onLimitsChanged(cb: () => void): () => void;
  /** Persisted page state. Swallows a storage that refuses to answer. */
  read(key: string): string | null;
  write(key: string, value: string): void;
}

export const tauriLimits: LimitsPort = {
  list: fetchLimits,
  checkLive: checkLiveLimits,
  scan,
  onLimitsChanged(cb) {
    // listen() is async; the unsubscribe resolves later, so teardown must await
    // it. Swallow a rejected setup (e.g. no Tauri runtime under test) so it
    // never surfaces as an unhandled rejection — same shape as pricing.ts.
    const un = listen('limits-changed', () => cb());
    un.catch(() => {});
    return () => {
      un.then((f) => f()).catch(() => {});
    };
  },
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
