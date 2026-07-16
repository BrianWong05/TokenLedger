import { describe, expect, it } from 'vitest';
import { formatCost } from './currency';

describe('formatCost (display currency)', () => {
  it('passes USD through unconverted — the rate is ignored', () => {
    expect(formatCost(10, { currency: 'USD', usdRate: 1 }, 'en')).toBe('$10.00');
    expect(formatCost(10, { currency: 'USD', usdRate: 7.8 }, 'en')).toBe('$10.00');
  });

  it('converts to the display currency at the fixed rate', () => {
    expect(formatCost(10, { currency: 'HKD', usdRate: 7.8 }, 'en')).toBe('HK$78.00');
  });

  it('formats in the zh-Hant (Hong Kong) locale', () => {
    expect(formatCost(10, { currency: 'HKD', usdRate: 7.8 }, 'zh-Hant')).toBe('HK$78.00');
  });
});
