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

// Set a controlled input/select value through the native setter so React's
// value tracking sees the change, then fire the events React listens for.
async function setValue(el: HTMLInputElement | HTMLSelectElement, value: string) {
  const proto = el instanceof HTMLSelectElement ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(proto, 'value')!.set!;
  await act(async () => {
    setter.call(el, value);
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
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
const refreshSeg = (c: HTMLElement) =>
  Array.from(c.querySelectorAll('.set-seg-mono button')) as HTMLButtonElement[];
const q = <T extends Element>(c: ParentNode, s: string) => c.querySelector(s) as T | null;

describe('SettingsPage', () => {
  it('renders all five items from the fake settings', async () => {
    const port = makeFakeSettings({ firstRunDone: true, currency: 'HKD', usdRate: 7.8 });
    const c = await mount(port);
    const text = c.textContent ?? '';

    // Appearance: theme segment + language select.
    expect(seg(c).map((b) => b.textContent)).toEqual(['System', 'Light', 'Dark']);
    expect(seg(c).find((b) => b.classList.contains('active'))?.textContent).toBe('System');
    expect(q<HTMLSelectElement>(c, 'select[aria-label="Language"]')?.value).toBe('en');

    // Display currency + exchange rate (non-USD shows the rate row).
    expect(q<HTMLSelectElement>(c, 'select[aria-label="Currency"]')?.value).toBe('HKD');
    expect(q<HTMLInputElement>(c, '.set-rate-input')?.value).toBe('7.8');

    // Startup + updates.
    expect(c.querySelector('[aria-label="Launch at login"]')).not.toBeNull();
    expect(text).toContain('Version 1.4.2');
    expect(text).toContain('Check for updates');
    expect(text).toContain('Nothing leaves this Mac.');
  });

  it('renders the Scanning group and persists the auto-refresh interval', async () => {
    localStorage.removeItem(STORAGE_KEY);
    const port = makeFakeSettings({ firstRunDone: true });
    const c = await mount(port);

    // Four presets + Custom; 30s active by default (parseRefreshSec fallback).
    expect(refreshSeg(c).map((b) => b.textContent)).toEqual(['10s', '30s', '60s', '5m', 'Custom']);
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
    await setValue(q<HTMLSelectElement>(c, 'select[aria-label="Language"]')!, 'zh-Hant');

    expect(c.textContent).toContain('外觀'); // Appearance
    expect(c.textContent).toContain('主題'); // Theme
    expect(c.textContent).not.toContain('Appearance');
    expect(port.value.language).toBe('zh-Hant');
  });

  it('hides the rate row for USD, shows it for another currency and persists a valid rate', async () => {
    const port = makeFakeSettings({ firstRunDone: true, currency: 'USD' });
    const c = await mount(port);

    expect(q<HTMLInputElement>(c, '.set-rate-input')).toBeNull();

    await setValue(q<HTMLSelectElement>(c, 'select[aria-label="Currency"]')!, 'HKD');
    expect(q<HTMLInputElement>(c, '.set-rate-input')).not.toBeNull();

    await setValue(q<HTMLInputElement>(c, '.set-rate-input')!, '7.85');
    expect(port.value.currency).toBe('HKD');
    expect(port.value.usdRate).toBe(7.85);
  });

  it('keeps an invalid rate editable without persisting it', async () => {
    const port = makeFakeSettings({ firstRunDone: true, currency: 'HKD', usdRate: 7.8 });
    const c = await mount(port);
    const input = q<HTMLInputElement>(c, '.set-rate-input')!;

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
