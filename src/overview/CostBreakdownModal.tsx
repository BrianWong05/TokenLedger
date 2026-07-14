import { useMemo } from 'react';
import type { BreakdownRow, Summary } from '../types';
import { buildCostBreakdown, formatBreakdownCost, formatSourceCost } from './costBreakdown';

interface CostBreakdownModalProps {
  summary: Summary;
  rows: BreakdownRow[];
  onClose: () => void;
}

export default function CostBreakdownModal({ summary, rows, onClose }: CostBreakdownModalProps) {
  const groups = useMemo(() => buildCostBreakdown(rows), [rows]);

  return (
    <div className="tt-cost-modal-backdrop">
      <section
        className="tt-cost-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="tt-cost-modal-title"
      >
        <header className="tt-cost-modal-head">
          <div>
            <div className="tt-cost-modal-eyebrow" id="tt-cost-modal-title">
              Estimated total Cost
            </div>
            <div className="tt-cost-modal-total">
              {formatBreakdownCost(summary.cost, summary.hasUnpriced && summary.cost !== null)}
            </div>
            {summary.hasUnpriced && (
              <div className="tt-cost-modal-note">
                Partial Cost · {summary.unpricedModels.length} Unpriced Model
                {summary.unpricedModels.length === 1 ? '' : 's'}
              </div>
            )}
          </div>
          <button type="button" className="tt-cost-modal-close" onClick={onClose} aria-label="Close Cost breakdown">
            ×
          </button>
        </header>

        <div className="tt-cost-modal-scroll">
          <div className="tt-cost-modal-columns" aria-hidden="true">
            <span>Model</span>
            <span>Cost</span>
          </div>

          {groups.map((group) => (
            <section className="tt-cost-group" key={group.sourceKey}>
              <div className="tt-cost-group-head">
                <span>{group.sourceName}</span>
                <span>{formatSourceCost(group.cost, group.unpricedCount)}</span>
              </div>
              {group.models.map((model) => (
                <div className="tt-cost-model" key={model.name}>
                  <span className="tt-cost-model-name">
                    {model.name}
                    {model.cacheEstimated && <span className="tt-tag">cache est.</span>}
                  </span>
                  <span className={model.cost === null ? 'unpriced' : ''}>
                    {formatBreakdownCost(model.cost)}
                  </span>
                </div>
              ))}
            </section>
          ))}
        </div>
      </section>
    </div>
  );
}
