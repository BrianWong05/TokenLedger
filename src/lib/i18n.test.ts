import { describe, expect, it } from 'vitest';
import { makeTranslator } from './i18n';

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
