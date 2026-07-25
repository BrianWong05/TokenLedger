/** @vitest-environment jsdom */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it } from 'vitest';
import Profile from './Profile';
import type { ProfileView } from './data';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const mounted: Root[] = [];

const view = (over: Partial<ProfileView> = {}): ProfileView => ({
  d7: 1_900_000_000,
  d30: 4_500_000_000,
  perActiveDay: 156_300_000,
  sessions30d: 10_800,
  models: [
    { name: 'claude-opus-4-6', tokens: 14_640_000_000, share: 0.488 },
    { name: 'kimi-for-coding', tokens: 8_160_000_000, share: 0.272 },
  ],
  startedIso: '2025-05-12',
  activeDays: 192,
  ...over,
});

function mount(profile: ProfileView) {
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  mounted.push(root);
  act(() => root.render(<Profile profile={profile} />));
  return container;
}

afterEach(() => {
  mounted.splice(0).forEach((root) => act(() => root.unmount()));
  document.body.innerHTML = '';
});

describe('Profile', () => {
  it('renders the four windowed tiles and the lifetime footer', () => {
    const el = mount(view());
    const tiles = [...el.querySelectorAll('.tt-profile-tile')].map((t) => [
      t.querySelector('.num')?.textContent,
      t.querySelector('.lbl')?.textContent,
    ]);
    expect(tiles).toEqual([
      ['1.90B', '7d'],
      ['4.50B', '30d'],
      ['156.3M', 'per active day'],
      ['10.8K', 'sessions · 30d'],
    ]);
    expect(el.querySelector('.tt-profile-foot')?.textContent).toContain('2025-05-12');
    expect(el.querySelector('.tt-profile-foot')?.textContent).toContain('192');
  });

  it('sizes each row bar to the same share it prints', () => {
    const el = mount(view());
    const rows = [...el.querySelectorAll<HTMLElement>('.tt-profile-model')];
    expect(rows[0].querySelector<HTMLElement>('.fill')!.style.width).toBe('48.8%');
    expect(rows[0].querySelector('.pct')!.textContent).toBe('48.8%');
    expect(rows[0].querySelector('.name')!.textContent).toBe('claude-opus-4-6');
    expect(rows[1].querySelector<HTMLElement>('.fill')!.style.width).toBe('27.2%');
  });

  it('shows no Cost anywhere — the block is tokens only', () => {
    expect(mount(view()).textContent).not.toContain('$');
  });

  it('is not clickable: a row is text, not a control', () => {
    const el = mount(view());
    expect(el.querySelector('button')).toBeNull();
    expect(el.querySelector('[role="button"]')).toBeNull();
  });

  it('renders an empty Ledger as dashes and zeros, never as a false average', () => {
    const el = mount(
      view({ d7: 0, d30: 0, perActiveDay: null, sessions30d: 0, models: [], startedIso: null, activeDays: 0 }),
    );
    const nums = [...el.querySelectorAll('.tt-profile-tile .num')].map((n) => n.textContent);
    expect(nums).toEqual(['0', '0', '—', '0']); // 0 tokens is true; an unknown average is not 0
    expect(el.querySelector('.tt-profile-empty')?.textContent).toBe('No usage yet');
    expect(el.querySelector('.tt-profile-foot')?.textContent).toContain('—');
  });

  it('dashes the Session tile while its own fetch has not landed', () => {
    const el = mount(view({ sessions30d: null }));
    const nums = [...el.querySelectorAll('.tt-profile-tile .num')].map((n) => n.textContent);
    expect(nums[3]).toBe('—');
    expect(nums[0]).toBe('1.90B'); // the rest does not wait on it
  });
});
