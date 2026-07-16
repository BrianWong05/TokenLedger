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

// A small spread of pricing states (priced / override / unpriced / cache-est.)
// so page tests have every badge to render. Rates are USD per token.
export function seedPricing(): ModelPricing[] {
  const per1m = (n: number) => n / 1_000_000;
  const rates = (i: number, o: number, cr: number, cw: number): RatesPerTok => ({
    input: per1m(i), output: per1m(o), cacheRead: per1m(cr), cacheWrite: per1m(cw),
  });
  return [
    { model: 'claude-opus-4-8', tool: 'claude', overrideRates: null,
      catalog: { origin: 'litellm', rates: rates(15, 75, 1.5, 18.75) } },
    { model: 'gpt-5.5-codex', tool: 'codex', overrideRates: null,
      catalog: { origin: 'litellm', rates: { input: per1m(1.75), output: per1m(14), cacheRead: null, cacheWrite: null } } },
    { model: 'gemini-3-flash', tool: 'gemini', overrideRates: null,
      catalog: { origin: 'openrouter', rates: rates(0.3, 2.5, 0.075, 1) } },
    { model: 'hermes-4-405b', tool: 'hermes', overrideRates: rates(1.2, 3.6, 0.12, 1.5),
      catalog: { origin: 'openrouter', rates: rates(1.1, 3.3, 0.11, 1.38) } },
    { model: 'hermes-4-70b', tool: 'hermes', overrideRates: null, catalog: null },
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
