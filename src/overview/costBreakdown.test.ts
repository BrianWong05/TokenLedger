import { describe, expect, it } from 'vitest';
import type { BreakdownRow, Summary } from '../types';
import {
  buildCostBreakdownView,
  formatBreakdownCost,
  formatSourceCost,
} from './costBreakdown';

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

function summary(overrides: Partial<Summary>): Summary {
  return {
    inputTokens: 1,
    outputTokens: 0,
    cacheReadTokens: 0,
    cacheWriteTokens: 0,
    totalTokens: 1,
    requests: 1,
    cost: 1,
    hasUnpriced: false,
    unpricedModels: [],
    cacheEstimatedModels: [],
    cacheHitRate: 0,
    ...overrides,
  };
}

describe('buildCostBreakdownView', () => {
  it('describes an all-Unpriced period without calling it Partial Cost', () => {
    const view = buildCostBreakdownView(
      summary({ cost: null, hasUnpriced: true, unpricedModels: ['claude-unknown'] }),
      [row({ key: 'claude-unknown', cost: null })],
    );

    expect(view.totalCostLabel).toBe('Unpriced');
    expect(view.note).toBeNull();
    expect(view.groups).toEqual([
      {
        sourceKey: 'claude',
        sourceName: 'Claude Code',
        costLabel: 'Unpriced',
        models: [
          {
            name: 'claude-unknown',
            costLabel: 'Unpriced',
            unpriced: true,
            cacheEstimated: false,
          },
        ],
      },
    ]);
  });

  it('converts every Cost to the Display Currency while keeping the ≥ / unpriced markers', () => {
    const view = buildCostBreakdownView(
      summary({ cost: 5, hasUnpriced: true, unpricedModels: ['z', 'a'] }),
      [row({ key: 'z', cost: null }), row({ key: 'cheap', cost: 5 }), row({ key: 'a', cost: null })],
      { currency: 'HKD', usdRate: 7.8 }, // fixed HKD rate; stored figures stay USD
      'en',
    );

    // 5 USD → HK$39.00, with the ≥ Partial-Cost prefix and Unpriced count intact.
    expect(view.totalCostLabel).toBe('≥ HK$39.00');
    expect(view.note).toBe('Partial Cost · 2 Unpriced Models');
    expect(view.groups[0].costLabel).toBe('≥ HK$39.00 · 2 unpriced');
    expect(view.groups[0].models.find((m) => m.name === 'cheap')!.costLabel).toBe('HK$39.00');
    // A null Cost stays "Unpriced" — never a converted HK$0.00.
    expect(view.groups[0].models.find((m) => m.unpriced)!.costLabel).toBe('Unpriced');
  });

  it('keeps Cache-Estimated as a Model marker without making Cost Partial', () => {
    const view = buildCostBreakdownView(
      summary({ cost: 4, cacheEstimatedModels: ['claude-opus'] }),
      [row({ key: 'claude-opus', cost: 4, cacheEstimated: true })],
    );

    expect(view.totalCostLabel).toBe('$4.00');
    expect(view.note).toBeNull();
    expect(view.groups[0].costLabel).toBe('$4.00');
    expect(view.groups[0].models[0].cacheEstimated).toBe(true);
  });
});

describe('buildCostBreakdownView grouping', () => {
  it('groups Models by canonical Source name and orders Sources by subtotal Cost', () => {
    const { groups } = buildCostBreakdownView(
      summary({ cost: 13 }),
      [
        row({ source: 'codex', key: 'gpt-5.4', cost: 2 }),
        row({ source: 'claude', key: 'claude-opus', cost: 8 }),
        row({ source: 'claude', key: 'claude-sonnet', cost: 3 }),
      ],
    );

    expect(groups.map((group) => [group.sourceName, group.costLabel])).toEqual([
      ['Claude Code', '$11.00'],
      ['Codex', '$2.00'],
    ]);
  });

  it('orders priced Models by Cost and keeps Unpriced Models last', () => {
    const { groups } = buildCostBreakdownView(
      summary({
        cost: 5,
        hasUnpriced: true,
        unpricedModels: ['z-unpriced', 'a-unpriced'],
      }),
      [
        row({ key: 'z-unpriced', cost: null }),
        row({ key: 'cheap', cost: 1 }),
        row({ key: 'expensive', cost: 4, cacheEstimated: true }),
        row({ key: 'a-unpriced', cost: null }),
      ],
    );
    const [group] = groups;

    expect(group.costLabel).toBe('≥ $5.00 · 2 unpriced');
    expect(group.models).toEqual([
      { name: 'expensive', costLabel: '$4.00', unpriced: false, cacheEstimated: true },
      { name: 'cheap', costLabel: '$1.00', unpriced: false, cacheEstimated: false },
      { name: 'a-unpriced', costLabel: 'Unpriced', unpriced: true, cacheEstimated: false },
      { name: 'z-unpriced', costLabel: 'Unpriced', unpriced: true, cacheEstimated: false },
    ]);
  });

  it('puts entirely Unpriced Sources after priced Sources', () => {
    const { groups } = buildCostBreakdownView(
      summary({ cost: 0, hasUnpriced: true, unpricedModels: ['claude-unknown'] }),
      [
        row({ source: 'claude', key: 'claude-unknown', cost: null }),
        row({ source: 'codex', key: 'gpt-5.4', cost: 0 }),
      ],
    );

    expect(groups.map((group) => [group.sourceName, group.costLabel])).toEqual([
      ['Codex', '$0.00'],
      ['Claude Code', 'Unpriced'],
    ]);
  });

  it('uses alphabetical ties and returns no groups for an empty period', () => {
    const { groups } = buildCostBreakdownView(
      summary({ cost: 8 }),
      [
        row({ source: 'codex', key: 'z-model', cost: 2 }),
        row({ source: 'codex', key: 'a-model', cost: 2 }),
        row({ source: 'claude', key: 'claude-model', cost: 4 }),
      ],
    );

    expect(groups.map((group) => group.sourceName)).toEqual(['Claude Code', 'Codex']);
    expect(groups[1].models.map((model) => model.name)).toEqual(['a-model', 'z-model']);
    expect(buildCostBreakdownView(summary({ cost: null }), []).groups).toEqual([]);
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
