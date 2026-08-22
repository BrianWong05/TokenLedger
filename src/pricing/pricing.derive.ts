// Pure derivation for the Pricing tab: the four mutually-exclusive pricing
// states, filtering/search, chip counts, and per-1M rate formatting. Mirrors
// the design mock's state logic (screen 1a <script>). No React, no fetching —
// so every branch is unit-testable against a ModelPricing[].
import type { ModelPricing, RatesPerTok } from '../types';
import { sourceMeta, type SourceMeta } from '../overview/meta';

export type PriceState = 'ok' | 'override' | 'unpriced' | 'est';
export type PriceFilter = 'all' | 'unpriced' | 'override' | 'est';

// The active rate: Override wins, else the catalog rate (ADR-0009 chain).
export function resolvedRates(m: ModelPricing): RatesPerTok | null {
  return m.overrideRates ?? m.catalog?.rates ?? null;
}

// Cache-Estimated (CONTEXT.md): a Model priced for input+output whose Cache
// rates are absent. The wire maps a stored 0.0 to null
// (`Rates::rate_absent` / `to_per_tok`). Any missing bucket is enough —
// Cost's cache_gap uses the same per-bucket predicate, then also requires
// counted tokens in that bucket.
export function cacheRatesAbsent(r: RatesPerTok): boolean {
  return r.cacheRead == null || r.cacheWrite == null;
}

// One mutually-exclusive state per Model (drives badge, action, chip, tint):
//   unpriced  — no Override and no catalog rate
//   override  — an Override is present
//   est       — catalog-priced for input+output but any cache rate is absent
//   ok        — everything else
export function modelState(m: ModelPricing): PriceState {
  if (!m.overrideRates && !m.catalog) return 'unpriced';
  if (m.overrideRates) return 'override';
  const r = m.catalog!.rates;
  if (r.input != null && r.output != null && cacheRatesAbsent(r)) return 'est';
  return 'ok';
}

export function pricingSourceMeta(source: string): SourceMeta {
  return sourceMeta(source);
}

// Displayed Source label (full source name, e.g. "Claude Code") — also the
// search target alongside the raw model name.
export function sourceLabel(source: string): string {
  return pricingSourceMeta(source).source;
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
    return m.model.toLowerCase().includes(q) || sourceLabel(m.tool).toLowerCase().includes(q);
  });
}

// ---- table sorting (TOKL-10) ----

// The columns the Pricing table can be sorted by: two text columns, the Overall
// column, and the four rate columns (which map 1:1 onto RatesPerTok keys).
export type SortCol = 'model' | 'source' | 'overall' | 'input' | 'output' | 'cacheRead' | 'cacheWrite';
export type SortDir = 'asc' | 'desc';

const RATE_COLS = new Set<SortCol>(['input', 'output', 'cacheRead', 'cacheWrite']);
// Columns that hold a price: they open highest-first and put unpriced Models last.
const NUMERIC_COLS = new Set<SortCol>([...RATE_COLS, 'overall']);

// The Overall List Price: the sum of a Model's four resolved rates (Override →
// catalog), nulls treated as 0. null only when NO rate resolves at all (an
// unpriced Model), so Overall sinks it last like the per-rate columns; a
// cache-estimated Model sums its Input + Output.
export function overallRate(m: ModelPricing): number | null {
  const r = resolvedRates(m);
  if (!r) return null;
  const parts = [r.input, r.output, r.cacheRead, r.cacheWrite].filter((x): x is number => x != null);
  return parts.length ? parts.reduce((a, b) => a + b, 0) : null;
}

// The direction a column opens in when first chosen: price columns open
// highest-price-first ("sort by highest price"); the text columns open A–Z.
export function defaultDir(col: SortCol): SortDir {
  return NUMERIC_COLS.has(col) ? 'desc' : 'asc';
}

// The Rate source column's own value, for sorting: a catalog origin sorts by the
// label that column shows (A–Z), then every Override, then every Unpriced Model.
// Keyed off what the column displays — not the Source that produced the usage,
// which lives under Model — so the visible column really does come out grouped.
// The two states sort as tiers rather than by their badge words, which keeps this
// i18n-free like the rest of the module.
function rateSourceKey(m: ModelPricing): [number, string] {
  const s = modelState(m);
  if (s === 'unpriced') return [2, ''];
  if (s === 'override') return [1, ''];
  return [0, originLabel(m.catalog!.origin)];
}

// A stable, total order for the Pricing table. Price columns read the resolved
// List Price (Override → catalog); a Model with no price for that column — an
// unpriced Model everywhere, or one missing that one rate — always sinks to the
// bottom, both directions, rather than ordering as a real 0. Ties break by Model
// name NEWEST-first, so the order never depends on input order.
export function sortPricing(models: ModelPricing[], col: SortCol, dir: SortDir): ModelPricing[] {
  const sign = dir === 'asc' ? 1 : -1;
  // Model names carry version numbers, so collate them numerically: a plain
  // string compare reads "claude-opus-4-10" as older than "claude-opus-4-9".
  const byName = (a: ModelPricing, b: ModelPricing) =>
    a.model.localeCompare(b.model, undefined, { numeric: true });
  const priceOf = (m: ModelPricing) =>
    col === 'overall' ? overallRate(m) : resolvedRates(m)?.[col as keyof RatesPerTok] ?? null;
  return [...models].sort((a, b) => {
    let primary = 0;
    if (col === 'model') {
      primary = sign * byName(a, b);
    } else if (col === 'source') {
      const [ta, la] = rateSourceKey(a);
      const [tb, lb] = rateSourceKey(b);
      primary = sign * (ta - tb || la.localeCompare(lb));
    } else {
      const ra = priceOf(a);
      const rb = priceOf(b);
      if (ra == null && rb == null) primary = 0;
      else if (ra == null) return 1; // unpriced last, regardless of direction
      else if (rb == null) return -1;
      else primary = sign * (ra - rb);
    }
    // Ties break newest-Model-first: where a whole family shares one price, the
    // opus-5 row belongs above opus-4-8, not buried under its own back
    // catalogue. Direction-independent, like the unpriced tail above — flipping
    // the sorted column must not sink the newest Model of a tied family.
    return primary || -byName(a, b);
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

// Re-exported so the Pricing tab's existing imports stay put; the function is
// shared with the Limits tab and lives in lib/format.ts.
export { fill } from '../lib/format';
