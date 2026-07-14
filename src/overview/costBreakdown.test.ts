import { describe, expect, it } from 'vitest';
import type { BreakdownRow } from '../types';
import { buildCostBreakdown, formatBreakdownCost, formatSourceCost } from './costBreakdown';

function row(overrides: Partial<BreakdownRow>): BreakdownRow {
  return {
    key: 'model',
    inputTokens: 1,
    outputTokens: 0,
    cacheReadTokens: 0,
    cacheWriteTokens: 0,
    totalTokens: 1,
    requests: 1,
    cost: 1,
    source: 'claude',
    reasoningTokens: null,
    convs: 1,
    cacheEstimated: false,
    ...overrides,
  };
}

describe('buildCostBreakdown', () => {
  it('groups Models by canonical Source name and orders Sources by subtotal Cost', () => {
    const groups = buildCostBreakdown([
      row({ source: 'codex', key: 'gpt-5.4', cost: 2 }),
      row({ source: 'claude', key: 'claude-opus', cost: 8 }),
      row({ source: 'claude', key: 'claude-sonnet', cost: 3 }),
    ]);

    expect(groups.map((group) => [group.sourceName, group.cost])).toEqual([
      ['Claude Code', 11],
      ['Codex', 2],
    ]);
  });

  it('orders priced Models by Cost and keeps Unpriced Models last', () => {
    const [group] = buildCostBreakdown([
      row({ key: 'z-unpriced', cost: null }),
      row({ key: 'cheap', cost: 1 }),
      row({ key: 'expensive', cost: 4, cacheEstimated: true }),
      row({ key: 'a-unpriced', cost: null }),
    ]);

    expect(group.cost).toBe(5);
    expect(group.unpricedCount).toBe(2);
    expect(group.models).toEqual([
      { name: 'expensive', cost: 4, cacheEstimated: true },
      { name: 'cheap', cost: 1, cacheEstimated: false },
      { name: 'a-unpriced', cost: null, cacheEstimated: false },
      { name: 'z-unpriced', cost: null, cacheEstimated: false },
    ]);
  });

  it('puts entirely Unpriced Sources after priced Sources', () => {
    const groups = buildCostBreakdown([
      row({ source: 'claude', key: 'claude-unknown', cost: null }),
      row({ source: 'codex', key: 'gpt-5.4', cost: 0 }),
    ]);

    expect(groups.map((group) => [group.sourceName, group.cost])).toEqual([
      ['Codex', 0],
      ['Claude Code', null],
    ]);
  });

  it('uses alphabetical ties and returns no groups for an empty period', () => {
    const groups = buildCostBreakdown([
      row({ source: 'codex', key: 'z-model', cost: 2 }),
      row({ source: 'codex', key: 'a-model', cost: 2 }),
      row({ source: 'claude', key: 'claude-model', cost: 4 }),
    ]);

    expect(groups.map((group) => group.sourceName)).toEqual(['Claude Code', 'Codex']);
    expect(groups[1].models.map((model) => model.name)).toEqual(['a-model', 'z-model']);
    expect(buildCostBreakdown([])).toEqual([]);
  });
});

describe('formatBreakdownCost', () => {
  it('shows actual tiny prices and clearly marks Partial and Unpriced Cost', () => {
    expect(formatBreakdownCost(4_148.76)).toBe('$4,148.76');
    expect(formatBreakdownCost(12.345)).toBe('$12.35');
    expect(formatBreakdownCost(0.004)).toBe('$0.0040');
    expect(formatBreakdownCost(0)).toBe('$0.00');
    expect(formatBreakdownCost(12.34, true)).toBe('≥ $12.34');
    expect(formatBreakdownCost(null)).toBe('Unpriced');
  });

  it('includes the Unpriced count in a Partial Source subtotal', () => {
    expect(formatSourceCost(12.34, 1)).toBe('≥ $12.34 · 1 unpriced');
    expect(formatSourceCost(12.34, 2)).toBe('≥ $12.34 · 2 unpriced');
    expect(formatSourceCost(null, 2)).toBe('Unpriced');
  });
});
