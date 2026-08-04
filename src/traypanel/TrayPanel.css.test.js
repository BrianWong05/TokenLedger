import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const css = readFileSync(resolve(process.cwd(), 'src/traypanel/TrayPanel.css'), 'utf8');
const conf = JSON.parse(
  readFileSync(resolve(process.cwd(), 'src-tauri/tauri.conf.json'), 'utf8'),
);

function backgroundOf(rule) {
  const match = css.match(new RegExp(`${rule}\\s*\\{([^}]*)\\}`));
  expect(match, `missing ${rule} rule`).not.toBeNull();
  return match[1].match(/background\s*:\s*([^;]+)/)[1].trim();
}

describe('TrayPanel translucency', () => {
  // Three things have to agree and they live in three files: the material
  // (tauri.conf.json), the card that lets it through (here), and the body
  // class that scopes the card (TrayPanel.tsx, asserted in TrayPanel.test.tsx).
  // Each fails silently alone — a material behind an opaque card looks
  // untouched, and a translucent card with no material is bare desktop.
  it('lets the card go translucent only where a material is painted behind it', () => {
    // Every stop, not just the first: an opaque one anywhere down the gradient
    // occludes the material across that band of the card.
    const stops = backgroundOf('body\\.tp-macos \\.tp').match(/rgba\([^)]*\)/g);
    expect(stops, 'the macOS card must be built from rgba() so the material shows through')
      .not.toBeNull();
    for (const stop of stops) {
      expect(Number(stop.match(/,\s*([\d.]+)\s*\)$/)[1]), stop).toBeLessThan(1);
    }

    const panel = conf.app.windows.find((w) => w.label === 'traypanel');
    expect(panel?.windowEffects?.effects).toContain('popover');
  });

  // The window is transparent on every platform (ADR-0010: one window list),
  // and Windows gets no material, so an unscoped translucent card would leave
  // its panel see-through with nothing blurring behind it.
  it('keeps the base card opaque, so platforms with no material keep a solid panel', () => {
    expect(backgroundOf('(?:^|\\n)\\.tp')).toBe('#1e1e24');
  });
});
