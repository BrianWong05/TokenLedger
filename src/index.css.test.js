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

  // Acceptance 25 asks the Ready info control to have *visible* focus. jsdom
  // applies no CSS and computes no styles, so this takes the same discharge as
  // 26's wrapped-layout clause: assert the rule exists, and that the control
  // does not opt itself out of it.
  it('gives every button a focus ring the Limits info control cannot opt out of', () => {
    const index = readFileSync(resolve(process.cwd(), 'src/index.css'), 'utf8');
    const ring = /button:focus-visible\s*\{([^}]*)\}/.exec(index);

    expect(ring, 'a global button:focus-visible rule must exist').not.toBeNull();
    expect(ring[1]).toMatch(/outline:\s*2px\s+solid/);

    const limits = readFileSync(resolve(process.cwd(), 'src/limits/limits.css'), 'utf8')
      .replace(/\/\*[\s\S]*?\*\//g, '');
    let seen = 0;
    for (const [, selector, body] of limits.matchAll(/([^{}]+)\{([^}]*)\}/g)) {
      if (!selector.includes('tl-lim-est-info')) continue;
      seen += 1;
      expect(body, `${selector.trim()} must not remove the focus ring`).not.toMatch(
        /outline:\s*(none|0)\b/,
      );
    }
    expect(seen, 'no .tl-lim-est-info rules found — has the class moved?').toBeGreaterThan(0);
  });
});
