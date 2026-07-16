// Launch-at-login enrollment via the Tauri autostart plugin. Both the Settings
// toggle and the first-run OK call this.
import { enable, disable } from '@tauri-apps/plugin-autostart';

export async function setLaunchAtLogin(enabled: boolean): Promise<void> {
  try {
    await (enabled ? enable() : disable());
  } catch {
    // No-op off-runtime (vitest/browser, where the plugin's IPC isn't present);
    // the SettingsPage test mocks this module, so this only guards real use.
  }
}
