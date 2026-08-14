import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

// Comments come out first: this file's are long and prose-heavy, and a selector
// captured as "everything since the last closing brace" would otherwise carry
// the paragraph above it.
const source = readFileSync(resolve(process.cwd(), 'src/limits/limits.css'), 'utf8');
const css = source.replace(/\/\*[\s\S]*?\*\//g, '');

const rules = [...css.matchAll(/([^{}]+)\{([^}]*)\}/g)].map(([, selector, body]) => ({
  selector: selector.trim(),
  body,
}));

const rule = (selector) => rules.find((r) => r.selector === selector);
const touching = (classes) =>
  rules.filter((r) => classes.some((c) => r.selector.includes(c)));

// The line, everything inside it, and every ancestor up to the card it wraps
// inside. `white-space: nowrap` on an ancestor defeats the wrap exactly as
// surely as one on the line, so the scan does not stop at `.tl-lim-est` — that
// blind spot is the whole reason this file exists rather than a single
// `toContain`.
const WRAP_CHAIN = ['.tl-lim-est', '.tl-lim-body', '.tl-lim-row', '.tl-lim-card'];

// Anything that would stop the line wrapping or cut a translation off.
// `min-width` is deliberately absent — it is the mechanism, asserted below — and
// so is `overflow-y`, which the page sets to scroll vertically by design.
const CLIPPING =
  /text-overflow|white-space:\s*(nowrap|pre)\b|overflow(-x)?:\s*(hidden|scroll|auto)|-webkit-line-clamp|flex-wrap:\s*nowrap|max-width/;

describe('the evidence line’s layout', () => {
  // The spec: "The evidence line wraps below the bar without truncation,
  // horizontal scrolling, or shrinking the primary percentage. Localized copy
  // may use multiple lines." A translation is longer than its English original —
  // the Chinese explanation runs to several lines in a narrow card — so these are
  // the properties that keep it from being cut off.
  //
  // What this file can and cannot do: jsdom applies no CSS and measures no
  // layout, so nothing here proves the line *did* wrap at some width. It proves
  // the stylesheet declares nothing that would stop it, which is the part that
  // regresses silently under an edit. Measurement needs a browser.
  it('tells the line to wrap', () => {
    const line = rule('.tl-lim-est');
    expect(line, 'no `.tl-lim-est` rule — has the class been renamed?').toBeTruthy();
    expect(line.body).toContain('flex-wrap: wrap');
  });

  it('sets nothing on the line, its contents, or its ancestors that could clip a translation', () => {
    const scanned = touching(WRAP_CHAIN);
    // Without this the loop below passes by visiting nothing, which is exactly
    // how a class rename would make the assertion vacuous.
    expect(scanned.length, 'nothing in the wrap chain was found to scan').toBeGreaterThan(5);

    for (const { selector, body } of scanned) {
      expect(body, `${selector} must not clip the line`).not.toMatch(CLIPPING);
    }
  });

  it('leaves the primary percentage its own column to shrink out of', () => {
    // `.tl-lim-row` is the flex container; the body is the child that gives way
    // and needs `min-width: 0` to shrink below its content, and the numeral is
    // the sibling that keeps a floor. A line placed in the row itself would take
    // width from the numeral instead of from the body.
    expect(rule('.tl-lim-row').body).toContain('display: flex');
    expect(rule('.tl-lim-body').body).toContain('min-width: 0');
    expect(rule('.tl-lim-num').body).toMatch(/min-width:\s*62px/);
  });

  it('stays within what this parser can actually see', () => {
    // The rule split treats `@media (...) { .x { ... } }` as one rule whose
    // "selector" is the at-rule, so declarations nested inside one are invisible
    // to every check above. There are none today. If that changes, this fails
    // rather than letting the scans quietly stop covering half the file.
    expect(source, 'nested at-rules need a real parser here').not.toMatch(/@(media|supports|container)/);
  });
});
