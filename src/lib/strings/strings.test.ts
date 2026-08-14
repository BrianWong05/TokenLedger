import { describe, expect, it } from 'vitest';
import { common } from './common';
import { limits } from './limits';
import { overview } from './overview';
import { pricing } from './pricing';
import { settings } from './settings';

const DICTS = { common, limits, overview, pricing, settings };

describe('strings', () => {
  // The same copy is read on macOS, Windows, and Linux, so naming one of them
  // describes someone else's computer to two thirds of the readers. This bit
  // hardest in the privacy lines, which are the strings it matters most to get
  // right: they told a Windows user their data stayed on a Mac.
  it('names no operating system, in any language', () => {
    for (const [name, dict] of Object.entries(DICTS)) {
      for (const [lang, entries] of Object.entries(dict)) {
        for (const [key, value] of Object.entries(entries as Record<string, string>)) {
          // A Limit is a rolling *window* (CONTEXT.md), so the plural is domain
          // vocabulary the Limits tab has to be able to say. Only the domain
          // phrases are excused, each one deliberately; "on Windows" and any
          // other bare mention still trip the guard.
          const copy = value.replace(/\b(vendor|completed|Limit) windows\b/gi, '');
          expect(copy, `${name}.${lang}.${key}`).not.toMatch(
            /\bmac\b|\bmacs\b|macos|\bosx\b|windows|\blinux\b/i,
          );
        }
      }
    }
  });

  // The translator falls back to English for a missing key, so a half-added
  // string is invisible in English and silently bilingual for everyone else.
  // Every dictionary carries the same key universe in both languages.
  it('says everything in both languages', () => {
    for (const [name, dict] of Object.entries(DICTS)) {
      const en = Object.keys((dict as Record<string, object>).en).sort();
      const zh = Object.keys((dict as Record<string, object>)['zh-Hant']).sort();
      expect(zh, name).toEqual(en);
    }
  });

  // CONTEXT.md's entry for *Limit* carries an _Avoid_ list, and docs/agents/
  // domain.md makes it binding: a glossary term must not be swapped for a synonym
  // the glossary rejects. The Limits copy reached for "quota" anyway, in both
  // languages, in the very paragraph that discloses what the feature does — so
  // the rule is a test now rather than a comment. The domain word is "window".
  it('uses the Limits vocabulary, not the synonyms CONTEXT.md rejects', () => {
    const rejected = /\bquota\b|配額|\ballowance\b|\bthrottl|\brate limit/i;
    for (const [lang, entries] of Object.entries(limits)) {
      for (const [key, value] of Object.entries(entries as Record<string, string>)) {
        expect(value, `limits.${lang}.${key}`).not.toMatch(rejected);
      }
    }
  });
});
