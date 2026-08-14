import { describe, expect, it } from 'vitest';
import { makeTranslator, pluralSuffix } from './i18n';

describe('makeTranslator', () => {
  // 'bye' is intentionally left untranslated to exercise the fallback.
  const t = makeTranslator(
    { greet: 'Hello', bye: 'Bye' },
    { greet: '你好' },
  );

  it('returns the zh-Hant value when present', () => {
    expect(t('zh-Hant', 'greet')).toBe('你好');
  });

  it('falls back to the English value when the zh-Hant key is missing', () => {
    expect(t('zh-Hant', 'bye')).toBe('Bye');
  });

  it('returns English directly for the en language', () => {
    expect(t('en', 'greet')).toBe('Hello');
  });
});

describe('pluralSuffix', () => {
  it('splits English at one, and at one only', () => {
    expect(pluralSuffix('en', 1)).toBe('One');
    // Zero takes the plural in English — "0 windows" — which is the case a
    // `n === 1` test in a component gets right by luck rather than by rule.
    expect(pluralSuffix('en', 0)).toBe('Many');
    expect(pluralSuffix('en', 2)).toBe('Many');
    expect(pluralSuffix('en', 11)).toBe('Many');
  });

  it('never splits Chinese, which has no plural to inflect', () => {
    for (const n of [0, 1, 2, 11]) expect(pluralSuffix('zh-Hant', n), `n=${n}`).toBe('Many');
  });
});
