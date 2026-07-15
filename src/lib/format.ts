import { parseLocalDate } from './dateRange';

interface FormatCostOptions {
  adaptivePrecision?: boolean;
  unpricedLabel?: string;
}

// null -> "unpriced"; hasUnpriced -> "≥ $X.XX"; else "$X.XX".
export function formatCost(
  c: number | null,
  hasUnpriced: boolean,
  options: FormatCostOptions = {},
): string {
  if (c === null) return options.unpricedLabel ?? 'unpriced';

  const absoluteCost = Math.abs(c);
  const fractionDigits =
    options.adaptivePrecision && c !== 0 && absoluteCost < 0.01
      ? Math.min(8, Math.max(4, Math.ceil(-Math.log10(absoluteCost)) + 1))
      : 2;
  const rounded = c.toFixed(fractionDigits);
  const amount =
    options.adaptivePrecision && c !== 0 && Number(rounded) === 0
      ? `$${c.toExponential(2)}`
      : new Intl.NumberFormat('en-US', {
          style: 'currency',
          currency: 'USD',
          minimumFractionDigits: fractionDigits,
          maximumFractionDigits: fractionDigits,
        }).format(c);
  return hasUnpriced ? `≥ ${amount}` : amount;
}

const TOKEN_UNITS = [
  { divisor: 1_000_000_000, suffix: 'B' },
  { divisor: 1_000_000, suffix: 'M' },
  { divisor: 1_000, suffix: 'K' },
] as const;

function compactTokenUnit(n: number, rolloverFactor: number) {
  return TOKEN_UNITS.find(({ divisor }) => n >= divisor * rolloverFactor);
}

export function formatCompactTokenTotal(total: number): string {
  const rounded = Math.max(0, Math.round(total));
  const unit = compactTokenUnit(rounded, 0.999995);
  if (!unit) return String(rounded);

  return (
    new Intl.NumberFormat('en-US', { maximumFractionDigits: 2 }).format(
      rounded / unit.divisor,
    ) + unit.suffix
  );
}

export function formatExactTokenTotal(total: number): string {
  return Math.max(0, Math.round(total)).toLocaleString('en-US');
}

// Compact token formatter for the Overview design (K/M/B with adaptive precision).
// Thresholds sit at 999.5 units so a value that toFixed would round up to
// "1000K"/"1000.0M" rolls over to the next unit instead; sub-1000 values are
// rounded (averages can be fractional — never render raw float noise).
export function fmtTok(n: number): string {
  const unit = compactTokenUnit(n, 0.9995);
  if (!unit) return String(Math.round(n));

  const fractionDigits =
    unit.suffix === 'B' ? 2 : unit.suffix === 'M' ? (n >= 1e7 ? 1 : 2) : n >= 1e5 ? 0 : 1;
  return (n / unit.divisor).toFixed(fractionDigits) + unit.suffix;
}
export function fmtPct(x: number): string {
  return (x * 100).toFixed(x < 0.1 ? 1 : 0) + '%';
}
export function fmtDate(d: Date): string {
  return d.toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
}
export function fmtIsoDate(iso: string): string {
  return fmtDate(parseLocalDate(iso));
}
