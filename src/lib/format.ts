// 1234 -> "1,234"; 1_234_567 -> "1.23M"; 1_234_567_890 -> "1.23B".
export function formatTokens(n: number): string {
  if (n >= 1_000_000_000) return (n / 1_000_000_000).toFixed(2) + 'B';
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(2) + 'M';
  return n.toLocaleString('en-US');
}

// null -> "unpriced"; hasUnpriced -> "≥ $X.XX"; else "$X.XX".
export function formatCost(c: number | null, hasUnpriced: boolean): string {
  if (c === null) return 'unpriced';
  const amount = `$${c.toFixed(2)}`;
  return hasUnpriced ? `≥ ${amount}` : amount;
}

// Compact token formatter for the Overview design (K/M/B with adaptive precision).
export function fmtTok(n: number): string {
  if (n >= 1e9) return (n / 1e9).toFixed(2) + 'B';
  if (n >= 1e6) return (n / 1e6).toFixed(n >= 1e7 ? 1 : 2) + 'M';
  if (n >= 1e3) return (n / 1e3).toFixed(n >= 1e5 ? 0 : 1) + 'K';
  return String(n);
}
export function fmtUSD(n: number): string {
  return '$' + n.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}
export function fmtPct(x: number): string {
  return (x * 100).toFixed(x < 0.1 ? 1 : 0) + '%';
}
export function fmtDate(d: Date): string {
  return d.toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
}
export function fmtIsoDate(iso: string): string {
  const [y, m, d] = iso.split('-').map(Number);
  return fmtDate(new Date(y, m - 1, d));
}
