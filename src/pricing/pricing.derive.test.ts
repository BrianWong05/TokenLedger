import { describe, expect, it } from 'vitest';
import { seedPricing } from './pricing.fake';
import {
  modelState, filterModels, chipCounts, fmtRate, fill, resolvedRates, originLabel, isRoutedRate,
  sortPricing, defaultDir, sourceLabel, overallRate,
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

  it('passes a publisher name through as its own label', () => {
    // A publisher rate carries the publishing organisation's name, which is
    // already how it should read — the column names who set the rate.
    expect(originLabel('Z.AI')).toBe('Z.AI');
    expect(originLabel('Anthropic')).toBe('Anthropic');
  });
});

describe('isRoutedRate', () => {
  const withOrigin = (origin: string | null) =>
    ({
      model: 'm',
      tool: 'claude',
      overrideRates: null,
      catalog: origin === null ? null : { origin, rates: { input: 1, output: 1, cacheRead: null, cacheWrite: null } },
    }) as ModelPricing;

  it('is true only for the routed tier, never for a published rate', () => {
    expect(isRoutedRate(withOrigin('openrouter'))).toBe(true);
    expect(isRoutedRate(withOrigin('litellm'))).toBe(false);
    // A publisher's own rate is a List Price, however it was fetched.
    expect(isRoutedRate(withOrigin('Z.AI'))).toBe(false);
    expect(isRoutedRate(withOrigin(null))).toBe(false);
  });

  it('is false once an Override supersedes the routed figure', () => {
    // The Override is the active rate, so that is what the interface reports —
    // qualifying the figure it replaced would only confuse.
    const overridden = {
      ...withOrigin('openrouter'),
      overrideRates: { input: 1, output: 1, cacheRead: null, cacheWrite: null },
    } as ModelPricing;
    expect(isRoutedRate(overridden)).toBe(false);
  });
});

describe('fill', () => {
  it('interpolates named tokens', () => {
    expect(fill('{a} of {b}', { a: 2, b: 12 })).toBe('2 of 12');
    expect(fill('{x} left {x}', { x: 'z' })).toBe('z left z');
  });
});

describe('sortPricing', () => {
  const all = seedPricing();
  const isUnpriced = (m: ModelPricing) => resolvedRates(m) === null;

  it('input desc puts the priciest input first and every unpriced Model last', () => {
    const out = sortPricing(all, 'input', 'desc');
    expect(out[0].model).toBe('claude-opus-4-8'); // 15/1M — the max input rate
    const firstUnpriced = out.findIndex(isUnpriced);
    expect(firstUnpriced).toBeGreaterThan(-1);
    expect(out.slice(firstUnpriced).every(isUnpriced)).toBe(true); // unpriced tail
  });

  it('input asc keeps unpriced last, cheapest priced first — not mutating input', () => {
    const before = all.map((m) => m.model);
    const out = sortPricing(all, 'input', 'asc');
    expect(out[0].model).toBe('grok-4-fast'); // 0.2/1M — the cheapest input
    const firstUnpriced = out.findIndex(isUnpriced);
    expect(out.slice(firstUnpriced).every(isUnpriced)).toBe(true);
    expect(all.map((m) => m.model)).toEqual(before); // pure
  });

  it('source groups Models by their owning Source (Source label), A–Z', () => {
    const labels = sortPricing(all, 'source', 'asc').map((m) => sourceLabel(m.tool));
    expect(labels).toEqual([...labels].sort((a, b) => a.localeCompare(b)));
  });

  it('a Model with no rate for the sorted column sinks last, like an unpriced Model', () => {
    // gpt-5.5-codex is priced overall but has null cacheRead/cacheWrite (est) —
    // under a cache column it has no price, so it joins the null tail.
    const out = sortPricing(all, 'cacheRead', 'desc');
    const noRate = (m: ModelPricing) => (resolvedRates(m)?.cacheRead ?? null) === null;
    const firstNull = out.findIndex(noRate);
    expect(out.slice(firstNull).every(noRate)).toBe(true);
    expect(out.slice(firstNull).some((m) => m.model === 'gpt-5.5-codex')).toBe(true);
  });

  it('model sorts by name A–Z', () => {
    const names = sortPricing(all, 'model', 'asc').map((m) => m.model);
    expect(names).toEqual([...names].sort((a, b) => a.localeCompare(b)));
  });

  it('opens price columns (rates + Overall) highest-first and text columns A–Z', () => {
    expect(defaultDir('input')).toBe('desc');
    expect(defaultDir('output')).toBe('desc');
    expect(defaultDir('overall')).toBe('desc');
    expect(defaultDir('model')).toBe('asc');
    expect(defaultDir('source')).toBe('asc');
  });

  it('overall desc puts the priciest total first and unpriced last', () => {
    const out = sortPricing(all, 'overall', 'desc');
    expect(out[0].model).toBe('claude-opus-4-8'); // 15+75+1.5+18.75 = 110.25/1M, the max total
    const firstUnpriced = out.findIndex(isUnpriced);
    expect(out.slice(firstUnpriced).every(isUnpriced)).toBe(true);
  });
});

describe('overallRate', () => {
  const byName = (name: string) => seedPricing().find((m) => m.model === name)!;

  it('sums the four resolved rates', () => {
    expect(fmtRate(overallRate(byName('claude-opus-4-8')))).toBe('$110.25'); // 15+75+1.5+18.75
  });

  it('sums only the present rates for a cache-estimated Model', () => {
    expect(fmtRate(overallRate(byName('gpt-5.5-codex')))).toBe('$15.75'); // 1.75+14, cache null
  });

  it('is null for an unpriced Model', () => {
    expect(overallRate(byName('hermes-4-70b'))).toBeNull();
  });
});
