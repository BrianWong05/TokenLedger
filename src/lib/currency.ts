import type { Settings } from '../types';
import type { Lang } from './i18n';

// Render a USD Cost in the user's Display Currency. USD passes through
// unconverted (identity); any other currency multiplies by the user's fixed
// usdRate and formats with Intl in the language's locale. Stored figures never
// leave USD — this is a display-time multiplication only (ADR-0002).
export function formatCost(
  usd: number,
  settings: Pick<Settings, 'currency' | 'usdRate'>,
  language: Lang,
): string {
  const amount = settings.currency === 'USD' ? usd : usd * settings.usdRate;
  const locale = language === 'zh-Hant' ? 'zh-Hant-HK' : 'en-US';
  return new Intl.NumberFormat(locale, {
    style: 'currency',
    currency: settings.currency,
  }).format(amount);
}
