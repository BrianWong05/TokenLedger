import { describe, expect, it } from 'vitest';
import { seedPricing } from './pricing.fake';
import {
  modelState, filterModels, chipCounts, fmtRate, fill, resolvedRates, originLabel,
} from './pricing.derive';
import type { ModelPricing } from '../types';

const byName = (name: string): ModelPricing => seedPricing().find((m) => m.model === name)!;

describe('modelState', () => {
  it('classifies each of the four states', () => {
    expect(modelState(byName('claude-opus-4-8'))).toBe('ok');
    expect(modelState(byName('hermes-4-405b'))).toBe('override');
    expect(modelState(byName('hermes-4-70b'))).toBe('unpriced');
    // input+output priced, both cache rates null -> cache-estimated
    expect(modelState(byName('gpt-5.5-codex'))).toBe('est');
  });

  it('override wins even when its cache rates are null', () => {
    const m: ModelPricing = {
      model: 'x', tool: 'hermes',
      overrideRates: { input: 1e-6, output: 2e-6, cacheRead: null, cacheWrite: null },
      catalog: null,
    };
    expect(modelState(m)).toBe('override');
  });
});

describe('resolvedRates', () => {
  it('prefers the override, else the catalog, else null', () => {
    expect(resolvedRates(byName('hermes-4-405b'))).toEqual(byName('hermes-4-405b').overrideRates);
    expect(resolvedRates(byName('claude-opus-4-8'))).toEqual(byName('claude-opus-4-8').catalog!.rates);
    expect(resolvedRates(byName('hermes-4-70b'))).toBeNull();
  });
});

describe('chipCounts', () => {
  it('counts the design 12-model set', () => {
    expect(chipCounts(seedPricing())).toEqual({ all: 12, unpriced: 2, override: 1, est: 1 });
  });
});

describe('filterModels', () => {
  const models = seedPricing();

  it('filters by state', () => {
    expect(filterModels(models, '', 'unpriced').map((m) => m.model)).toEqual(['hermes-4-70b', 'antigravity-flow-1']);
    expect(filterModels(models, '', 'override').map((m) => m.model)).toEqual(['hermes-4-405b']);
    expect(filterModels(models, '', 'est').map((m) => m.model)).toEqual(['gpt-5.5-codex']);
    expect(filterModels(models, '', 'all')).toHaveLength(12);
  });

  it('searches model name (case-insensitive)', () => {
    expect(filterModels(models, 'CLAUDE', 'all').map((m) => m.model)).toEqual([
      'claude-opus-4-8', 'claude-sonnet-4-8', 'claude-haiku-4-5',
    ]);
  });

  it('searches the tool label too', () => {
    // "Gemini CLI" is the source label, not in any model name
    expect(filterModels(models, 'gemini cli', 'all').map((m) => m.model)).toEqual(['gemini-3-pro', 'gemini-3-flash']);
  });

  it('combines search and state filter', () => {
    expect(filterModels(models, 'hermes', 'unpriced').map((m) => m.model)).toEqual(['hermes-4-70b']);
  });
});

describe('fmtRate', () => {
  it('renders per-1M USD with 3 decimals below $0.10', () => {
    expect(fmtRate(15 / 1_000_000)).toBe('$15.00');
    expect(fmtRate(0.3 / 1_000_000)).toBe('$0.30');
    expect(fmtRate(0.04 / 1_000_000)).toBe('$0.040');
    expect(fmtRate(0.075 / 1_000_000)).toBe('$0.075');
  });

  it('renders an em dash for a null rate', () => {
    expect(fmtRate(null)).toBe('—');
  });
});

describe('originLabel', () => {
  it('maps catalog origins to display names', () => {
    expect(originLabel('litellm')).toBe('LiteLLM');
    expect(originLabel('openrouter')).toBe('OpenRouter');
  });
});

describe('fill', () => {
  it('interpolates named tokens', () => {
    expect(fill('{a} of {b}', { a: 2, b: 12 })).toBe('2 of 12');
    expect(fill('{x} left {x}', { x: 'z' })).toBe('z left z');
  });
});
