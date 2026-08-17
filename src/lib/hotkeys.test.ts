/** @vitest-environment jsdom */

import { describe, expect, it } from 'vitest';
import { hotkeyHint, isHotkey } from './hotkeys';
import type { Platform } from './platform';

function key(init: KeyboardEventInit): KeyboardEvent {
  return new KeyboardEvent('keydown', init);
}

describe('hotkeys', () => {
  it('spells ⌘ on macOS and Ctrl on Windows and Linux', () => {
    expect(hotkeyHint('rescan', 'macos')).toBe('⇧⌘R');
    expect(hotkeyHint('settings', 'macos')).toBe('⌘,');
    expect(hotkeyHint('quit', 'macos')).toBe('⌘Q');
    for (const platform of ['windows', 'linux'] as Platform[]) {
      expect(hotkeyHint('rescan', platform)).toBe('Ctrl+Shift+R');
      expect(hotkeyHint('settings', platform)).toBe('Ctrl+,');
      expect(hotkeyHint('quit', platform)).toBe('Ctrl+Q');
    }
  });

  it('fires Ctrl on Windows and Linux, and ignores ⌘ there', () => {
    expect(isHotkey(key({ key: 'r', metaKey: true, shiftKey: true }), 'rescan', 'macos')).toBe(true);
    expect(isHotkey(key({ key: 'r', ctrlKey: true, shiftKey: true }), 'rescan', 'macos')).toBe(false);
    expect(isHotkey(key({ key: ',', metaKey: true }), 'settings', 'macos')).toBe(true);
    for (const platform of ['windows', 'linux'] as Platform[]) {
      expect(isHotkey(key({ key: 'r', ctrlKey: true, shiftKey: true }), 'rescan', platform)).toBe(true);
      expect(isHotkey(key({ key: 'r', metaKey: true, shiftKey: true }), 'rescan', platform)).toBe(false);
      expect(isHotkey(key({ key: ',', ctrlKey: true }), 'settings', platform)).toBe(true);
      expect(isHotkey(key({ key: ',', metaKey: true }), 'settings', platform)).toBe(false);
      expect(isHotkey(key({ key: 'q', ctrlKey: true }), 'quit', platform)).toBe(true);
      expect(isHotkey(key({ key: 'q', metaKey: true }), 'quit', platform)).toBe(false);
    }
  });
});
