/** @vitest-environment jsdom */

import { afterEach, describe, expect, it } from 'vitest';
import { applyTheme } from './theme';

// Controllable matchMedia mock for '(prefers-color-scheme: dark)'.
function stubMatchMedia(matches: boolean) {
  const listeners = new Set<() => void>();
  const mql = { matches };
  window.matchMedia = (() => ({
    get matches() { return mql.matches; },
    addEventListener: (_: string, cb: () => void) => listeners.add(cb),
    removeEventListener: (_: string, cb: () => void) => listeners.delete(cb),
  })) as unknown as typeof window.matchMedia;
  return {
    set(next: boolean) { mql.matches = next; listeners.forEach((cb) => cb()); },
    listenerCount: () => listeners.size,
  };
}

const theme = () => document.documentElement.getAttribute('data-theme');

afterEach(() => document.documentElement.removeAttribute('data-theme'));

describe('applyTheme', () => {
  it('forces light regardless of OS preference', () => {
    stubMatchMedia(true);
    applyTheme('light');
    expect(theme()).toBe('light');
  });

  it('forces dark regardless of OS preference', () => {
    stubMatchMedia(false);
    applyTheme('dark');
    expect(theme()).toBe('dark');
  });

  it('system follows matchMedia and tracks live OS changes', () => {
    const mm = stubMatchMedia(true);
    const cleanup = applyTheme('system');
    expect(theme()).toBe('dark');
    mm.set(false); // OS flips to light
    expect(theme()).toBe('light');
    cleanup();
    expect(mm.listenerCount()).toBe(0);
  });

  it('forced modes do not subscribe to matchMedia', () => {
    const mm = stubMatchMedia(true);
    applyTheme('dark');
    expect(mm.listenerCount()).toBe(0);
  });
});
