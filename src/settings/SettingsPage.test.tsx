/** @vitest-environment jsdom */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import SettingsPage from './SettingsPage';
import FirstRunDialog from './FirstRunDialog';
import { SettingsProvider, useSettings } from './SettingsContext';
import { makeFakeSettings, type FakeSettings } from './settings.fake';
import { setLaunchAtLogin } from './startup';
import { I18nProvider } from '../lib/i18n';
import { STORAGE_KEY } from '../overview/useAutoRefresh';
import { CUSTOM_PRESETS_KEY } from '../overview/customPresets';
import { publishFirstRecord } from '../overview/ledgerExtent';
import type { UpdateStatus } from './settings';

vi.mock('@tauri-apps/api/app', () => ({ getVersion: vi.fn().mockResolvedValue('1.4.2') }));
vi.mock('./startup', () => ({ setLaunchAtLogin: vi.fn() }));

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

// applyTheme() reads matchMedia (jsdom has none); default it to light ('system'
// resolves to light when matches is false).
beforeEach(() => {
  vi.clearAllMocks();
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
});

// Mirrors App's AppInner wiring so language flows from context and the first-run
// dialog mounts on the same gate.
function Harness({ port }: { port: FakeSettings }) {
  return (
    <SettingsProvider port={port}>
      <Inner port={port} />
    </SettingsProvider>
  );
}
function Inner({ port }: { port: FakeSettings }) {
  const { settings, loaded } = useSettings();
  return (
    <I18nProvider lang={settings.language}>
      <SettingsPage port={port} />
      {loaded && !settings.firstRunDone && <FirstRunDialog />}
    </I18nProvider>
  );
}

const mountedRoots: Root[] = [];

async function settle(times = 4) {
  for (let i = 0; i < times; i++) {
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });
  }
}

async function mount(port: FakeSettings): Promise<HTMLElement> {
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  mountedRoots.push(root);
  await act(async () => {
    root.render(<Harness port={port} />);
  });
  await settle();
  return container;
}

// Set a controlled input value through the native setter so React's value
// tracking sees the change, then fire the events React listens for.
async function setValue(el: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!;
  await act(async () => {
    setter.call(el, value);
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
  });
}

async function blur(el: HTMLElement) {
  await act(async () => {
    el.blur();
    el.dispatchEvent(new FocusEvent('blur', { bubbles: false }));
    el.dispatchEvent(new FocusEvent('focusout', { bubbles: true }));
  });
}

async function click(el: Element) {
  await act(async () => {
    (el as HTMLElement).click();
  });
}

afterEach(() => {
  for (const root of mountedRoots.splice(0)) act(() => root.unmount());
  document.body.replaceChildren();
});

const seg = (c: HTMLElement) =>
  Array.from(c.querySelectorAll('.set-seg[aria-label="Theme"] button')) as HTMLButtonElement[];
// Two mono segments now carry interval choices — the Overview's auto-refresh
// and the Menu Bar Extra's — so each is addressed by its own label rather than
// by the shared class.
const refreshSeg = (c: HTMLElement) =>
  Array.from(
    c.querySelectorAll('.set-seg-mono[aria-label="Auto-refresh interval"] button'),
  ) as HTMLButtonElement[];
const menuBarSeg = (c: HTMLElement) =>
  Array.from(
    c.querySelectorAll('.set-seg-mono[aria-label="Menu Bar Extra refresh interval"] button'),
  ) as HTMLButtonElement[];
const q = <T extends Element>(c: ParentNode, s: string) => c.querySelector(s) as T | null;

// The exchange rate shares .set-rate-input with the Custom-range day count, so
// it is addressed by its own label.
const RATE_INPUT = 'input[aria-label="Exchange rate"]';

// The dropdowns are the page's own button + menu, not a <select> (a native
// option list is drawn by the OS and takes no app styling): the button carries
// the chosen value, and choosing means open it, then click the option.
const dropdown = (c: ParentNode, label: string) =>
  q<HTMLButtonElement>(c, `button[aria-label="${label}"]`)!;
const chosen = (btn: HTMLButtonElement) => btn.dataset.value;
const option = (btn: HTMLButtonElement, v: string) =>
  btn.parentElement!.querySelector(`.set-menu-item[data-value="${v}"]`) as HTMLButtonElement | null;

async function pick(btn: HTMLButtonElement, value: string) {
  await click(btn);
  await click(option(btn, value)!);
}

describe('SettingsPage', () => {
  it('renders all five items from the fake settings', async () => {
    const port = makeFakeSettings({ firstRunDone: true, currency: 'HKD', usdRate: 7.8 });
    const c = await mount(port);
    const text = c.textContent ?? '';

    // Appearance: theme segment + language dropdown.
    expect(seg(c).map((b) => b.textContent)).toEqual(['System', 'Light', 'Dark']);
    expect(seg(c).find((b) => b.classList.contains('active'))?.textContent).toBe('System');
    expect(chosen(dropdown(c, 'Language'))).toBe('en');

    // Display currency + exchange rate (non-USD shows the rate row).
    expect(chosen(dropdown(c, 'Currency'))).toBe('HKD');
    expect(q<HTMLInputElement>(c, RATE_INPUT)?.value).toBe('7.8');

    // Startup + updates.
    expect(c.querySelector('[aria-label="Launch at login"]')).not.toBeNull();
    expect(text).toContain('Version 1.4.2');
    expect(text).toContain('Check for updates');
    expect(text).toContain('Nothing leaves this computer.');
  });

  it('keeps a pinned window-drag strip at the window top', async () => {
    const c = await mount(makeFakeSettings({ firstRunDone: true }));
    expect(c.querySelector('.tl-set-dragstrip')?.hasAttribute('data-tauri-drag-region')).toBe(true);
  });

  it('renders the Scanning group and persists the auto-refresh interval', async () => {
    localStorage.removeItem(STORAGE_KEY);
    const port = makeFakeSettings({ firstRunDone: true });
    const c = await mount(port);

    // Off + four presets + Custom; 30s active by default (parseRefreshSec fallback).
    expect(refreshSeg(c).map((b) => b.textContent))
      .toEqual(['Off', '10s', '30s', '60s', '5m', 'Custom']);
    expect(refreshSeg(c).find((b) => b.classList.contains('active'))?.textContent).toBe('30s');

    await click(refreshSeg(c).find((b) => b.textContent === '60s')!);

    expect(refreshSeg(c).find((b) => b.classList.contains('active'))?.textContent).toBe('60s');
    expect(localStorage.getItem(STORAGE_KEY)).toBe('60');
  });

  it('opens a custom interval row that persists an in-range integer', async () => {
    localStorage.removeItem(STORAGE_KEY);
    const port = makeFakeSettings({ firstRunDone: true });
    const c = await mount(port);

    const custom = () => refreshSeg(c).find((b) => b.textContent === 'Custom')!;
    const input = () => q<HTMLInputElement>(c, 'input[aria-label="Custom interval"]');

    // No custom row until Custom is chosen.
    expect(input()).toBeNull();

    await click(custom());
    expect(custom().classList.contains('active')).toBe(true);
    expect(input()?.value).toBe('30'); // seeded from the stored seconds

    await setValue(input()!, '90');
    expect(localStorage.getItem(STORAGE_KEY)).toBe('90');
    expect(custom().classList.contains('active')).toBe(true); // stays active

    // Invalid text stays editable but is never persisted.
    await setValue(input()!, 'abc');
    expect(input()?.value).toBe('abc');
    expect(localStorage.getItem(STORAGE_KEY)).toBe('90');

    // Choosing a preset closes the row and persists the preset.
    await click(refreshSeg(c).find((b) => b.textContent === '30s')!);
    expect(input()).toBeNull();
    expect(localStorage.getItem(STORAGE_KEY)).toBe('30');
  });

  it('theme segment click persists and applies data-theme immediately', async () => {
    const port = makeFakeSettings({ firstRunDone: true });
    const c = await mount(port);

    await click(seg(c).find((b) => b.textContent === 'Dark')!);

    expect(document.documentElement.getAttribute('data-theme')).toBe('dark');
    expect(port.calls.set[port.calls.set.length - 1]?.theme).toBe('dark');
    expect(port.value.theme).toBe('dark');
  });

  it('language switch re-renders visible strings in Traditional Chinese', async () => {
    const port = makeFakeSettings({ firstRunDone: true });
    const c = await mount(port);

    expect(c.textContent).toContain('Appearance');
    await pick(dropdown(c, 'Language'), 'zh-Hant');

    expect(c.textContent).toContain('外觀'); // Appearance
    expect(c.textContent).toContain('主題'); // Theme
    expect(c.textContent).not.toContain('Appearance');
    expect(port.value.language).toBe('zh-Hant');
  });

  it('hides the rate row for USD, shows it for another currency and persists a valid rate', async () => {
    const port = makeFakeSettings({ firstRunDone: true, currency: 'USD' });
    const c = await mount(port);

    expect(q<HTMLInputElement>(c, RATE_INPUT)).toBeNull();

    await pick(dropdown(c, 'Currency'), 'HKD');
    expect(q<HTMLInputElement>(c, RATE_INPUT)).not.toBeNull();

    await setValue(q<HTMLInputElement>(c, RATE_INPUT)!, '7.85');
    expect(port.value.currency).toBe('HKD');
    expect(port.value.usdRate).toBe(7.85);
  });

  // Custom range: up to four configured shortcuts for the Overview's range
  // picker. One add row builds them; each configured one gets a row of its own,
  // captioned with the window it resolves to. Slots stay positional and never
  // compact, so the stored value is always four entries with holes where one was
  // removed.
  describe('custom range presets', () => {
    const stored = () => JSON.parse(localStorage.getItem(CUSTOM_PRESETS_KEY) ?? 'null');
    const types = (c: HTMLElement) =>
      Array.from(
        c.querySelectorAll('.set-seg[aria-label="Preset type"] button'),
      ) as HTMLButtonElement[];
    const type = (c: HTMLElement, text: string) => types(c).find((b) => b.textContent === text)!;
    const dayInput = (c: HTMLElement) => q<HTMLInputElement>(c, 'input[aria-label="Day count"]');
    const addBtn = (c: HTMLElement) =>
      Array.from(c.querySelectorAll('.set-btn')).find(
        (b) => b.textContent === 'Add',
      ) as HTMLButtonElement;
    const arrows = (c: HTMLElement, dir: 'up' | 'down') =>
      Array.from(
        c.querySelectorAll(`button[aria-label^="Move ${dir} "]`),
      ) as HTMLButtonElement[];
    const removes = (c: HTMLElement) =>
      Array.from(c.querySelectorAll('button[aria-label^="Remove "]')) as HTMLButtonElement[];
    // Each configured Preset, by the label its row shows.
    const shortcuts = (c: HTMLElement) =>
      removes(c).map((b) => b.getAttribute('aria-label')!.replace('Remove ', ''));
    const captionOf = (c: HTMLElement, n: number) =>
      removes(c)[n].closest('.set-row')!.querySelector('.set-row-caption')!.textContent;

    // The extent is module state published by the Overview; captions resolve
    // against the plain calendar period until it lands.
    beforeEach(() => {
      localStorage.removeItem(CUSTOM_PRESETS_KEY);
      publishFirstRecord('');
    });

    it('starts with none configured and adds the seeded rolling preset', async () => {
      const c = await mount(makeFakeSettings({ firstRunDone: true }));

      expect(c.textContent).toContain('None yet');
      expect(shortcuts(c)).toEqual([]);
      expect(dayInput(c)?.value).toBe('14'); // first count nothing has claimed

      await click(addBtn(c));

      expect(stored()).toEqual([{ key: 'rolling', days: 14 }, null, null, null]);
      expect(shortcuts(c)).toEqual(['Last 14 days']);
      expect(c.textContent).not.toContain('None yet');
    });

    it('captions a preset with the window it resolves to', async () => {
      const c = await mount(makeFakeSettings({ firstRunDone: true }));
      await click(addBtn(c));

      // 14 days ending today, so only the span is fixed enough to assert on;
      // presetsOf owns the dates and is tested against them directly.
      expect(captionOf(c, 0)).toMatch(/^.+ – .+ · 14 days$/);
    });

    it('says so when the Ledger has no history for a preset yet', async () => {
      // A Ledger that starts today: last year ended long before the first
      // record, so the picker will not offer that preset at all.
      publishFirstRecord(new Date().toISOString().slice(0, 10));
      const c = await mount(makeFakeSettings({ firstRunDone: true }));
      await click(type(c, 'Last year'));
      await click(addBtn(c));

      expect(captionOf(c, 0)).toContain('not offered yet');
    });

    it('adds a calendar period and will not offer it twice', async () => {
      const c = await mount(makeFakeSettings({ firstRunDone: true }));

      await click(type(c, 'Last quarter'));
      expect(dayInput(c)).toBeNull(); // no day field outside the rolling type
      await click(addBtn(c));

      expect(stored()[0]).toEqual({ key: 'lastQuarter' });
      expect(type(c, 'Last quarter').disabled).toBe(true);
      expect(type(c, 'Last month').disabled).toBe(false);
      // adding one falls back to the type that is always available
      expect(dayInput(c)?.value).toBe('14');
    });

    it('stops adding at four', async () => {
      const c = await mount(makeFakeSettings({ firstRunDone: true }));
      for (const label of ['Last N days', 'Last month', 'Last quarter', 'Last year']) {
        await click(type(c, label));
        await click(addBtn(c));
      }

      expect(shortcuts(c)).toHaveLength(4);
      expect(addBtn(c).disabled).toBe(true);
    });

    it('keeps an out-of-bounds or duplicate day count editable without adding it', async () => {
      const c = await mount(makeFakeSettings({ firstRunDone: true }));
      await setValue(dayInput(c)!, '21');
      await click(addBtn(c));

      // 1 is today alone (Yesterday means something else), 1826 is past the
      // ceiling, 90 is the shipped Last-90-days shortcut, 21 is now taken, and
      // 'abc' is not a number at all.
      for (const bad of ['1', '1826', '90', '21', 'abc', '']) {
        await setValue(dayInput(c)!, bad);
        expect(dayInput(c)?.value).toBe(bad); // stays editable
        expect(addBtn(c).disabled).toBe(true);
      }
      expect(stored()).toEqual([{ key: 'rolling', days: 21 }, null, null, null]);

      // 7 and 30 shadow the Week and Month segments and are allowed anyway:
      // that no-repeat rule governs the set we ship, not the reader's own.
      for (const ok of ['7', '30', '2', '1825']) {
        await setValue(dayInput(c)!, ok);
        expect(addBtn(c).disabled).toBe(false);
      }
    });

    it('leaves a removed preset as a hole for the next one to fill', async () => {
      const c = await mount(makeFakeSettings({ firstRunDone: true }));
      await click(type(c, 'Last month'));
      await click(addBtn(c));
      await click(type(c, 'Last year'));
      await click(addBtn(c));

      await click(removes(c)[0]);

      expect(stored()).toEqual([null, { key: 'lastYear' }, null, null]);
      expect(shortcuts(c)).toEqual(['Last year']);

      await click(addBtn(c));
      expect(stored()).toEqual([{ key: 'rolling', days: 14 }, { key: 'lastYear' }, null, null]);
    });

    // Update: a rolling preset's count is editable in its own row, so changing
    // one does not mean removing it and adding it back at the bottom.
    it('edits a rolling preset in place, keeping its position', async () => {
      localStorage.setItem(
        CUSTOM_PRESETS_KEY,
        JSON.stringify([{ key: 'rolling', days: 14 }, { key: 'lastYear' }, null, null]),
      );
      const c = await mount(makeFakeSettings({ firstRunDone: true }));
      const field = () => q<HTMLInputElement>(c, 'input[aria-label="Day count Last 14 days"]')!;

      await setValue(field(), '45');
      expect(stored()[0]).toEqual({ key: 'rolling', days: 14 }); // not while typing
      await blur(field());

      expect(stored()).toEqual([{ key: 'rolling', days: 45 }, { key: 'lastYear' }, null, null]);
      expect(shortcuts(c)).toEqual(['Last 45 days', 'Last year']);
      expect(captionOf(c, 0)).toMatch(/· 45 days$/);
    });

    it('falls back to the stored count when the edit is not a preset', async () => {
      localStorage.setItem(
        CUSTOM_PRESETS_KEY,
        JSON.stringify([{ key: 'rolling', days: 14 }, { key: 'rolling', days: 30 }, null, null]),
      );
      const c = await mount(makeFakeSettings({ firstRunDone: true }));
      const field = () => q<HTMLInputElement>(c, 'input[aria-label="Day count Last 14 days"]')!;

      // 1 is below the floor, 1826 past the ceiling, 90 is a shipped shortcut,
      // 30 belongs to the other row, and 'abc' is not a number.
      for (const bad of ['1', '1826', '90', '30', 'abc', '']) {
        await setValue(field(), bad);
        await blur(field());
        expect(field().value).toBe('14');
        expect(stored()[0]).toEqual({ key: 'rolling', days: 14 });
      }
    });

    it('moves a preset past the row below it, and the picker follows', async () => {
      const c = await mount(makeFakeSettings({ firstRunDone: true }));
      await click(type(c, 'Last month'));
      await click(addBtn(c));
      await click(type(c, 'Last year'));
      await click(addBtn(c));
      expect(shortcuts(c)).toEqual(['Last month', 'Last year']);

      await click(arrows(c, 'down')[0]);

      // Slot order is the picker's order, so the store is the assertion.
      expect(stored()).toEqual([{ key: 'lastYear' }, { key: 'lastMonth' }, null, null]);
      expect(shortcuts(c)).toEqual(['Last year', 'Last month']);

      await click(arrows(c, 'up')[1]);
      expect(shortcuts(c)).toEqual(['Last month', 'Last year']);
    });

    it('swaps past a hole without filling it', async () => {
      // Three presets, middle one removed: the hole stays a hole while the two
      // survivors trade places around it.
      localStorage.setItem(
        CUSTOM_PRESETS_KEY,
        JSON.stringify([{ key: 'lastMonth' }, null, { key: 'lastYear' }, null]),
      );
      const c = await mount(makeFakeSettings({ firstRunDone: true }));

      await click(arrows(c, 'down')[0]);

      expect(stored()).toEqual([{ key: 'lastYear' }, null, { key: 'lastMonth' }, null]);
      expect(shortcuts(c)).toEqual(['Last year', 'Last month']);
    });

    it('offers no move past either end', async () => {
      const c = await mount(makeFakeSettings({ firstRunDone: true }));
      await click(addBtn(c)); // one preset: nowhere to go in either direction
      expect(arrows(c, 'up')[0].disabled).toBe(true);
      expect(arrows(c, 'down')[0].disabled).toBe(true);

      await click(type(c, 'Last year'));
      await click(addBtn(c));
      expect(arrows(c, 'up').map((b) => b.disabled)).toEqual([true, false]);
      expect(arrows(c, 'down').map((b) => b.disabled)).toEqual([false, true]);
    });

    it('reads configured presets back after a remount', async () => {
      const first = await mount(makeFakeSettings({ firstRunDone: true }));
      await setValue(dayInput(first)!, '45');
      await click(addBtn(first));
      await click(type(first, 'Last year'));
      await click(addBtn(first));

      for (const root of mountedRoots.splice(0)) act(() => root.unmount());
      const c = await mount(makeFakeSettings({ firstRunDone: true }));

      expect(shortcuts(c)).toEqual(['Last 45 days', 'Last year']);
    });

    it('reads malformed stored presets as none configured', async () => {
      localStorage.setItem(CUSTOM_PRESETS_KEY, '{"not":"an array"}');
      const c = await mount(makeFakeSettings({ firstRunDone: true }));
      expect(shortcuts(c)).toEqual([]);
      expect(c.textContent).toContain('None yet');
    });

    it('drops stored entries that are not usable presets', async () => {
      localStorage.setItem(
        CUSTOM_PRESETS_KEY,
        JSON.stringify([{ key: 'rolling', days: 9999 }, { key: 'nonsense' }, { key: 'lastYear' }]),
      );
      const c = await mount(makeFakeSettings({ firstRunDone: true }));
      expect(shortcuts(c)).toEqual(['Last year']);
    });
  });

  it('turns auto-refresh off, and says what off does not stop', async () => {
    const c = await mount(makeFakeSettings({ firstRunDone: true }));
    const seg = (label: string) => refreshSeg(c).find((b) => b.textContent === label)!;

    await click(seg('Off'));

    expect(localStorage.getItem(STORAGE_KEY)).toBe('0');
    expect(seg('Off').classList.contains('active')).toBe(true);
    // Off is not a duration, so the Custom field must not open on it
    expect(q<HTMLInputElement>(c, 'input[aria-label="Custom interval"]')).toBeNull();
    // "off" on a recording app has to say what it leaves running
    expect(c.textContent).toContain('Background recording carries on');

    await click(seg('60s'));
    expect(localStorage.getItem(STORAGE_KEY)).toBe('60');
    expect(seg('Off').classList.contains('active')).toBe(false);
    expect(c.textContent).not.toContain('Background recording carries on');
  });

  it('reads a stored Off back as Off rather than as a custom interval', async () => {
    localStorage.setItem(STORAGE_KEY, '0');
    const c = await mount(makeFakeSettings({ firstRunDone: true }));
    const seg = (label: string) => refreshSeg(c).find((b) => b.textContent === label)!;

    expect(seg('Off').classList.contains('active')).toBe(true);
    expect(seg('Custom').classList.contains('active')).toBe(false);
  });

  // The Menu Bar Extra's cadence is persisted through the port, not in
  // localStorage: the Rust loop that honours it runs with no webview alive.
  it('persists the menu bar refresh interval through the port', async () => {
    const port = makeFakeSettings({ firstRunDone: true });
    const c = await mount(port);
    const seg = (label: string) => menuBarSeg(c).find((b) => b.textContent === label)!;

    expect(menuBarSeg(c).map((b) => b.textContent)).toEqual(['Off', '1m', '5m', '15m']);
    expect(seg('1m').classList.contains('active')).toBe(true);

    await click(seg('15m'));
    expect(port.value.menuBarRefreshSec).toBe(900);
    expect(seg('15m').classList.contains('active')).toBe(true);
    expect(seg('1m').classList.contains('active')).toBe(false);
  });

  it('turns the menu bar refresh off, and says what off does not stop', async () => {
    const port = makeFakeSettings({ firstRunDone: true });
    const c = await mount(port);
    const seg = (label: string) => menuBarSeg(c).find((b) => b.textContent === label)!;

    await click(seg('Off'));

    expect(port.value.menuBarRefreshSec).toBe(0);
    expect(seg('Off').classList.contains('active')).toBe(true);
    // Off paces the bar back to the resident floor. On an app whose job is
    // recording, the one thing it must not be read as is "stop recording".
    expect(c.textContent).toContain('Recording never stops');

    await click(seg('5m'));
    expect(port.value.menuBarRefreshSec).toBe(300);
    expect(c.textContent).not.toContain('Recording never stops');
  });

  // The two intervals are separate settings on separate surfaces; one must not
  // move the other.
  it('keeps the two refresh intervals independent', async () => {
    const port = makeFakeSettings({ firstRunDone: true });
    const c = await mount(port);

    // Deliberately never equal, and neither ever 0 while the other is being
    // moved: shared values would let a cross-write pass unnoticed.
    await click(refreshSeg(c).find((b) => b.textContent === '10s')!);
    expect(localStorage.getItem(STORAGE_KEY)).toBe('10');
    expect(port.value.menuBarRefreshSec).toBe(60);

    await click(menuBarSeg(c).find((b) => b.textContent === '15m')!);
    expect(port.value.menuBarRefreshSec).toBe(900);
    expect(localStorage.getItem(STORAGE_KEY)).toBe('10');

    await click(refreshSeg(c).find((b) => b.textContent === 'Off')!);
    expect(localStorage.getItem(STORAGE_KEY)).toBe('0');
    expect(port.value.menuBarRefreshSec).toBe(900);
  });

  it('keeps an invalid rate editable without persisting it', async () => {
    const port = makeFakeSettings({ firstRunDone: true, currency: 'HKD', usdRate: 7.8 });
    const c = await mount(port);
    const input = q<HTMLInputElement>(c, RATE_INPUT)!;

    for (const bad of ['abc', '-3', '', '0']) {
      await setValue(input, bad);
      expect(input.value).toBe(bad); // stays editable
      expect(port.value.usdRate).toBe(7.8); // never persisted
    }
  });

  it('launch-at-login toggle persists and calls the enrollment wrapper', async () => {
    const port = makeFakeSettings({ firstRunDone: true, launchAtLogin: true });
    const c = await mount(port);

    await click(c.querySelector('[aria-label="Launch at login"]')!);

    expect(port.value.launchAtLogin).toBe(false);
    expect(setLaunchAtLogin).toHaveBeenCalledWith(false);
  });

  it('auto-check toggle persists', async () => {
    const port = makeFakeSettings({ firstRunDone: true, autoCheckUpdates: true });
    const c = await mount(port);

    await click(c.querySelector('[aria-label="Check for updates automatically"]')!);

    expect(port.value.autoCheckUpdates).toBe(false);
  });

  it('renders the honest caption for the not-configured update state', async () => {
    const port = makeFakeSettings({ firstRunDone: true }); // fake defaults to not-configured
    const c = await mount(port);

    expect(port.calls.checkUpdates).toBeGreaterThan(0);
    expect(c.textContent).toContain('Update checks arrive with signed releases');
    expect(c.textContent).not.toContain('up to date');
    expect(c.querySelector('.set-banner')).toBeNull();
  });

  it('renders the update banner for a downloaded release', async () => {
    const downloaded: UpdateStatus = { state: 'downloaded', version: '1.5.0' };
    const port = makeFakeSettings({ firstRunDone: true }, downloaded);
    const c = await mount(port);

    expect(c.querySelector('.set-banner')).not.toBeNull();
    const text = c.textContent ?? '';
    expect(text).toContain('TokenLedger 1.5.0');
    expect(text).toContain('is ready');
    expect(text).toContain('Restart to update');
    expect(text).toContain('1.5.0 downloaded · restart to install');
  });

  it('shows the first-run dialog when firstRunDone is false, and OK persists the choice once', async () => {
    const port = makeFakeSettings({ firstRunDone: false, launchAtLogin: true });
    const c = await mount(port);

    const dialog = q<HTMLElement>(c, '[role="dialog"]');
    expect(dialog).not.toBeNull();
    expect(dialog!.getAttribute('aria-modal')).toBe('true');

    // Turn the disclosed launch-at-login default OFF, then confirm.
    await click(dialog!.querySelector('.set-toggle')!);
    await click(dialog!.querySelector('.set-firstrun-ok')!);

    expect(port.value.firstRunDone).toBe(true);
    expect(port.value.launchAtLogin).toBe(false);
    expect(setLaunchAtLogin).toHaveBeenCalledWith(false);

    // Never reappears.
    await settle();
    expect(c.querySelector('[role="dialog"]')).toBeNull();
  });
});
