// The app's keyboard shortcuts, in one table: the chord to match and how each
// platform spells it, so what a surface prints and what fires cannot drift
// apart (CONTEXT.md, Menu Bar Extra). Both the Menu Bar Extra's panel and the
// main window read from here, and the same chord means the same action in both.
//
// Quit is the panel's alone — the main window has no quit action of its own,
// and on macOS the default application menu owns ⌘Q there anyway.
import type { Platform } from './platform';

export type HotkeyId = 'rescan' | 'settings' | 'quit';

const HOTKEYS: Record<HotkeyId, { key: string; shift?: boolean; macos: string; other: string }> = {
  rescan: { key: 'r', shift: true, macos: '⇧⌘R', other: 'Ctrl+Shift+R' },
  settings: { key: ',', macos: '⌘,', other: 'Ctrl+,' },
  quit: { key: 'q', macos: '⌘Q', other: 'Ctrl+Q' },
};

/// How this platform spells the shortcut — the string a surface displays.
export function hotkeyHint(id: HotkeyId, platform: Platform): string {
  return platform === 'macos' ? HOTKEYS[id].macos : HOTKEYS[id].other;
}

/// Whether this keydown is that shortcut, exactly: the platform's own modifier
/// (⌘ on macOS, Ctrl elsewhere) and no other, so a key the hints don't claim —
/// ⌘R bare, ⌥⇧⌘R, or one platform's spelling pressed on the other — is left
/// for the app and the OS to deal with.
export function isHotkey(e: KeyboardEvent, id: HotkeyId, platform: Platform): boolean {
  const chord = HOTKEYS[id];
  const mod = platform === 'macos' ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  return mod && !e.altKey && e.shiftKey === !!chord.shift && e.key.toLowerCase() === chord.key;
}
