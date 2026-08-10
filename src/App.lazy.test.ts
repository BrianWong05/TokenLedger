import { expect, it, vi } from 'vitest';

const loaded = vi.hoisted(() => ({ pricing: 0, settings: 0 }));

vi.mock('./pricing/PricingPage', () => {
  loaded.pricing += 1;
  return { default: () => null };
});

vi.mock('./settings/SettingsPage', () => {
  loaded.settings += 1;
  return { default: () => null };
});

it('does not eagerly load inactive tabs with the dashboard module', async () => {
  await import('./App');

  expect(loaded.pricing).toBe(0);
  expect(loaded.settings).toBe(0);
});
