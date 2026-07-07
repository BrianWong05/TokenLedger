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
