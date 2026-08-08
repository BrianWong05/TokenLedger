import { describe, it, expect } from 'vitest';
import { formatCost, fmtTok, fmtPct, fmtIsoDate, formatCompactTokenTotal } from './format';

describe('formatCost', () => {
  it('null is unpriced', () => expect(formatCost(null, false)).toBe('unpriced'));
  it('priced', () => expect(formatCost(12.5, false)).toBe('$12.50'));
  it('unpriced marker', () => expect(formatCost(12.5, true)).toBe('≥ $12.50'));
  it('supports detailed Cost labels without losing tiny nonzero values', () => {
    expect(formatCost(4_148.76, false, { adaptivePrecision: true })).toBe('$4,148.76');
    expect(formatCost(0.004, false, { adaptivePrecision: true })).toBe('$0.0040');
    expect(formatCost(null, false, { unpricedLabel: 'Unpriced' })).toBe('Unpriced');
  });
});

describe('overview formatters', () => {
  it('fmtTok scales K/M/B', () => {
    expect(fmtTok(950)).toBe('950');
    expect(fmtTok(1500)).toBe('1.5K');
    expect(fmtTok(2_340_000)).toBe('2.34M');
    expect(fmtTok(1_200_000_000)).toBe('1.20B');
  });
  it('fmtTok rounds fractional averages instead of printing float noise', () => {
    expect(fmtTok(850 / 3)).toBe('283');
  });
  it('fmtTok rolls over units at rounding boundaries', () => {
    expect(fmtTok(999_600)).toBe('1.00M'); // not '1000K'
    expect(fmtTok(999_950_000)).toBe('1.00B'); // not '1000.0M'
    expect(fmtTok(999.6)).toBe('1.0K'); // not '1000'
  });
  it('fmtPct adapts precision below 10%', () => {
    expect(fmtPct(0.5)).toBe('50%');
    expect(fmtPct(0.043)).toBe('4.3%');
  });
  it('fmtIsoDate renders a local date', () => {
    expect(fmtIsoDate('2026-07-04')).toBe('Jul 4');
  });
  it('formatCompactTokenTotal spends its extra decimal only where it says something', () => {
    // The Overview asks for three decimals, the tray panel keeps the default two.
    expect(formatCompactTokenTotal(2_174_300_000, 3)).toBe('2.174B');
    expect(formatCompactTokenTotal(2_174_300_000)).toBe('2.17B');
    // Trailing zeros stay dropped at either precision, so round totals stay short.
    expect(formatCompactTokenTotal(11_000_000_000, 3)).toBe('11B');
    expect(formatCompactTokenTotal(1_200_000, 3)).toBe('1.2M');
  });
  it('formatCompactTokenTotal rolls over before three decimals can print a full unit', () => {
    expect(formatCompactTokenTotal(999_999_600, 3)).toBe('1B'); // not '1000M'
    expect(formatCompactTokenTotal(999_999, 3)).toBe('999.999K');
  });
});
