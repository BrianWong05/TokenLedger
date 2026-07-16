// Launch-at-login enrollment. The Tauri autostart plugin (@tauri-apps/plugin-
// autostart) is not installed yet — a later wave adds it — so we reach for its
// guest API dynamically and treat its absence as a no-op. Both the Settings
// toggle and the first-run OK call this.
export async function setLaunchAtLogin(enabled: boolean): Promise<void> {
  try {
    // Non-literal specifier so tsc/vite don't try to resolve the not-yet-
    // installed plugin at build time; it resolves (or fails) at runtime.
    const spec = '@tauri-apps/plugin-autostart';
    const { enable, disable } = await import(/* @vite-ignore */ spec);
    await (enabled ? enable() : disable());
  } catch {
    // ponytail: plugin not installed yet — silently no-op until the autostart
    // wave lands. Remove this fallback once the plugin is a dependency.
  }
}
