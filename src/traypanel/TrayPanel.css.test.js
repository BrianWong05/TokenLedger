import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const css = readFileSync(resolve(process.cwd(), 'src/traypanel/TrayPanel.css'), 'utf8');
const conf = JSON.parse(
  readFileSync(resolve(process.cwd(), 'src-tauri/tauri.conf.json'), 'utf8'),
);

function declarationsFor(selector) {
  const match = css.match(new RegExp(`\\.${selector}\\s*\\{([^}]*)\\}`));
  expect(match, `missing .${selector} rule`).not.toBeNull();
  return match[1];
}

describe('TrayPanel translucency', () => {
  // The two halves live in different files and different languages, and either
  // one alone fails silently: the material behind an opaque card looks
  // untouched, and a translucent card with no material is raw see-through
  // desktop. Assert both so a half-revert can't pass.
  it('pairs a translucent card with the macOS material that blurs what shows through', () => {
    const alpha = declarationsFor('tp').match(
      /background\s*:\s*rgba\([^)]*,\s*([\d.]+)\s*\)/,
    );
    expect(alpha, '.tp background must be rgba() so the material shows through').not.toBeNull();
    expect(Number(alpha[1])).toBeLessThan(1);

    const panel = conf.app.windows.find((w) => w.label === 'traypanel');
    expect(panel?.windowEffects?.effects).toContain('popover');
  });
});
