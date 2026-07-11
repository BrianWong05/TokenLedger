import { describe, it, expect } from 'vitest';
import { formatTokens, formatCost, fmtTok, fmtPct, fmtIsoDate, fmtRemain, fmtAgo } from './format';

describe('formatTokens', () => {
  it('zero', () => expect(formatTokens(0)).toBe('0'));
  it('groups thousands', () => expect(formatTokens(1234)).toBe('1,234'));
  it('millions', () => expect(formatTokens(1_234_567)).toBe('1.23M'));
  it('billions', () => expect(formatTokens(1_234_567_890)).toBe('1.23B'));
  it('exact million boundary', () => expect(formatTokens(1_000_000)).toBe('1.00M'));
});

describe('formatCost', () => {
  it('null is unpriced', () => expect(formatCost(null, false)).toBe('unpriced'));
  it('priced', () => expect(formatCost(12.5, false)).toBe('$12.50'));
  it('unpriced marker', () => expect(formatCost(12.5, true)).toBe('≥ $12.50'));
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
});

const NOW = Date.parse('2026-07-12T12:00:00Z');
const ts = (iso: string) => Math.floor(Date.parse(iso) / 1000);

describe('fmtRemain', () => {
  it('em-dash for null', () => expect(fmtRemain(null, NOW)).toBe('—'));
  it('now for past', () => expect(fmtRemain(ts('2026-07-12T11:00:00Z'), NOW)).toBe('now'));
  it('minutes under an hour', () =>
    expect(fmtRemain(ts('2026-07-12T12:34:00Z'), NOW)).toBe('34m'));
  it('hours under a day', () =>
    expect(fmtRemain(ts('2026-07-12T14:30:00Z'), NOW)).toBe('2h'));
  it('days beyond 24h', () =>
    expect(fmtRemain(ts('2026-07-18T13:00:00Z'), NOW)).toBe('6d'));
});

describe('fmtAgo', () => {
  it('em-dash for null', () => expect(fmtAgo(null, NOW)).toBe('—'));
  it('just now under a minute', () =>
    expect(fmtAgo(ts('2026-07-12T11:59:30Z'), NOW)).toBe('just now'));
  it('minutes', () => expect(fmtAgo(ts('2026-07-12T11:15:00Z'), NOW)).toBe('45m ago'));
  it('hours', () => expect(fmtAgo(ts('2026-07-12T10:00:00Z'), NOW)).toBe('2h ago'));
  it('days', () => expect(fmtAgo(ts('2026-07-10T10:00:00Z'), NOW)).toBe('2d ago'));
});
