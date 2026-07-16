import { CATEGORIES, type ToolMeta } from './meta';
import type { ModelBar } from './data';
import { fmtTok, fmtPct, formatCost } from '../lib/format';

// Per-model token breakdown for one source. Each bar's filled width is the
// model's share of the source; inner segments are the four token categories.
export default function ModelsList({
  tool,
  toolTokens,
  models,
  showCost = true,
  onModelClick,
}: {
  tool: ToolMeta;
  toolTokens: number;
  models: ModelBar[];
  showCost?: boolean;
  // When set, a Model row is a button that opens the Override editor in place
  // (the Pricing fix reachable where the "unpriced" symptom shows).
  onModelClick?: (model: string) => void;
}) {
  return (
    <>
      <div className="tt-models-head">
        <div className="lbl">
          <span className="dot" style={{ background: tool.color }} />
          Models <span className="count">· {models.length}</span>
        </div>
        <span className="tot">{fmtTok(toolTokens)}</span>
      </div>
      {models.map((m) => (
        <div
          className="tt-model"
          key={m.name}
          role={onModelClick ? 'button' : undefined}
          tabIndex={onModelClick ? 0 : undefined}
          style={onModelClick ? { cursor: 'pointer' } : undefined}
          onClick={onModelClick ? () => onModelClick(m.name) : undefined}
          onKeyDown={
            onModelClick
              ? (e) => {
                  if (e.key === 'Enter' || e.key === ' ') {
                    e.preventDefault();
                    onModelClick(m.name);
                  }
                }
              : undefined
          }
        >
          <div className="top">
            <span className="name">
              {m.name}
              {m.cacheEstimated && <span className="tt-tag">cache est.</span>}
            </span>
            <span className="figs">
              <span className="tok">{fmtTok(m.tokens)}</span>
              {showCost && <span className="cost">{formatCost(m.cost, false)}</span>}
              <span className="pct">{fmtPct(m.share)}</span>
            </span>
          </div>
          <div className="track">
            <div className="segs" style={{ width: m.share * 100 + '%' }}>
              {m.segs.map((c) => (
                <div key={c.key} style={{ width: c.frac * 100 + '%', background: c.color }} />
              ))}
            </div>
          </div>
        </div>
      ))}
      <div className="tt-legend" style={{ marginTop: 12, paddingTop: 12, borderTop: '1px solid rgba(255,255,255,.06)' }}>
        {CATEGORIES.map((c) => (
          <span className="item" key={c.key}>
            <span className="sw" style={{ background: c.color }} />
            {c.label}
          </span>
        ))}
      </div>
    </>
  );
}
