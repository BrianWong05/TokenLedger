// In-memory PricingPort for tests: a mutable model list, a call log, per-method
// failNext, and a prices-rebuilt emitter — mirroring ledger.fake.ts in spirit.
import type { PricingPort } from './pricing';
import type { ModelPricing, RatesPerTok } from '../types';

export interface FakePricing extends PricingPort {
  models: ModelPricing[];
  calls: { list: number; setOverride: [string, RatesPerTok][]; deleteOverride: string[] };
  failNext(method: 'list' | 'setOverride' | 'deleteOverride', err: unknown): void;
  emitPricesRebuilt(): void;
}

// The design's 12-model dataset (screen 1a): the four pricing states spread
// across every Source — 8 catalog-priced (ok), 1 override, 1 cache-estimated
// (input+output priced, cache rates null), 2 unpriced. Rates are USD per token.
export function seedPricing(): ModelPricing[] {
  const per1m = (n: number) => n / 1_000_000;
  const rates = (i: number, o: number, cr: number, cw: number): RatesPerTok => ({
    input: per1m(i), output: per1m(o), cacheRead: per1m(cr), cacheWrite: per1m(cw),
  });
  const cat = (origin: 'litellm' | 'openrouter', r: RatesPerTok) => ({ origin, rates: r });
  return [
    { model: 'claude-opus-4-8', tool: 'claude', overrideRates: null, catalog: cat('litellm', rates(15, 75, 1.5, 18.75)) },
    { model: 'claude-sonnet-4-8', tool: 'claude', overrideRates: null, catalog: cat('litellm', rates(3, 15, 0.3, 3.75)) },
    { model: 'claude-haiku-4-5', tool: 'claude', overrideRates: null, catalog: cat('litellm', rates(0.8, 4, 0.08, 1)) },
    // Cache-Estimated: input + output priced, both cache rates null.
    { model: 'gpt-5.5-codex', tool: 'codex', overrideRates: null,
      catalog: cat('litellm', { input: per1m(1.75), output: per1m(14), cacheRead: null, cacheWrite: null }) },
    { model: 'gpt-5.5-mini', tool: 'codex', overrideRates: null, catalog: cat('litellm', rates(0.35, 2.8, 0.04, 0.44)) },
    { model: 'gemini-3-pro', tool: 'gemini', overrideRates: null, catalog: cat('litellm', rates(2.5, 15, 0.31, 4.5)) },
    { model: 'gemini-3-flash', tool: 'gemini', overrideRates: null, catalog: cat('openrouter', rates(0.3, 2.5, 0.075, 1)) },
    // Override wins over its OpenRouter catalog entry.
    { model: 'hermes-4-405b', tool: 'hermes', overrideRates: rates(1.2, 3.6, 0.12, 1.5), catalog: cat('openrouter', rates(1.1, 3.3, 0.11, 1.38)) },
    { model: 'hermes-4-70b', tool: 'hermes', overrideRates: null, catalog: null },
    { model: 'grok-code-2', tool: 'grok', overrideRates: null, catalog: cat('openrouter', rates(0.85, 6, 0.09, 1.1)) },
    { model: 'grok-4-fast', tool: 'grok', overrideRates: null, catalog: cat('openrouter', rates(0.2, 0.5, 0.05, 0.25)) },
    { model: 'antigravity-flow-1', tool: 'antigravity', overrideRates: null, catalog: null },
  ];
}

export function makeFakePricing(seed: ModelPricing[] = seedPricing()): FakePricing {
  const models = seed.map((m) => ({ ...m }));
  const calls: FakePricing['calls'] = { list: 0, setOverride: [], deleteOverride: [] };
  const fails = new Map<string, unknown>();
  const cbs = new Set<() => void>();

  const guard = <T>(method: string, value: () => T): Promise<T> => {
    if (fails.has(method)) {
      const e = fails.get(method);
      fails.delete(method);
      return Promise.reject(e);
    }
    return Promise.resolve(value());
  };

  return {
    models,
    calls,
    list: () => guard('list', () => { calls.list++; return models.map((m) => ({ ...m })); }),
    setOverride: (model, rates) =>
      guard('setOverride', () => {
        calls.setOverride.push([model, rates]);
        const m = models.find((x) => x.model === model);
        if (m) m.overrideRates = rates;
      }),
    deleteOverride: (model) =>
      guard('deleteOverride', () => {
        calls.deleteOverride.push(model);
        const m = models.find((x) => x.model === model);
        if (m) m.overrideRates = null;
      }),
    onPricesRebuilt: (cb) => {
      cbs.add(cb);
      return () => cbs.delete(cb);
    },
    failNext: (method, err) => fails.set(method, err),
    emitPricesRebuilt: () => cbs.forEach((cb) => cb()),
  };
}
