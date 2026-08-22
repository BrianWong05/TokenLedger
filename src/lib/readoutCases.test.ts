import { describe, it, expect } from 'vitest';
import cases from '../readout-cases.json';
import { formatCompactTokenTotal } from './format';
import { formatCost } from './currency';
import { unreadableSourcesIn, unbookedSourcesIn } from './tokenCompleteness';
import { isPartialCost } from './costCompleteness';
import { CURRENCIES } from '../settings/SettingsPage';
import type { Lang } from './i18n';

// One half of the shared read-out pin (src/readout-cases.json): this side
// proves the table says what Intl actually renders, the Rust side
// (src-tauri/src/readout.rs) proves the Menu Bar Extra's native rendering
// says the same — so the bar title and the panel cannot disagree about the
// same Summary without a build failing. The tie rows are load-bearing: Intl
// rounds the shortest decimal form half-up, which is exactly where a naive
// binary rounding drifts.

describe('readout cases — the panel rendering matches the shared table', () => {
  it.each(cases.tokens)('compact token total: $n → $text', ({ n, text }) => {
    expect(formatCompactTokenTotal(n)).toBe(text);
  });

  it.each(cases.costs)(
    'cost: $usd USD × $usdRate as $currency ($lang) → $text',
    ({ usd, usdRate, currency, lang, text }) => {
      expect(formatCost(usd, { currency, usdRate }, lang as Lang)).toBe(text);
    },
  );

  it.each(cases.floors)(
    'floor: $artifactsUnreadable unreadable, mtime $unreadableMaxMtime, start $windowStart → $floor',
    ({ artifactsUnreadable, unreadableMaxMtime, windowStart, floor }) => {
      const marked = unreadableSourcesIn([{ artifactsUnreadable, unreadableMaxMtime }], windowStart);
      expect(marked.length > 0).toBe(floor);
    },
  );

  // TOKL-25's cause, pinned the same way: an Unbooked Request has a real span,
  // so both window edges matter here where the unreadable rule has only a start.
  it.each(cases.unbookedFloors)(
    'unbooked floor: $requests requests in [$firstAt, $lastAt], window [$windowStart, $windowEnd] → $floor',
    ({ requests, firstAt, lastAt, windowStart, windowEnd, floor }) => {
      const marked = unbookedSourcesIn([{ requests, firstAt, lastAt }], windowStart, windowEnd);
      expect(marked.length > 0).toBe(floor);
    },
  );

  it.each(cases.partialCosts)(
    'partial cost: $cost, unpriced $hasUnpriced, unattributed $unattributedTokens → $partial',
    ({ cost, hasUnpriced, unattributedTokens, partial }) => {
      expect(isPartialCost({ cost, hasUnpriced, unattributedTokens })).toBe(partial);
    },
  );

  // Adding a currency to Settings must not be a silent two-language edit: a
  // code offered there without table rows would let the bar drift to the
  // generic fallback while the panel shows a real symbol, with no test red.
  it.each(['en', 'zh-Hant'])('every currency Settings offers has a %s row', (lang) => {
    for (const [code] of CURRENCIES) {
      expect(
        cases.costs.some((c) => c.currency === code && c.lang === lang),
        `${code} has no ${lang} row in readout-cases.json`,
      ).toBe(true);
    }
  });
});
