// Overview localisation glue: language + Display Currency, kept self-contained
// so the Overview wave never edits the shared i18n barrel. A local translator is
// built from the overview string module via the exported makeTranslator, keyed
// off the current language in the I18n context (so I18nProvider still drives it);
// dates/months come from Intl in that language, and every displayed Cost routes
// through the Display-Currency formatCost (USD stays byte-identical to before).
import { makeTranslator, useT, type Lang } from '../lib/i18n';
import { overview } from '../lib/strings/overview';
import { parseLocalDate } from '../lib/dateRange';
import { calendarSpan } from './data';
import { formatCost as formatUsdCost } from '../lib/format';
import { formatCost as formatCurrency } from '../lib/currency';
import {
  isAllUnattributedCost,
  isPartialCost,
  type CostCompleteness,
} from '../lib/costCompleteness';
import { unreadableSourcesIn, type UnreadableCounts } from '../lib/tokenCompleteness';
import { sourceMeta, type Range8b } from './meta';
import type { PresetKey, PresetSlot, RangePreset } from './data';
import type { Settings } from '../types';

export type OverviewKey = keyof typeof overview.en;

const translate = makeTranslator(overview.en, overview['zh-Hant']);

// Pure translator for the data/store layer and tests (defaults to English).
export function overviewT(lang: Lang, key: OverviewKey): string {
  return translate(lang, key);
}

// Hook for components: current language + a bound t(). Works without a provider
// (the I18n context defaults to 'en'), so a bare-rendered component still reads.
export function useOverviewT(): { lang: Lang; t: (key: OverviewKey) => string } {
  const { lang } = useT();
  return { lang, t: (key) => translate(lang, key) };
}

const localeOf = (lang: Lang) => (lang === 'zh-Hant' ? 'zh-Hant-HK' : 'en-US');

// "Jul 16" (en) / locale-correct short form — replaces lib/format's en-US-only
// fmtDate/fmtIsoDate for the language-aware overview surfaces.
export function fmtDateL(d: Date, lang: Lang): string {
  return d.toLocaleDateString(localeOf(lang), { month: 'short', day: 'numeric' });
}
export function fmtIsoDateL(iso: string, lang: Lang): string {
  return fmtDateL(parseLocalDate(iso), lang);
}
// Short month name for a 0-based month index (heatmap + monthly trend labels).
export function monthShortL(monthIndex: number, lang: Lang): string {
  return new Date(2020, monthIndex, 1).toLocaleDateString(localeOf(lang), { month: 'short' });
}
// Full month name — the picker's calendar heading, where the short form reads
// clipped next to the year beside it.
export function monthLongL(monthIndex: number, lang: Lang): string {
  return new Date(2020, monthIndex, 1).toLocaleDateString(localeOf(lang), { month: 'long' });
}
// "Wed, Jul 16" style — the heatmap day tooltip header.
export function fmtWeekdayDateL(d: Date, lang: Lang): string {
  return d.toLocaleDateString(localeOf(lang), { weekday: 'short', month: 'short', day: 'numeric' });
}
// Short weekday name for a 0-based day index (0 = Sun) — the 2D heatmap row rail.
export function weekdayShortL(dow: number, lang: Lang): string {
  // 2023-01-01 was a Sunday
  return new Date(2023, 0, 1 + dow).toLocaleDateString(localeOf(lang), { weekday: 'short' });
}
// Single-letter weekday for a 0-based day index (0 = Sun) — the picker's column
// headers, where a 26px column has room for one glyph.
export function weekdayNarrowL(dow: number, lang: Lang): string {
  return new Date(2023, 0, 1 + dow).toLocaleDateString(localeOf(lang), { weekday: 'narrow' });
}

// "N word": English pluralises via the one/many key; Chinese has no plural, so
// both keys carry the same measure phrase and the count leads unchanged.
export function countLabel(n: number, oneKey: OverviewKey, manyKey: OverviewKey, lang: Lang): string {
  return `${n} ${translate(lang, n === 1 ? oneKey : manyKey)}`;
}

// "14 days" — how long a picked window is. Shared by the picker's mid-pick hint
// and the Settings rows that caption a configured Preset.
export function spanLabelL(from: string, to: string, lang: Lang): string {
  return countLabel(calendarSpan(from, to), 'overview.dayOne', 'overview.daysUnit', lang);
}

// "Apr 1, 2026 – Jun 30, 2026" / "2026年4月1日 – 2026年6月30日". Both ends carry
// the year, which a pair of bare short dates cannot: "Jan 1 – Dec 31" hides
// that in Q1 the last quarter is October of the year before. Repeating it beats
// collapsing the shared parts, which each language does differently (Intl's own
// formatRange would, but it is absent from the WebView on older macOS).
export function fmtIsoRangeL(from: string, to: string, lang: Lang): string {
  const withYear = (iso: string) =>
    parseLocalDate(iso).toLocaleDateString(localeOf(lang), {
      year: 'numeric',
      month: 'short',
      day: 'numeric',
    });
  return `${withYear(from)} – ${withYear(to)}`;
}

// The reason half of tokenFloor below: names the Sources holding Unreadable
// Artifacts, e.g. "Antigravity: 100 sessions unreadable".
function unreadableReasons(
  sources: { source: string; artifactsUnreadable: number }[],
  lang: Lang,
): string {
  return sources
    .map((u) => `${sourceMeta(u.source).label}: ${countLabel(u.artifactsUnreadable, 'overview.unreadableSessionOne', 'overview.unreadableSessionMany', lang)}`)
    .join(' · ');
}

// The ≥ token floor as ONE shape for every surface — the mirror of Cost's
// isPartialCost + formatDisplayCost pairing. tokenCompleteness.ts owns which
// Sources count against a window, this the floor fact and its hover reason.
// (a null start means an unbounded window); this owns what the reader sees.
// markedTokenFigure renders any figure against it, in whatever precision the
// surface chose. One render site keeps the glyph in its own JSX: the animated
// headline (TokenTotalHeadline), whose counter animates in a separate element
// — it still reads this shape, never a private encoding.
export interface TokenFloor {
  marked: boolean;
  reason: string; // '' when unmarked; otherwise the ≥ marker's hover text
}

export const NO_FLOOR: TokenFloor = { marked: false, reason: '' };

// Unbooked Requests (TOKL-25) deliberately contribute NO reason and NO marker
// here: they are stated in the scan footer, never folded into the ≥. Only
// ADR-0017's Unreadable Artifacts mark a figure.
export function tokenFloor(
  sources: (UnreadableCounts & { source: string })[],
  windowStartSec: number | null,
  lang: Lang,
): TokenFloor {
  const unreadableIn = unreadableSourcesIn(sources, windowStartSec);
  return unreadableIn.length > 0
    ? { marked: true, reason: unreadableReasons(unreadableIn, lang) }
    : NO_FLOOR;
}

// Named for tokens because that is where the ≥ started (ADR-0017), but the
// marker belongs on any figure the same gap bounds — an unreadable Session
// hides the Requests inside it as well as the tokens.
export function markedTokenFigure(figure: string, floor: TokenFloor): string {
  return floor.marked ? `≥ ${figure}` : figure;
}

// Picker shortcut -> string key, same no-computed-key discipline as RANGE_LABEL_KEY.
export const PRESET_LABEL_KEY: Record<PresetKey, OverviewKey> = {
  yesterday: 'overview.preset.yesterday',
  thisMonth: 'overview.preset.thisMonth',
  last90: 'overview.preset.last90',
  thisYear: 'overview.preset.thisYear',
  lastMonth: 'overview.preset.lastMonth',
  lastQuarter: 'overview.preset.lastQuarter',
  lastYear: 'overview.preset.lastYear',
};

// A shipped shortcut is a static key; a configured rolling one cannot be, since
// its N is arbitrary — so that label is composed from a prefix and the day
// count, reading "Last 14 days" and "過去 14 天" beside the shipped windows. One
// builder for the picker's buttons and the Settings rows that configure them.
export function presetLabelL(p: RangePreset | PresetSlot, lang: Lang): string {
  return p.key === 'rolling'
    ? `${translate(lang, 'overview.preset.lastN')} ${countLabel(p.days, 'overview.dayOne', 'overview.daysUnit', lang)}`
    : translate(lang, PRESET_LABEL_KEY[p.key]);
}

// Range8b -> string keys, so both the segment (short) and the eyebrow (long)
// translate the same presets without a computed-key type hole.
export const RANGE_LABEL_KEY: Record<Range8b, OverviewKey> = {
  day: 'overview.range.day',
  week: 'overview.range.week',
  month: 'overview.range.month',
  total: 'overview.range.total',
  custom: 'overview.range.custom',
};
export const RANGE_LONG_KEY: Record<Range8b, OverviewKey> = {
  day: 'overview.range.day.long',
  week: 'overview.range.week.long',
  month: 'overview.range.month.long',
  total: 'overview.range.total.long',
  custom: 'overview.range.custom.long',
};

// Token-category key -> its label string key (the four canonical categories).
export const CAT_KEY: Record<string, OverviewKey> = {
  input: 'overview.cat.input',
  output: 'overview.cat.output',
  cacheRead: 'overview.cat.cacheRead',
  cacheWrite: 'overview.cat.cacheWrite',
};

// Granularity ('hour'|'day'|'week'|'month') -> the "avg / {unit}" word.
export const PER_UNIT_KEY: Record<string, OverviewKey> = {
  hour: 'overview.per.hour',
  day: 'overview.per.day',
  week: 'overview.per.week',
  month: 'overview.per.month',
};

// Granularity -> the trend enlarge inspector's "Selected {unit}" heading.
export const SEL_HEADING_KEY: Record<string, OverviewKey> = {
  hour: 'overview.trend.selHour',
  day: 'overview.trend.selDay',
  week: 'overview.trend.selWeek',
  month: 'overview.trend.selMonth',
};

// Bar-interval option ('auto' | explicit granularity) -> its adjective label.
export const INTERVAL_LABEL_KEY: Record<string, OverviewKey> = {
  auto: 'overview.trend.int.auto',
  day: 'overview.trend.int.day',
  week: 'overview.trend.int.week',
  month: 'overview.trend.int.month',
};

// Render a USD Cost in the Display Currency, preserving the ≥ (Partial Cost)
// prefix and null→unpriced label. USD delegates to lib/format so it is
// byte-identical to today (including adaptive precision); any other currency
// converts through lib/currency at 2dp and re-applies the markers.
export function formatDisplayCost(
  usd: number | null,
  partial: boolean,
  settings: Pick<Settings, 'currency' | 'usdRate'>,
  lang: Lang,
  opts: { adaptivePrecision?: boolean; unpricedLabel?: string } = {},
): string {
  if (settings.currency === 'USD') return formatUsdCost(usd, partial, opts);
  if (usd === null) return opts.unpricedLabel ?? 'unpriced';
  const amount = formatCurrency(usd, settings, lang);
  return partial ? `≥ ${amount}` : amount;
}

export function formatSummaryCost(
  summary: CostCompleteness,
  settings: Pick<Settings, 'currency' | 'usdRate'>,
  lang: Lang,
  opts: { adaptivePrecision?: boolean; unpricedLabel?: string } = {},
): string {
  if (isAllUnattributedCost(summary)) return overviewT(lang, 'overview.unavailableCost');
  return formatDisplayCost(summary.cost, isPartialCost(summary), settings, lang, opts);
}

// A USD identity, for callers (and tests) that render before real settings land.
export const USD_IDENTITY: Pick<Settings, 'currency' | 'usdRate'> = { currency: 'USD', usdRate: 1 };
