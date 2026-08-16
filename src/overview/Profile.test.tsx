/** @vitest-environment jsdom */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it } from 'vitest';
import Profile from './Profile';
import type { ProfileModel, ProfileView } from './data';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const mounted: Root[] = [];

const view = (over: Partial<ProfileView> = {}): ProfileView => ({
  models: [
    { name: 'claude-opus-4-6', tokens: 14_640_000_000, share: 0.488 },
    { name: 'kimi-for-coding', tokens: 8_160_000_000, share: 0.272 },
  ],
  windowTokens: 30_000_000_000,
  startedIso: '2025-05-12',
  activeDays: 192,
  ...over,
});

// N ranked Models, descending — enough to push past the top-N cut.
const manyModels = (n: number): ProfileModel[] =>
  Array.from({ length: n }, (_, i) => ({
    name: `model-${String(i).padStart(2, '0')}`,
    tokens: (n - i) * 1_000_000,
    share: (n - i) / 100,
  }));

function mount(profile: ProfileView) {
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  mounted.push(root);
  act(() => root.render(<Profile profile={profile} />));
  return container;
}

const rowNames = (el: HTMLElement) =>
  [...el.querySelectorAll('.tt-profile-model .name')].map((n) => n.textContent);

const toggle = (el: HTMLElement) => el.querySelector<HTMLButtonElement>('.tt-profile-more');

afterEach(() => {
  mounted.splice(0).forEach((root) => act(() => root.unmount()));
  document.body.innerHTML = '';
});

describe('Profile', () => {
  it('shows the window\'s top five Models, the rest behind a toggle', () => {
    const el = mount(view({ models: manyModels(7) }));
    expect(rowNames(el)).toEqual(['model-00', 'model-01', 'model-02', 'model-03', 'model-04']);
    expect(toggle(el)!.textContent).toContain('7');
  });

  it('expands to every Model in the window and collapses back', () => {
    const el = mount(view({ models: manyModels(7) }));
    act(() => toggle(el)!.click());
    expect(rowNames(el)).toHaveLength(7);
    expect(toggle(el)!.textContent).toContain('5');

    act(() => toggle(el)!.click());
    expect(rowNames(el)).toHaveLength(5);
  });

  it('offers no toggle when every Model already fits', () => {
    expect(toggle(mount(view({ models: manyModels(5) })))).toBeNull();
    expect(toggle(mount(view()))).toBeNull();
  });

  it('sizes each row bar to the same share it prints', () => {
    const el = mount(view());
    const rows = [...el.querySelectorAll<HTMLElement>('.tt-profile-model')];
    expect(rows[0].querySelector<HTMLElement>('.fill')!.style.width).toBe('48.8%');
    expect(rows[0].querySelector('.pct')!.textContent).toBe('48.8%');
    expect(rows[0].querySelector('.name')!.textContent).toBe('claude-opus-4-6');
    expect(rows[1].querySelector<HTMLElement>('.fill')!.style.width).toBe('27.2%');
  });

  it('separates a row\'s figures so they never read as one run', () => {
    const rows = [...mount(view()).querySelectorAll<HTMLElement>('.tt-profile-model .row')];
    expect(rows[0].textContent).toBe('1 claude-opus-4-6 14.64B 48.8%');
  });

  it('shows no Cost anywhere — the block is tokens only', () => {
    expect(mount(view()).textContent).not.toContain('$');
  });

  it('prints no window label, count or total above the rows', () => {
    // The card is the ranked list alone: the range picker already names the
    // window, and a second copy of it here was the thing readers had to
    // reconcile. First thing in the card is a Model row.
    const el = mount(view());
    expect(el.querySelector('.tt-profile')!.firstElementChild!.className).toBe('tt-profile-models');
  });

  it('keeps a Model row as text — only the toggle is a control', () => {
    const el = mount(view({ models: manyModels(7) }));
    expect(el.querySelector('.tt-profile-model [role="button"]')).toBeNull();
    expect(el.querySelector('.tt-profile-model button')).toBeNull();
    expect([...el.querySelectorAll('button')]).toHaveLength(1);
  });

  it('says the window holds no usage rather than borrowing another window\'s Models', () => {
    const el = mount(view({ models: [], windowTokens: 0 }));
    expect(el.querySelector('.tt-profile-empty')?.textContent).toBe('No usage in this window');
    expect(rowNames(el)).toEqual([]);
    expect(toggle(el)).toBeNull();
  });

  it('does not call a window of Unattributed usage empty', () => {
    // Rows come from Models, and Unattributed is never one — but its tokens are
    // real and the headline is counting them, so "no usage" would be a lie.
    const el = mount(view({ models: [], windowTokens: 40_000 }));
    expect(el.querySelector('.tt-profile-empty')?.textContent).toBe(
      'Only Unattributed usage in this window',
    );
  });

  it('reports the Ledger span in the footer, dashes included', () => {
    expect(mount(view()).querySelector('.tt-profile-foot')!.textContent).toContain('2025-05-12');
    expect(mount(view()).querySelector('.tt-profile-foot')!.textContent).toContain('192');
    const empty = mount(view({ models: [], windowTokens: 0, startedIso: null, activeDays: 0 }));
    expect(empty.querySelector('.tt-profile-foot')!.textContent).toContain('—');
  });
});
