import { describe, expect, it } from 'vitest';
import { common } from './common';
import { overview } from './overview';
import { pricing } from './pricing';
import { settings } from './settings';

const DICTS = { common, overview, pricing, settings };

describe('strings', () => {
  // The same copy is read on macOS, Windows, and Linux, so naming one of them
  // describes someone else's computer to two thirds of the readers. This bit
  // hardest in the privacy lines, which are the strings it matters most to get
  // right: they told a Windows user their data stayed on a Mac.
  it('names no operating system, in any language', () => {
    for (const [name, dict] of Object.entries(DICTS)) {
      for (const [lang, entries] of Object.entries(dict)) {
        for (const [key, value] of Object.entries(entries as Record<string, string>)) {
          expect(value, `${name}.${lang}.${key}`).not.toMatch(
            /\bmac\b|\bmacs\b|macos|\bosx\b|windows|\blinux\b/i,
          );
        }
      }
    }
  });
});
