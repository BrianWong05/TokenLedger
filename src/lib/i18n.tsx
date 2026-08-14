// Hand-rolled i18n: no dependency. Per-area string modules are merged into one
// English dictionary (the key universe) and a Traditional-Chinese overlay; a
// missing zh-Hant key falls back to its English value. A React context hands
// components `t(key)` keyed to the current language (which comes from Settings).
import { createContext, useContext, useMemo, type ReactNode } from 'react';
import { common } from './strings/common';
import { limits } from './strings/limits';
import { pricing } from './strings/pricing';
import { settings } from './strings/settings';

export type Lang = 'en' | 'zh-Hant';

const EN = { ...common.en, ...limits.en, ...pricing.en, ...settings.en };
const ZH: Partial<Record<keyof typeof EN, string>> = {
  ...common['zh-Hant'],
  ...limits['zh-Hant'],
  ...pricing['zh-Hant'],
  ...settings['zh-Hant'],
};

export type StringKey = keyof typeof EN;

// The lookup, factored out so the fallback is unit-testable against fixtures.
export function makeTranslator<K extends string>(
  en: Record<K, string>,
  zh: Partial<Record<K, string>>,
) {
  return (lang: Lang, key: K): string => (lang === 'zh-Hant' ? zh[key] : undefined) ?? en[key];
}

const translate = makeTranslator(EN, ZH);

/**
 * Which of a key pair's two forms a count takes — decided by the reader's own
 * grammar rather than by English's. Chinese inflects no plural, so every count
 * takes the `Many` form; English splits at one, and at one only (`0 windows`,
 * not `0 window`), which a `n === 1` test in a component happens to get right
 * for exactly these two languages and for no guaranteed reason.
 *
 * CLDR has four more categories — `zero`, `two`, `few`, `many` — that neither
 * language uses. They fold into `Many` because that is the pair this dictionary's
 * keys come in; a language that needs a third form needs a third key first.
 */
export function pluralSuffix(lang: Lang, n: number): 'One' | 'Many' {
  return new Intl.PluralRules(lang).select(n) === 'one' ? 'One' : 'Many';
}

interface I18n {
  lang: Lang;
  t: (key: StringKey) => string;
}

const Ctx = createContext<I18n>({ lang: 'en', t: (k) => EN[k] });

export function I18nProvider({ lang, children }: { lang: Lang; children: ReactNode }) {
  const value = useMemo<I18n>(() => ({ lang, t: (k) => translate(lang, k) }), [lang]);
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useT(): I18n {
  return useContext(Ctx);
}
