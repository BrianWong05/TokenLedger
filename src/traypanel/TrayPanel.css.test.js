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

// The macOS card is declared twice: the Dark-mode veil, then the heavier one
// inside the prefers-color-scheme branch. In source order, first is dark.
function veilAlphas() {
  const rules = css.match(/body\.tp-macos \.tp\s*\{[^}]*\}/g) ?? [];
  expect(rules, 'expected a Dark-mode veil and a Light-mode one').toHaveLength(2);
  // The background only — the border and the inset rim carry alphas of their
  // own, and they are not the veil.
  return rules.map((r) =>
    (r.match(/background\s*:\s*[^;]+/)[0].match(/rgba\([^)]*\)/g) ?? []).map((s) =>
      Number(s.match(/,\s*([\d.]+)\s*\)$/)[1]),
    ),
  );
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

  // Which way the material went decides how much veil the text needs. Dark mode
  // gets its darkness from the material, so a heavy veil there buries it and the
  // card reads as flat paint again; Light mode has a near-white material behind
  // dark-only text, so the veil is the only thing holding contrast up.
  it('veils Light mode more heavily than Dark, where the material already darkens', () => {
    const [dark, light] = veilAlphas();
    expect(dark).toHaveLength(light.length);
    dark.forEach((a, i) => expect(a, `stop ${i}`).toBeLessThan(light[i]));
  });
});
