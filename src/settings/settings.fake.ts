// In-memory SettingsPort for tests: a get/set round-trip over one held Settings
// value, a call log, per-method failNext, and a canned checkUpdates.
import { DEFAULT_SETTINGS, type SettingsPort, type UpdateStatus } from './settings';
import type { Settings } from '../types';

export interface FakeSettings extends SettingsPort {
  value: Settings;
  calls: { get: number; set: Settings[]; checkUpdates: number; downloadUpdate: number; restartApp: number };
  failNext(method: 'get' | 'set' | 'checkUpdates' | 'downloadUpdate' | 'restartApp', err: unknown): void;
}

const NO_UPDATE: UpdateStatus = { state: 'not-configured', version: null };

export function makeFakeSettings(
  seed: Partial<Settings> = {},
  update: UpdateStatus = NO_UPDATE,
): FakeSettings {
  let value: Settings = { ...DEFAULT_SETTINGS, ...seed };
  const calls: FakeSettings['calls'] = { get: 0, set: [], checkUpdates: 0, downloadUpdate: 0, restartApp: 0 };
  const fails = new Map<string, unknown>();
  // download stages the checked update: same version, state 'downloaded'.
  const downloaded: UpdateStatus = { state: 'downloaded', version: update.version };

  const guard = <T>(method: string, produce: () => T): Promise<T> => {
    if (fails.has(method)) {
      const e = fails.get(method);
      fails.delete(method);
      return Promise.reject(e);
    }
    return Promise.resolve(produce());
  };

  return {
    get value() { return value; },
    calls,
    get: () => guard('get', () => { calls.get++; return { ...value }; }),
    set: (s) => guard('set', () => { calls.set.push(s); value = { ...s }; }),
    checkUpdates: () => guard('checkUpdates', () => { calls.checkUpdates++; return update; }),
    downloadUpdate: () => guard('downloadUpdate', () => { calls.downloadUpdate++; return downloaded; }),
    restartApp: () => guard('restartApp', () => { calls.restartApp++; }),
    failNext: (method, err) => fails.set(method, err),
  };
}
