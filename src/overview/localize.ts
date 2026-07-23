// Overview localisation glue: language + Display Currency, kept self-contained
// so the Overview wave never edits the shared i18n barrel. A local translator is
// built from the overview string module via the exported makeTranslator, keyed
// off the current language in the I18n context (so I18nProvider still drives it);
// dates/months come from Intl in that language, and every displayed Cost routes
// through the Display-Currency formatCost (USD stays byte-identical to before).
import { makeTranslator, useT, type Lang } from '../lib/i18n';
import { overview } from '../lib/strings/overview';
import { parseLocalDate } from '../lib/dateRange';
import { formatCost as formatUsdCost } from '../lib/format';
import { formatCost as formatCurrency } from '../lib/currency';
import type { Range8b } from './meta';
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
// "Wed, Jul 16" style — the heatmap day tooltip header.
export function fmtWeekdayDateL(d: Date, lang: Lang): string {
  return d.toLocaleDateString(localeOf(lang), { weekday: 'short', month: 'short', day: 'numeric' });
}
// Short weekday name for a 0-based day index (0 = Sun) — the 2D heatmap row rail.
export function weekdayShortL(dow: number, lang: Lang): string {
  // 2023-01-01 was a Sunday
  return new Date(2023, 0, 1 + dow).toLocaleDateString(localeOf(lang), { weekday: 'short' });
}

// "N word": English pluralises via the one/many key; Chinese has no plural, so
// both keys carry the same measure phrase and the count leads unchanged.
export function countLabel(n: number, oneKey: OverviewKey, manyKey: OverviewKey, lang: Lang): string {
  return `${n} ${translate(lang, n === 1 ? oneKey : manyKey)}`;
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
  hasUnpriced: boolean,
  settings: Pick<Settings, 'currency' | 'usdRate'>,
  lang: Lang,
  opts: { adaptivePrecision?: boolean; unpricedLabel?: string } = {},
): string {
  if (settings.currency === 'USD') return formatUsdCost(usd, hasUnpriced, opts);
  if (usd === null) return opts.unpricedLabel ?? 'unpriced';
  const amount = formatCurrency(usd, settings, lang);
  return hasUnpriced ? `≥ ${amount}` : amount;
}

// A USD identity, for callers (and tests) that render before real settings land.
export const USD_IDENTITY: Pick<Settings, 'currency' | 'usdRate'> = { currency: 'USD', usdRate: 1 };
