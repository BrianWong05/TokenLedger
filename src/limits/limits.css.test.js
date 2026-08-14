import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

// Comments come out first: this file's are long and prose-heavy, and a selector
// captured as "everything since the last closing brace" would otherwise carry
// the paragraph above it.
const css = readFileSync(resolve(process.cwd(), 'src/limits/limits.css'), 'utf8')
  .replace(/\/\*[\s\S]*?\*\//g, '');

// Every rule whose selector mentions the given fragment, body text included.
function rulesFor(selectorFragment) {
  const found = [];
  for (const [, selector, body] of css.matchAll(/([^{}]+)\{([^}]*)\}/g)) {
    if (selector.includes(selectorFragment)) found.push({ selector: selector.trim(), body });
  }
  return found;
}

const rule = (selector) => rulesFor(selector).find((r) => r.selector === selector);

describe('the evidence line’s layout', () => {
  // The spec: "The evidence line wraps below the bar without truncation,
  // horizontal scrolling, or shrinking the primary percentage. Localized copy
  // may use multiple lines." A localized line is longer than its English
  // original — the Chinese explanation runs to three lines in a narrow card — so
  // this is the property that keeps a translation from being cut off. jsdom
  // applies no CSS and measures no layout, so the check is on the source; a
  // browser pass covers what the source cannot say.
  it('wraps rather than truncating or scrolling', () => {
    const line = rule('.tl-lim-est');
    expect(line, '.tl-lim-est must exist').toBeTruthy();

    expect(line.body).toContain('flex-wrap: wrap');
    // Without this a flex item refuses to shrink below its content, which is
    // what pushes a long line past the card edge instead of onto a second row.
    expect(line.body).toContain('min-width: 0');
  });

  it('sets nothing anywhere on the line that could clip a translation', () => {
    for (const { selector, body } of rulesFor('.tl-lim-est')) {
      expect(body, `${selector} must not clip`).not.toMatch(
        /text-overflow|white-space:\s*nowrap|overflow-x|-webkit-line-clamp/,
      );
    }
  });

  it('leaves the primary percentage its own column to shrink out of', () => {
    // The line lives inside `.tl-lim-body`, which is the flex child that gives
    // way; the numeral is its sibling and keeps a floor. A line placed in the
    // row itself would take width from the numeral instead.
    expect(rule('.tl-lim-body').body).toContain('min-width: 0');
    expect(rule('.tl-lim-num').body).toMatch(/min-width:\s*62px/);
  });
});
