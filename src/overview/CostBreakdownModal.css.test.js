import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const css = readFileSync(resolve(process.cwd(), 'src/overview/overview.css'), 'utf8');

function declarationsFor(selector) {
  const match = css.match(new RegExp(`\\.${selector}\\s*\\{([^}]*)\\}`));
  expect(match, `missing .${selector} rule`).not.toBeNull();
  return match[1];
}

describe('CostBreakdownModal scroll layers', () => {
  it('keeps pinned chrome and scrolling content in normal flow without compositor overlap', () => {
    const pinnedRule = declarationsFor('tt-cost-pinned-head');
    const scrollerRule = declarationsFor('tt-cost-modal-scroll');

    expect(pinnedRule).not.toMatch(/position\s*:\s*absolute/);
    expect(pinnedRule).not.toMatch(/(?:^|\n)\s*transform\s*:/);
    expect(scrollerRule).not.toMatch(/(?:^|\n)\s*transform\s*:/);
  });
});
