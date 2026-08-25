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

// sRGB relative luminance and WCAG contrast, for the veil/type checks below.
function luminance([r, g, b]) {
  const lin = (c) => (c / 255 <= 0.04045 ? c / 255 / 12.92 : ((c / 255 + 0.055) / 1.055) ** 2.4);
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}
function contrast(a, b) {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}
const hex = (h) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));
const token = (name) => hex(css.match(new RegExp(`${name}\\s*:\\s*(#[0-9a-f]{6})`))[1]);
// One stop of the Dark veil composited over `material`. Stop 0 is the card's
// lightest band (worst place to read type); stop 1 is the mid band the eye
// reads as "how dark is this card".
function cardBand(material, stop) {
  // The background declaration only, like veilAlphas: the same rule carries a
  // border-colour and two inset rims in rgba, and compositing one of those
  // would pass this check for a colour the card never paints.
  const rule = css.match(/body\.tp-macos \.tp\s*\{[^}]*\}/)[0];
  const veil = rule.match(/background\s*:\s*[^;]+/)[0];
  const [r, g, b, a] = veil
    .match(/rgba\([^)]*\)/g)[stop]
    .match(/[\d.]+/g)
    .map(Number);
  return [r, g, b].map((c, i) => Math.round(a * c + (1 - a) * material[i]));
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

  // Both veils are heavy now (see the CSS header: a Dark-mode material over a
  // white document comes out light too), but not equally: the Light-mode
  // material starts from near-white, so it needs the heavier of the two. This
  // ordering is what keeps the card reading as glass rather than paint in Dark
  // mode, where there is at least some darkness behind it to let through.
  it('veils Light mode more heavily than Dark, which has darker material to let through', () => {
    const [dark, light] = veilAlphas();
    expect(dark).toHaveLength(light.length);
    dark.forEach((a, i) => expect(a, `stop ${i}`).toBeLessThan(light[i]));
  });

  // The material transmits whatever is behind the window, so "Dark mode" is no
  // promise that the card comes out dark: measured on a real panel over a white
  // document it composited to rgb(65,67,70), where the old quiet grey sat at
  // 1.7:1. rgb(78,80,84) is that one measurement solved back to the material's
  // own output, assuming the flat source-over the CSS describes — the real
  // layer is blurred vibrancy, and a brighter backdrop than that sample would
  // go lighter still. So this is not a readability proof for every desktop.
  // 2.2 sits just under what the chosen grey scores (2.33), which makes this a
  // guard against sliding back toward the #6e6e76 that provoked the change
  // (1.68) — not a pin on the exact shade picked: the four steps immediately
  // below it still clear the floor, and #808089 is the first that does not.
  //
  // The veil cannot substitute for the type colour on this band — the card's
  // top stop (72,72,88) is about as light as the material itself, so no alpha
  // moves it much. The veil's own guard is the next test.
  it('pins the quiet type against the brightest card this design has been measured at', () => {
    const lightest = cardBand([78, 80, 84], 0);
    expect(contrast(token('--tp-faint'), lightest)).toBeGreaterThan(2.2);
  });

  // And this is the appearance one: whatever the type does, a card washed to
  // rgb(65,67,70) does not read as this panel. The mid stop is where the veil
  // has real leverage, so that is where the floor goes — thin it back out and
  // the card drifts up toward the measured wash.
  it('keeps the card itself dark when a white document lights the material', () => {
    const mid = cardBand([78, 80, 84], 1);
    expect(luminance(mid)).toBeLessThan(luminance([58, 58, 64]));
  });

  // Three tiers, and the panel leans on their order for hierarchy: each must
  // stay quieter than the one above it, or lifting one for contrast silently
  // flattens the panel into a single shade of grey. The loudest tier is read
  // out of the card rule rather than written here, so this cannot go on
  // passing while the card's own colour moves under it.
  it('keeps the quiet type tiers in order', () => {
    const card = css.match(/(?:^|\n)\.tp\s*\{([^}]*)\}/)[1];
    const body = hex(card.match(/(?:^|;)\s*color\s*:\s*(#[0-9a-f]{6})/)[1]);
    expect(luminance(token('--tp-faint'))).toBeLessThan(luminance(token('--tp-quiet')));
    expect(luminance(token('--tp-quiet'))).toBeLessThan(luminance(body));
  });

  // The window and the card must agree on size and corner: the conf sizes the
  // window and rounds the material, the CSS sizes and rounds the card, and
  // TrayPanel.tsx re-asserts the width on every ResizeObserver resize. Apart
  // they show as a clipped card, material poking past the corners, or a panel
  // that snaps widths on first paint. (tray.rs's PANEL_WIDTH is pinned to the
  // same conf by a Rust test.)
  it('keeps the card and the resize width in step with the window and its material', () => {
    const panel = conf.app.windows.find((w) => w.label === 'traypanel');
    const card = css.match(/(?:^|\n)\.tp\s*\{([^}]*)\}/)[1];
    expect(card.match(/(?:^|;)\s*width\s*:\s*([^;]+)/)[1].trim()).toBe(`${panel.width}px`);
    expect(card.match(/border-radius\s*:\s*([^;]+)/)[1].trim()).toBe(
      `${panel.windowEffects.radius}px`,
    );
    const tsx = readFileSync(resolve(process.cwd(), 'src/traypanel/TrayPanel.tsx'), 'utf8');
    expect(tsx.match(/const PANEL_WIDTH = (\d+)/)[1]).toBe(String(panel.width));
  });
});
