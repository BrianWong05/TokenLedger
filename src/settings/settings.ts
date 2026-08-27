// The Settings seam: "remote but owned". Thin adapter over the Tauri IPC fns in
// src/api.ts, mirroring ledger.ts so a page (and the shell) depends on this port
// instead of @tauri-apps directly (lets tests swap in settings.fake.ts).
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getVersion } from '@tauri-apps/api/app';
import { getSettings, setSettings, checkUpdates } from '../api';
import type { AppliedUpdate, Settings, UpdateStatus } from '../types';

// Re-exported so the Settings pages keep importing UpdateStatus from this port;
// the union itself now lives in types.ts (the backend contract).
export type { AppliedUpdate, UpdateStatus } from '../types';

export interface SettingsPort {
  get(): Promise<Settings>;
  set(s: Settings): Promise<void>;
  checkUpdates(): Promise<UpdateStatus>;
  // The running app version, shown in Settings.
  version(): Promise<string>;
  // The climb this launch applied, for the shell's "Updated" card — handed
  // over once (Rust owns the last-run-version memory, ADR-0026); null on an
  // ordinary run, or when a hidden start's OS notification already said it.
  appliedUpdate(): Promise<AppliedUpdate | null>;
  // User-approved update actions driven by the Settings banner button.
  downloadUpdate(): Promise<UpdateStatus>;
  restartApp(): Promise<void>;
  // The Menu Bar Extra's "Settings…" item; subscribe, returns unsubscribe.
  onOpenSettings(cb: () => void): () => void;
}

export const tauriSettings: SettingsPort = {
  get: getSettings,
  set: setSettings,
  checkUpdates,
  // getVersion can fail synchronously off-runtime (jsdom), so the throw is
  // turned into a rejection here — the port owns the Tauri quirk, not its
  // callers.
  version: () => Promise.resolve().then(getVersion),
  // Inlined invoke (not routed through api.ts, which the Overview wave owns);
  // matches the check_updates command shape in src-tauri.
  appliedUpdate: () => invoke('applied_update'),
  downloadUpdate: () => invoke('download_update'),
  restartApp: () => invoke('restart_app'),
  onOpenSettings(cb) {
    // listen() is async; the unsubscribe resolves later, so teardown must
    // await it (same shape as ledger.ts onPricesRebuilt).
    const un = listen('open-settings', () => cb());
    return () => {
      un.then((f) => f());
    };
  },
};

// The shipped defaults, matching the spec: theme System, launch-at-login and
// auto-update-check both ON, first-run disclosure not yet shown, USD (rate 1),
// and a menu bar that refreshes every minute.
export const DEFAULT_SETTINGS: Settings = {
  theme: 'system',
  language: 'en',
  currency: 'USD',
  usdRate: 1,
  launchAtLogin: true,
  autoCheckUpdates: true,
  firstRunDone: false,
  menuBarRefreshSec: 60,
};

// The Menu Bar Extra's refresh cadences, in seconds. Off is the reader pacing
// the bar, never stopping the recording: the resident capture floor still scans
// every few hours (ADR-0005), so Off hands the bar back to that pace. The
// backend resolves it — this is only what the section offers.
export const MENU_BAR_REFRESH_OFF = 0;
export const MENU_BAR_REFRESH_PRESETS: ReadonlyArray<{ label: string; sec: number }> = [
  { label: '1m', sec: 60 },
  { label: '5m', sec: 300 },
  { label: '15m', sec: 900 },
];
