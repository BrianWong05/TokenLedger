import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const css = readFileSync(resolve(process.cwd(), 'src/settings/settings.css'), 'utf8');

function bodyOf(selector) {
  const match = css.match(new RegExp(`${selector.replace('.', '\\.')}\\s*\\{([^}]*)\\}`));
  expect(match, `missing ${selector} rule`).not.toBeNull();
  return match[1];
}

describe('panel-card drag', () => {
  // Picking up a row near the top used to overflow-anchor the Updates group
  // into view, which is the jump to the bottom of Settings. These three
  // declarations are the CSS half of that fix; jsdom applies no styles, so
  // the assertion is on the source.
  it('does not overflow-anchor the page when a row is picked up', () => {
    for (const selector of ['.tl-page-settings', '.set-panelcards', '.set-row-panelcard']) {
      expect(bodyOf(selector), selector).toMatch(/overflow-anchor:\s*none/);
    }
  });

  it('paints the card fill on the row that layout-animates', () => {
    expect(bodyOf('.set-row-panelcard')).toMatch(/background:\s*var\(--card\)/);
  });
});
