import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

describe('the hidden-text utility', () => {
  // The Limits evidence line shows "≈" and says "approximately" — two different
  // words for two audiences in one sentence — and only this rule keeps the spoken
  // one off the screen. jsdom applies no CSS, so every DOM test that asserts the
  // class NAME would still pass with the rule deleted and the word (and the whole
  // explanation paragraph beside it) plainly visible. This is the check that
  // fails instead, so the assertion is on the source.
  it('actually clips what it hides', () => {
    const css = readFileSync(resolve(process.cwd(), 'src/index.css'), 'utf8');
    const rule = /\.tl-sr-only\s*\{([^}]*)\}/.exec(css);

    expect(rule, '.tl-sr-only must exist in src/index.css').not.toBeNull();
    // Off-flow, sized to nothing, and clipped: any one of these missing leaves
    // the text somewhere on screen.
    for (const declaration of [
      'position: absolute',
      'width: 1px',
      'height: 1px',
      'overflow: hidden',
      'clip-path: inset(50%)',
    ]) {
      expect(rule[1], declaration).toContain(declaration);
    }
  });
});
