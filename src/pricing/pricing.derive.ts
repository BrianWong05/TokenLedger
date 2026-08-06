// Pure derivation for the Pricing tab: the four mutually-exclusive pricing
// states, filtering/search, chip counts, and per-1M rate formatting. Mirrors
// the design mock's state logic (screen 1a <script>). No React, no fetching —
// so every branch is unit-testable against a ModelPricing[].
import type { ModelPricing, RatesPerTok } from '../types';
import { sourceMeta, type ToolMeta } from '../overview/meta';

export type PriceState = 'ok' | 'override' | 'unpriced' | 'est';
export type PriceFilter = 'all' | 'unpriced' | 'override' | 'est';

// The active rate: Override wins, else the catalog rate (ADR-0009 chain).
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

export function toolMeta(tool: string): ToolMeta {
  return sourceMeta(tool);
}

// Displayed tool label (full source name, e.g. "Claude Code") — also the
// search target alongside the raw model name.
export function toolLabel(tool: string): string {
  return toolMeta(tool).source;
}

// The two catalog ids an origin can carry. Anything else is a publisher's name
// (ADR-0009), which is why these are compared against rather than enumerated.
const LITELLM_ORIGIN = 'litellm';
const ROUTED_ORIGIN = 'openrouter';

// An origin is either one of the two catalog ids or, when the rate came from the
// Model's own publisher, that publisher's name as the catalog reports it ("Z.AI",
// "Anthropic"). Those read fine as-is.
export function originLabel(origin: string): string {
  if (origin === LITELLM_ORIGIN) return 'LiteLLM';
  if (origin === ROUTED_ORIGIN) return 'OpenRouter';
  return origin;
}

// The source in prose, for the Override editor, where there is no second badge to
// carry the qualifier: "Anthropic", "LiteLLM", "OpenRouter (routed)". Keyed off
// the origin rather than isRoutedRate because the editor always talks about the
// CATALOG rate — the one an Override replaces or falls back to — which is still
// worth qualifying while an Override is active. The suffix is passed in so this
// stays free of i18n, like every other function here.
export function originLabelQualified(origin: string, routedSuffix: string): string {
  return originLabel(origin) + (origin === ROUTED_ORIGIN ? routedSuffix : '');
}

// A Routed Rate is blended across every host serving the Model and moves with
// their discounts — no publisher sets it (CONTEXT.md), so it is the one catalog
// figure the interface must not let pass for a published rate. Since ADR-0009 the
// OpenRouter origin means exactly this tier: a publisher rate read from that same
// catalog carries the publisher's name instead.
//
// False once an Override exists: the Override is then the active rate and what
// the interface reports, so qualifying the superseded figure would only confuse.
export function isRoutedRate(m: ModelPricing): boolean {
  return !m.overrideRates && m.catalog?.origin === ROUTED_ORIGIN;
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
