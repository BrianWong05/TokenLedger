/** @vitest-environment jsdom */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import TokenTotalHeadline from './TokenTotalHeadline';
import { I18nProvider, type Lang } from '../lib/i18n';
import './overview.css';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

const mountedRoots: Root[] = [];
const ENTRANCE_PLAYED_KEY = 'tokenledger.tokenTotalEntrancePlayed';
const RANGE_WINDOWS = [
  'day:2026-08-14:2026-08-14',
  'week:2026-08-08:2026-08-14',
  'month:2026-07-16:2026-08-14',
  'total:2026-07-16:2026-08-14',
  'custom:2026-08-10:2026-08-12',
] as const;
const RANGE_SWITCHES = RANGE_WINDOWS.flatMap((from) =>
  RANGE_WINDOWS.filter((to) => to !== from).map((to) => [from, to] as const),
);

function mountHeadline(total: number, authoritative: boolean, initialWindowKey = 'day::') {
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  mountedRoots.push(root);
  // windowKey defaults to one fixed window, so a rerender that only moves the
  // total reads as a background scan; pass a new key to mean a period switch.
  // visible false is another tab showing over a still-mounted Overview.
  const rerender = (
    nextTotal: number,
    nextAuthoritative: boolean,
    windowKey = 'day::',
    visible = true,
  ) => {
    act(() =>
      root.render(
        <TokenTotalHeadline
          total={nextTotal}
          fresh={nextTotal}
          authoritative={nextAuthoritative}
          windowKey={windowKey}
          visible={visible}
        />,
      ),
    );
  };
  rerender(total, authoritative, initialWindowKey);
  return { button: container.querySelector('button')!, rerender };
}

function renderHeadline(total: number): HTMLButtonElement {
  sessionStorage.setItem(ENTRANCE_PLAYED_KEY, 'true');
  return mountHeadline(total, true).button;
}

function setMediaPreferences({
  reducedMotion = false,
  compactLayout = false,
}: {
  reducedMotion?: boolean;
  compactLayout?: boolean;
}) {
  Object.defineProperty(window, 'matchMedia', {
    configurable: true,
    value: vi.fn().mockImplementation((query: string) => ({
      matches:
        query === '(prefers-reduced-motion: reduce)'
          ? reducedMotion
          : query === '(max-width: 639px)' && compactLayout,
      media: query,
      onchange: null,
      addListener: vi.fn(),
      removeListener: vi.fn(),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })),
  });
}

function setReducedMotion(matches: boolean) {
  setMediaPreferences({ reducedMotion: matches });
}

describe('TokenTotalHeadline', () => {
  beforeEach(() => {
    localStorage.clear();
    sessionStorage.clear();
    setReducedMotion(false);
  });

  afterEach(() => {
    vi.useRealTimers();
    for (const root of mountedRoots.splice(0)) {
      act(() => root.unmount());
    }
    document.body.replaceChildren();
  });

  it('rolls a mode change and settles on the exact total after 1.4 seconds', () => {
    vi.useFakeTimers();
    const button = renderHeadline(4_500_000_000);

    act(() => button.click());

    expect(button.getAttribute('aria-busy')).toBe('true');
    expect(button.getAttribute('aria-label')).toBe(
      '4,500,000,000 total tokens. Show compact token count',
    );

    act(() => vi.advanceTimersByTime(1_399));
    expect(button.getAttribute('aria-busy')).toBe('true');

    act(() => vi.advanceTimersByTime(1));
    expect(button.getAttribute('aria-busy')).toBeNull();
    expect(button.textContent).toBe('4,500,000,000');
  });

  it('rolls from a zero-shaped compact value when the first authoritative total arrives', () => {
    vi.useFakeTimers();
    const { button, rerender } = mountHeadline(4_500_000_000, false);

    expect(button.textContent).toBe('0.0B');

    rerender(4_500_000_000, true);

    expect(button.getAttribute('aria-busy')).toBe('true');
    act(() => vi.advanceTimersByTime(1_399));
    expect(button.getAttribute('aria-busy')).toBe('true');

    act(() => vi.advanceTimersByTime(1));
    expect(button.getAttribute('aria-busy')).toBeNull();
    expect(button.textContent).toBe('4.5B');
  });

  it('uses the saved exact shape for the first authoritative total', () => {
    vi.useFakeTimers();
    localStorage.setItem('tokenledger.tokenTotalDisplayMode', 'exact');
    const { button, rerender } = mountHeadline(4_500_000_000, false);

    expect(button.textContent).toBe('0,000,000,000');

    rerender(4_500_000_000, true);
    expect(button.getAttribute('aria-busy')).toBe('true');

    act(() => vi.advanceTimersByTime(1_400));
    expect(button.textContent).toBe('4,500,000,000');
  });

  it('waits past an authoritative zero for the first later nonzero total', () => {
    vi.useFakeTimers();
    const { button, rerender } = mountHeadline(0, true);

    expect(button.textContent).toBe('0');
    expect(button.getAttribute('aria-busy')).toBeNull();

    rerender(4_500_000_000, true);
    expect(button.getAttribute('aria-busy')).toBe('true');

    act(() => vi.advanceTimersByTime(1_400));
    expect(button.textContent).toBe('4.5B');
  });

  it('rolls a period switch in place instead of replaying the zero-shaped entrance', () => {
    vi.useFakeTimers();
    const { button, rerender } = mountHeadline(4_500_000_000, true);
    act(() => vi.advanceTimersByTime(1_400));

    // The new window's key arrives while its Summary is still pending.
    rerender(4_500_000_000, true, 'week::');
    expect(button.getAttribute('aria-busy')).toBe('true');
    act(() => vi.advanceTimersByTime(1_400));

    // A changed figure from the same window still updates without a second
    // roll, because background scans are not range switches.
    rerender(5_000_000_000, true, 'week::');
    expect(button.getAttribute('aria-busy')).toBeNull();
    expect(button.textContent).toBe('5B');

    // Another mount (the entrance already played this session) starts at rest.
    const revisited = mountHeadline(6_000_000_000, true).button;
    expect(revisited.getAttribute('aria-busy')).toBeNull();
    expect(revisited.textContent).toBe('6B');
  });

  it.each(RANGE_SWITCHES)('rolls before the Summary lands for %s → %s', (from, to) => {
    vi.useFakeTimers();
    sessionStorage.setItem(ENTRANCE_PLAYED_KEY, 'true');
    const { button, rerender } = mountHeadline(4_500_000_000, true, from);
    act(() => vi.advanceTimersByTime(1_400));

    rerender(4_500_000_000, true, to);

    expect(button.getAttribute('aria-busy')).toBe('true');
  });

  it('rolls the figure already on screen when the Overview tab comes back', () => {
    vi.useFakeTimers();
    const { button, rerender } = mountHeadline(4_500_000_000, true);
    act(() => vi.advanceTimersByTime(1_400));

    // Another tab shows: the Overview stays mounted, so nothing rolls yet.
    rerender(4_500_000_000, true, 'day::', false);
    expect(button.getAttribute('aria-busy')).toBeNull();

    // Back to the Overview. The total has not moved; the return is the occasion.
    rerender(4_500_000_000, true, 'day::', true);
    expect(button.getAttribute('aria-busy')).toBe('true');
    act(() => vi.advanceTimersByTime(1_400));
    expect(button.getAttribute('aria-busy')).toBeNull();
    expect(button.textContent).toBe('4.5B');
  });

  it('leaves a still-owed entrance to play on the first Overview return', () => {
    vi.useFakeTimers();
    // authoritative false: the entrance has not played, so it still owes its own
    // zero-shaped motion and the return must not pre-empt it.
    const { button, rerender } = mountHeadline(4_500_000_000, false);
    rerender(4_500_000_000, false, 'day::', false);
    rerender(4_500_000_000, false, 'day::', true);
    expect(button.getAttribute('aria-busy')).toBeNull();
    expect(button.textContent).toBe('0.0B');

    // The authoritative total then arrives and the entrance plays as always.
    rerender(4_500_000_000, true);
    expect(button.getAttribute('aria-busy')).toBe('true');
    act(() => vi.advanceTimersByTime(1_400));
    expect(button.textContent).toBe('4.5B');
  });

  it('holds still returning to a zero-usage Overview', () => {
    vi.useFakeTimers();
    const { button, rerender } = mountHeadline(0, true);
    rerender(0, true, 'day::', false);
    rerender(0, true, 'day::', true);

    expect(button.getAttribute('aria-busy')).toBeNull();
    expect(button.textContent).toBe('0');
  });

  it('swaps an Overview return silently under Reduce Motion', () => {
    vi.useFakeTimers();
    setReducedMotion(true);
    const { button, rerender } = mountHeadline(4_500_000_000, true);
    rerender(4_500_000_000, true, 'day::', false);
    rerender(4_500_000_000, true, 'day::', true);

    expect(button.getAttribute('aria-busy')).toBeNull();
    expect(button.textContent).toBe('4.5B');
  });

  it('holds still when a background scan changes the same window figure', () => {
    vi.useFakeTimers();
    const { button, rerender } = mountHeadline(4_500_000_000, true);
    act(() => vi.advanceTimersByTime(1_400));

    // Same windowKey: a 30s scan tick ingesting usage. Routine updates must not
    // animate (#12 story 9) — the figure just updates.
    rerender(5_000_000_000, true);
    expect(button.getAttribute('aria-busy')).toBeNull();
    expect(button.textContent).toBe('5B');
  });

  it('leaves the headline clickable while a period-switch roll runs', () => {
    vi.useFakeTimers();
    const { button, rerender } = mountHeadline(4_500_000_000, true);
    act(() => vi.advanceTimersByTime(1_400));
    rerender(5_000_000_000, true, 'week::');
    expect(button.getAttribute('aria-busy')).toBe('true');

    // Only a click's own reel swallows clicks; this one must reach the toggle.
    act(() => button.click());
    act(() => vi.advanceTimersByTime(1_400));
    expect(button.textContent).toBe('5,000,000,000');
  });

  it('swaps a period switch silently under Reduce Motion', () => {
    vi.useFakeTimers();
    setReducedMotion(true);
    const { button, rerender } = mountHeadline(4_500_000_000, true);
    act(() => vi.advanceTimersByTime(1_400));

    rerender(5_000_000_000, true, 'week::');
    expect(button.getAttribute('aria-busy')).toBeNull();
    expect(button.textContent).toBe('5B');
  });

  it('swaps a period switch in without rolling in a compact layout', () => {
    vi.useFakeTimers();
    setMediaPreferences({ compactLayout: true });
    const { button, rerender } = mountHeadline(4_500_000_000, true);
    act(() => vi.advanceTimersByTime(1_400));

    rerender(5_000_000_000, true, 'week::');
    expect(button.getAttribute('aria-busy')).toBeNull();
    expect(button.textContent).toBe('5B');
  });

  it('reveals the first authoritative total immediately with Reduce Motion', () => {
    setReducedMotion(true);
    const { button, rerender } = mountHeadline(4_500_000_000, false);

    rerender(4_500_000_000, true);

    expect(button.getAttribute('aria-busy')).toBeNull();
    expect(button.textContent).toBe('4.5B');
  });

  it('still rolls the first authoritative total in a compact layout', () => {
    vi.useFakeTimers();
    setMediaPreferences({ compactLayout: true });
    const { button, rerender } = mountHeadline(4_500_000_000, false);

    rerender(4_500_000_000, true);

    expect(button.getAttribute('aria-busy')).toBe('true');
    act(() => vi.advanceTimersByTime(1_400));
    expect(button.textContent).toBe('4.5B');
  });

  it('swaps a zero-usage window in without rolling', () => {
    vi.useFakeTimers();
    const { button, rerender } = mountHeadline(4_500_000_000, true);
    act(() => vi.advanceTimersByTime(1_400));

    // A window with no usage reads 0 immediately: rolling to it would imply
    // activity settling into place where there is none.
    rerender(0, true, 'week::');
    expect(button.getAttribute('aria-busy')).toBeNull();
    expect(button.textContent).toBe('0');
  });

  it('swaps modes immediately in a compact layout', () => {
    vi.useFakeTimers();
    setMediaPreferences({ compactLayout: true });
    const button = renderHeadline(4_500_000_000);

    act(() => button.click());

    expect(button.getAttribute('aria-busy')).toBeNull();
    expect(button.textContent).toBe('4,500,000,000');
  });

  it('swaps modes immediately when Reduce Motion is enabled', () => {
    setReducedMotion(true);
    const button = renderHeadline(4_500_000_000);

    act(() => button.click());

    expect(button.getAttribute('aria-busy')).toBeNull();
    expect(button.textContent).toBe('4,500,000,000');
    expect(button.title).toBe('Show compact token count');
    expect(renderHeadline(4_500_000_000).textContent).toBe('4,500,000,000');
  });

  it('rolls exact mode back into compact mode and settles after 1.4 seconds', () => {
    vi.useFakeTimers();
    localStorage.setItem('tokenledger.tokenTotalDisplayMode', 'exact');
    const button = renderHeadline(4_500_000_000);

    expect(button.textContent).toBe('4,500,000,000');

    act(() => button.click());

    expect(button.getAttribute('aria-busy')).toBe('true');
    expect(button.getAttribute('aria-label')).toBe(
      '4,500,000,000 total tokens. Show exact token count',
    );
    act(() => vi.advanceTimersByTime(1_399));
    expect(button.getAttribute('aria-busy')).toBe('true');

    act(() => vi.advanceTimersByTime(1));
    expect(button.getAttribute('aria-busy')).toBeNull();
    expect(button.textContent).toBe('4.5B');
  });

  it('ignores another activation until the current mode change settles', () => {
    vi.useFakeTimers();
    const button = renderHeadline(4_500_000_000);

    act(() => button.click());
    act(() => button.click());

    expect(button.getAttribute('aria-busy')).toBe('true');
    expect(button.title).toBe('Show compact token count');
    expect(localStorage.getItem('tokenledger.tokenTotalDisplayMode')).toBe('exact');

    act(() => vi.advanceTimersByTime(1_400));
    expect(button.textContent).toBe('4,500,000,000');
  });

  it('rebuilds immediately into only the target token structure', () => {
    vi.useFakeTimers();
    localStorage.setItem('tokenledger.tokenTotalDisplayMode', 'exact');
    const button = renderHeadline(5_841_112_112);

    act(() => button.click());

    const counter = button.querySelector<HTMLElement>('[data-counter-root="true"]')!;
    const tokens = Array.from(
      counter.querySelectorAll<HTMLElement>('[data-counter-token]'),
      (token) => token.dataset.counterToken,
    );
    expect(tokens).toEqual(['digit', 'static', 'digit', 'digit', 'digit', 'static']);
    expect(
      Array.from(
        counter.querySelectorAll<HTMLElement>('[data-counter-token="static"]'),
        (token) => token.textContent,
      ),
    ).toEqual(['.', 'B']);
    expect(counter.textContent).not.toContain(',');
  });

  it('uses digit prefixes for deterministic non-uniform travel', () => {
    vi.useFakeTimers();
    localStorage.setItem('tokenledger.tokenTotalDisplayMode', 'exact');
    const button = renderHeadline(5_841_112_112);

    act(() => button.click());

    const targets = Array.from(
      button.querySelectorAll<HTMLElement>('[data-counter-token="digit"]'),
      (digit) => Number(digit.dataset.counterTarget),
    );
    const glyphs = Array.from(
      button.querySelectorAll<HTMLElement>('[data-counter-token="digit"]'),
      (digit) => Number(digit.dataset.counterGlyph),
    );
    expect(targets).toEqual([5, 58, 584, 5841]);
    expect(glyphs).toEqual([5, 8, 4, 1]);
  });

  it('anchors target punctuation and units while the digits roll', () => {
    vi.useFakeTimers();
    localStorage.setItem('tokenledger.tokenTotalDisplayMode', 'exact');
    const button = renderHeadline(5_841_112_112);

    act(() => button.click());

    const anchoredSymbols = Array.from(
      button.querySelectorAll<HTMLElement>('[data-counter-token="static"]'),
      (symbol) => symbol.textContent,
    );
    expect(anchoredSymbols).toEqual(expect.arrayContaining(['.', 'B']));
  });

  it('classifies punctuation in the animated row', () => {
    vi.useFakeTimers();
    const button = renderHeadline(5_841_112_112);

    act(() => button.click());

    const comma = button.querySelector<HTMLElement>('[data-counter-token="static"]')!;
    expect(comma.textContent).toBe(',');
    expect(comma.classList.contains('is-comma')).toBe(true);
  });

  it.each([
    { initialMode: 'compact', expectedWidth: 'calc(13ch - 0.39em)' },
    { initialMode: 'exact', expectedWidth: 'calc(6ch - 0.18em)' },
  ])(
    'reserves the settled target width while rolling from $initialMode mode',
    ({ initialMode, expectedWidth }) => {
      vi.useFakeTimers();
      if (initialMode === 'exact') {
        localStorage.setItem('tokenledger.tokenTotalDisplayMode', 'exact');
      }
      const button = renderHeadline(5_841_112_112);

      act(() => button.click());

      const row = button.querySelector<HTMLElement>('.tt-token-counter-row')!;
      expect(row.style.width).toBe(expectedWidth);
    },
  );

  it('uses the settled target font size throughout a mode change', () => {
    vi.useFakeTimers();
    localStorage.setItem('tokenledger.tokenTotalDisplayMode', 'exact');
    const button = renderHeadline(5_841_112_112);

    act(() => button.click());
    expect(button.getAttribute('style')).toContain('25.833cqi');

    act(() => vi.advanceTimersByTime(1_400));
    expect(button.getAttribute('style')).toContain('25.833cqi');
  });

  it('forms the target token row before the digit springs settle', () => {
    vi.useFakeTimers();
    const button = renderHeadline(5_841_112_112);

    act(() => button.click());

    const counter = button.querySelector<HTMLElement>('[data-counter-root="true"]')!;
    expect(counter.querySelectorAll('[data-counter-token]').length).toBe(13);
    expect(counter.querySelectorAll('[data-counter-token="digit"]').length).toBe(10);
    expect(
      Array.from(
        counter.querySelectorAll<HTMLElement>('[data-counter-token="static"]'),
        (token) => token.textContent,
      ),
    ).toEqual([',', ',', ',']);
  });

  it('uses ten-glyph columns inside a fixed counter window', () => {
    vi.useFakeTimers();
    const button = renderHeadline(5_841_112_112);

    act(() => button.click());

    const digitColumns = Array.from(
      button.querySelectorAll<HTMLElement>('[data-counter-token="digit"]'),
    );
    expect(digitColumns.length).toBeGreaterThan(1);
    expect(digitColumns.every((digit) => digit.childElementCount === 10)).toBe(true);
    expect(button.style.getPropertyValue('--tt-counter-height')).toBe('1.0833em');
  });

  it('keeps exact-to-compact motion inside one target-only row for 1.4 seconds', () => {
    vi.useFakeTimers();
    localStorage.setItem('tokenledger.tokenTotalDisplayMode', 'exact');
    const button = renderHeadline(5_841_112_112);

    act(() => button.click());

    const counter = button.firstElementChild as HTMLElement;
    expect(counter.dataset.counterRoot).toBe('true');
    expect(counter.childElementCount).toBe(1);
    expect(counter.querySelectorAll('[data-counter-token]').length).toBe(6);
    expect(counter.textContent).not.toContain(',');

    act(() => vi.advanceTimersByTime(1_399));
    expect(button.getAttribute('aria-busy')).toBe('true');

    act(() => vi.advanceTimersByTime(1));
    expect(button.getAttribute('aria-busy')).toBeNull();
    expect(button.textContent).toBe('5.841B');
  });

  it('lets the user switch between compact and exact totals and remembers the choice', () => {
    vi.useFakeTimers();
    const button = renderHeadline(4_500_000_000);

    expect(button.textContent).toBe('4.5B');
    expect(button.type).toBe('button');
    expect(button.title).toBe('Show exact token count');
    expect(button.getAttribute('aria-label')).toBe(
      '4,500,000,000 total tokens. Show exact token count',
    );

    act(() => button.click());

    expect(button.getAttribute('aria-busy')).toBe('true');
    expect(button.title).toBe('Show compact token count');
    expect(button.getAttribute('aria-label')).toBe(
      '4,500,000,000 total tokens. Show compact token count',
    );
    act(() => vi.advanceTimersByTime(1_400));
    expect(button.textContent).toBe('4,500,000,000');

    const restored = renderHeadline(4_500_000_000);
    expect(restored.textContent).toBe('4,500,000,000');
  });

  it.each([
    [950, '950'],
    [1_500, '1.5K'],
    [1_140_000, '1.14M'],
    [114_000_000, '114M'],
    [114_200_000, '114.2M'],
    [4_560_000_000, '4.56B'],
    // At three decimals this reads precisely, so it no longer needs rolling up
    // to '1B' — but the guard still fires once a value would print a full 1000.
    [999_995_000, '999.995M'],
    [999_999_600, '1B'],
  ])('renders %i tokens compactly as %s', (total, expected) => {
    expect(renderHeadline(total).textContent).toBe(expected);
  });

  it('keeps a long exact total on one line within the available width', () => {
    vi.useFakeTimers();
    const button = renderHeadline(4_500_123_456);
    act(() => button.click());
    act(() => vi.advanceTimersByTime(1_400));

    expect(button.textContent).toBe('4,500,123,456');
    const style = getComputedStyle(button);
    expect(style.whiteSpace).toBe('nowrap');
    expect(style.maxWidth).toBe('100%');
  });

  it('remembers a mode change when compact and exact text are identical', () => {
    const button = renderHeadline(950);
    act(() => button.click());

    expect(button.textContent).toBe('950');
    expect(button.getAttribute('aria-busy')).toBeNull();
    expect(button.title).toBe('Show compact token count');
    expect(renderHeadline(1_500).textContent).toBe('1,500');
  });

  it('occupies its own row so estimated Cost renders underneath', () => {
    const button = renderHeadline(4_500_000_000);

    expect(getComputedStyle(button).display).toBe('block');
  });
});

// The Fresh Tokens sub-line: Input + Output + Cache Write, quoted under the
// total in the same shape the headline is wearing.
describe('TokenTotalHeadline fresh sub-line', () => {
  beforeEach(() => {
    localStorage.clear();
    sessionStorage.clear();
    setReducedMotion(false);
    sessionStorage.setItem(ENTRANCE_PLAYED_KEY, 'true');
  });

  afterEach(() => {
    for (const root of mountedRoots.splice(0)) {
      act(() => root.unmount());
    }
    document.body.replaceChildren();
  });

  function renderFresh(
    fresh: number,
    { lang = 'en', incomplete = null }: { lang?: Lang; incomplete?: string | null } = {},
  ): string {
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    mountedRoots.push(root);
    act(() =>
      root.render(
        <I18nProvider lang={lang}>
          <TokenTotalHeadline
            total={14_287_948}
            fresh={fresh}
            authoritative
            windowKey="day::"
            incomplete={incomplete}
          />
        </I18nProvider>,
      ),
    );
    return container.textContent ?? '';
  }

  it('quotes the window fresh figure under the total', () => {
    expect(renderFresh(399_590)).toContain('399.59K fresh');
  });

  it('reads an empty window as zero instead of dropping the line', () => {
    expect(renderFresh(0)).toContain('0 fresh');
  });

  it('follows the headline into exact mode', () => {
    localStorage.setItem('tokenledger.tokenTotalDisplayMode', 'exact');
    expect(renderFresh(399_590)).toContain('399,590 fresh');
  });

  it('leads with the label under zh-Hant', () => {
    expect(renderFresh(399_590, { lang: 'zh-Hant' })).toContain('新工作 399.59K');
  });

  it('marks fresh a floor when the total is one', () => {
    expect(renderFresh(399_590, { incomplete: 'Antigravity: 100 sessions unreadable' })).toContain(
      '≥ 399.59K fresh',
    );
  });
});
