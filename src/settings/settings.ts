// The Settings seam: "remote but owned". Thin adapter over the Tauri IPC fns in
// src/api.ts, mirroring ledger.ts so a page (and the shell) depends on this port
// instead of @tauri-apps directly (lets tests swap in settings.fake.ts).
import { getSettings, setSettings, checkUpdates } from '../api';
import type { Settings, UpdateStatus } from '../types';

export interface SettingsPort {
  get(): Promise<Settings>;
  set(s: Settings): Promise<void>;
  checkUpdates(): Promise<UpdateStatus>;
}

export const tauriSettings: SettingsPort = {
  get: getSettings,
  set: setSettings,
  checkUpdates,
};

// The shipped defaults, matching the spec: theme System, launch-at-login and
// auto-update-check both ON, first-run disclosure not yet shown, USD (rate 1).
export const DEFAULT_SETTINGS: Settings = {
  theme: 'system',
  language: 'en',
  currency: 'USD',
  usdRate: 1,
  launchAtLogin: true,
  autoCheckUpdates: true,
  firstRunDone: false,
};
