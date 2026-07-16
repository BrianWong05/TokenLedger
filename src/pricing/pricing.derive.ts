// Pure derivation for the Pricing tab: the four mutually-exclusive pricing
// states, filtering/search, chip counts, and per-1M rate formatting. Mirrors
// the design mock's state logic (screen 1a <script>). No React, no fetching —
// so every branch is unit-testable against a ModelPricing[].
import type { ModelPricing, RatesPerTok } from '../types';
import { TOOLS, type ToolMeta } from '../overview/meta';

export type PriceState = 'ok' | 'override' | 'unpriced' | 'est';
export type PriceFilter = 'all' | 'unpriced' | 'override' | 'est';

// The active rate: Override wins, else the catalog rate (ADR-0003 chain).
export function resolvedRates(m: ModelPricing): RatesPerTok | null {
  return m.overrideRates ?? m.catalog?.rates ?? null;
}

// One mutually-exclusive state per Model (drives badge, action, chip, tint):
//   unpriced  — no Override and no catalog rate
//   override  — an Override is present
//   est       — catalog-priced for input+output but both cache rates are null
//   ok        — everything else
export function modelState(m: ModelPricing): PriceState {
  if (!m.overrideRates && !m.catalog) return 'unpriced';
  if (m.overrideRates) return 'override';
  const r = m.catalog!.rates;
  if (r.input != null && r.output != null && r.cacheRead == null && r.cacheWrite == null) return 'est';
  return 'ok';
}

export function toolMeta(tool: string): ToolMeta | undefined {
  return TOOLS.find((t) => t.key === tool);
}

// Displayed tool label (full source name, e.g. "Claude Code") — also the
// search target alongside the raw model name.
export function toolLabel(tool: string): string {
  return toolMeta(tool)?.source ?? tool;
}

export function originLabel(origin: 'litellm' | 'openrouter'): string {
  return origin === 'litellm' ? 'LiteLLM' : 'OpenRouter';
}

// case-insensitive match on model name OR tool label, then the state filter.
export function filterModels(models: ModelPricing[], query: string, filter: PriceFilter): ModelPricing[] {
  const q = query.trim().toLowerCase();
  return models.filter((m) => {
    if (filter !== 'all' && modelState(m) !== filter) return false;
    if (!q) return true;
    return m.model.toLowerCase().includes(q) || toolLabel(m.tool).toLowerCase().includes(q);
  });
}

export interface ChipCounts {
  all: number;
  unpriced: number;
  override: number;
  est: number;
}

export function chipCounts(models: ModelPricing[]): ChipCounts {
  const c: ChipCounts = { all: models.length, unpriced: 0, override: 0, est: 0 };
  for (const m of models) {
    const s = modelState(m);
    if (s === 'unpriced') c.unpriced++;
    else if (s === 'override') c.override++;
    else if (s === 'est') c.est++;
  }
  return c;
}

// Per-token USD -> "$X.XX per 1M", 3 decimals below $0.10; null -> em dash.
// (fmt() in the mock takes the already-per-1M value; rates here are per-token.)
export function fmtRate(perTok: number | null): string {
  if (perTok == null) return '—';
  const v = perTok * 1_000_000;
  return '$' + (v < 0.1 ? v.toFixed(3) : v.toFixed(2));
}

// Interpolate {name} tokens in a translated string.
export function fill(s: string, vars: Record<string, string | number>): string {
  return s.replace(/\{(\w+)\}/g, (_, k) => (k in vars ? String(vars[k]) : `{${k}}`));
}
