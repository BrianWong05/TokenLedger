import type { Summary } from '../types';
import { formatTokens, formatCost } from '../lib/format';

export interface HeroCardProps {
  summary: Summary | null;
}

export default function HeroCard({ summary }: HeroCardProps) {
  if (!summary) {
    return (
      <div className="hero-card">
        <div className="hero-label">Total tokens</div>
        <div className="hero-number">—</div>
      </div>
    );
  }

  const costText =
    summary.cost !== null && summary.hasUnpriced
      ? `${formatCost(summary.cost, true)} · ${summary.unpricedModels.length} unpriced models`
      : formatCost(summary.cost, summary.hasUnpriced);

  return (
    <div className="hero-card">
      <div className="hero-label">Total tokens</div>
      <div className="hero-number">{formatTokens(summary.totalTokens)}</div>
      <div className="hero-meta">
        <span className="hero-requests">{formatTokens(summary.requests)} requests</span>
        <span className="hero-cost">
          <span className="hero-cost-label">Est. cost</span>
          <span className="hero-cost-value">{costText}</span>
          <span className="hero-cost-sub">at API list prices — not billed</span>
        </span>
      </div>
    </div>
  );
}
