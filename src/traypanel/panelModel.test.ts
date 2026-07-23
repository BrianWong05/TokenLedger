import { describe, expect, it } from 'vitest';
import { panelModel, periodWindows } from './panelModel';
import { DEFAULT_SETTINGS } from '../settings/settings';
import type { BreakdownRow, Summary } from '../types';

function sum(totalTokens: number, cost: number | null, hasUnpriced = false, requests = 0): Summary {
  return {
    inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0,
    totalTokens, requests, cost, hasUnpriced,
    unpricedModels: [], cacheEstimatedModels: [], cacheHitRate: 0,
  };
}

function brow(key: string, totalTokens: number, cost: number | null, hasUnpriced = false): BreakdownRow {
  return {
    key, source: null, inputTokens: 0, outputTokens: 0, cacheReadTokens: 0,
    cacheWriteTokens: 0, totalTokens, requests: 0, cost, reasoningTokens: null,
    convs: 0, cacheEstimated: false, hasUnpriced,
  };
}

const S = DEFAULT_SETTINGS;

describe('panelModel', () => {
  it('renders the 2b header: cost, delta vs same-time-yesterday, tokens and requests', () => {
    const m = panelModel(sum(3_400_000, 12.84, false, 1912), sum(1_000_000, 10.0), [], S, 'en');
    expect(m.cost).toBe('$12.84');
    expect(m.delta).toBe('+28.4%'); // 12.84 / 10 → +28.4, one decimal
    expect(m.deltaUp).toBe(true);
    expect(m.sub).toBe('3.4M tok · 1,912 req');
  });

  it('falling pace reads negative and not-up', () => {
    const m = panelModel(sum(3_400_000, 9.0), sum(1_000_000, 10.0), [], S, 'en');
    expect(m.delta).toBe('-10.0%');
    expect(m.deltaUp).toBe(false);
  });

  it('delta hidden when yesterday-so-far had no cost — zero or unpriced', () => {
    expect(panelModel(sum(1, 5), sum(0, 0), [], S, 'en').delta).toBeNull();
    expect(panelModel(sum(1, 5), sum(9, null, true), [], S, 'en').delta).toBeNull();
    expect(panelModel(sum(1, null, true), sum(9, 10), [], S, 'en').delta).toBeNull();
  });

  it('partial cost carries the marker; all-unpriced says unpriced, never $0', () => {
    expect(panelModel(sum(1, 12.8, true), sum(0, null), [], S, 'en').cost).toBe('≥ $12.80');
    expect(panelModel(sum(1, null, true), sum(0, null), [], S, 'en').cost).toBe('unpriced');
  });

  it('honors Display Currency like every other Cost in the app', () => {
    const m = panelModel(sum(1, 10.0), sum(0, null), [], { ...S, currency: 'HKD', usdRate: 7.8 }, 'en');
    expect(m.cost).toBe('HK$78.00');
  });

  it('empty day is flagged and shows no delta even when yesterday had usage', () => {
    const m = panelModel(sum(0, 0), sum(9, 10.0), [], S, 'en');
    expect(m.empty).toBe(true);
    expect(m.delta).toBeNull(); // never "No usage yet" beside "-100.0%"
  });

  it('source rows: zero-usage absent, cost desc, all-unpriced last by tokens, per-row ≥', () => {
    const m = panelModel(sum(1, 1), sum(0, null), [
      brow('codex', 238_100, 1.11),
      brow('gemini', 0, null),
      brow('grok', 964_200, null, true),
      brow('hermes', 500_000, 2.0),
      brow('claude', 1_800_000, 6.12, true), // mixed → partial
    ], S, 'en');
    expect(m.rows.map((r) => [r.label, r.tokens, r.cost])).toEqual([
      ['Claude', '1.8M', '≥ $6.12'],
      ['Hermes', '500K', '$2.00'],
      ['Codex', '238.1K', '$1.11'],
      ['Grok', '964.2K', 'unpriced'],
    ]);
  });

  it('exposes raw values and per-frame formatters for the count-up animation', () => {
    const m = panelModel(sum(3_400_000, 10.0, true, 1912), sum(0, null), [], { ...S, currency: 'HKD', usdRate: 7.8 }, 'en');
    expect(m.costValue).toBe(10.0); // USD, conversion happens in fmtCost
    expect(m.tokensValue).toBe(3_400_000);
    expect(m.requestsText).toBe('1,912');
    expect(m.fmtCost(5.0)).toBe('≥ HK$39.00'); // marker survives every frame
    expect(m.fmtTokens(964_200)).toBe('964.2K');
  });

  it('unknown sources keep their raw key and never disappear', () => {
    const m = panelModel(sum(1, 1), sum(0, null), [brow('weirdtool', 1_000, 1.0)], S, 'en');
    expect(m.rows[0].label).toBe('weirdtool');
    expect(m.rows[0].icon).toBeUndefined();
  });
});

describe('periodWindows', () => {
  const now = new Date(2026, 5, 15, 10, 30, 0); // June 15, 10:30 local
  const mid = (d: number) => Math.floor(new Date(2026, 5, d).getTime() / 1000);

  it('today brackets the local calendar day; comparison clamped to now − 24h', () => {
    const w = periodWindows('today', now);
    expect(w.start).toBe(mid(15));
    expect(w.end).toBe(mid(16));
    // so-far vs so-far: yesterday up to the same time.
    expect(w.prevStart).toBe(mid(14));
    expect(w.prevEnd).toBe(Math.floor(now.getTime() / 1000) - 86_400);
  });

  it('yesterday is the full previous day vs the full day before it', () => {
    const w = periodWindows('yesterday', now);
    expect(w.start).toBe(mid(14));
    expect(w.end).toBe(mid(15));
    expect(w.prevStart).toBe(mid(13));
    expect(w.prevEnd).toBe(mid(14));
  });

  it('30 days trails 30 calendar days including today vs the previous 30', () => {
    const w = periodWindows('days30', now);
    expect(w.start).toBe(Math.floor(new Date(2026, 4, 17).getTime() / 1000)); // May 17
    expect(w.end).toBe(mid(16)); // through today, end-exclusive tomorrow
    expect(w.prevStart).toBe(Math.floor(new Date(2026, 3, 17).getTime() / 1000)); // Apr 17
    expect(w.prevEnd).toBe(Math.floor(new Date(2026, 4, 17).getTime() / 1000));
  });
});
