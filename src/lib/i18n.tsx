// Hand-rolled i18n: no dependency. Per-area string modules are merged into one
// English dictionary (the key universe) and a Traditional-Chinese overlay; a
// missing zh-Hant key falls back to its English value. A React context hands
// components `t(key)` keyed to the current language (which comes from Settings).
import { createContext, useContext, useMemo, type ReactNode } from 'react';
import { common } from './strings/common';
import { pricing } from './strings/pricing';
import { settings } from './strings/settings';

export type Lang = 'en' | 'zh-Hant';

const EN = { ...common.en, ...pricing.en, ...settings.en };
const ZH: Partial<Record<keyof typeof EN, string>> = {
  ...common['zh-Hant'],
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
