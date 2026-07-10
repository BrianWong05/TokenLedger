import { useState } from 'react';
import { TOOLS, TOTAL_TOKENS, TOTAL_COST, TOOL_TOTALS, contextBreakdown, mockCtxTotals, mockModelBars, fmtTok, fmtUSD, fmtPct, type ToolKey } from './mock';
import ContextBreakdown from './ContextBreakdown';
import ModelsList from './ModelsList';

type Tab = 'context' | 'models';

export default function FocusPanel() {
  const [sel, setSel] = useState<ToolKey>('claude');
  const [tab, setTab] = useState<Tab>('context');

  const tool = TOOLS.find((t) => t.key === sel)!;
  const toolTotal = TOOL_TOTALS[sel];
  const cacheHit = contextBreakdown(sel).cacheHit;

  return (
    <div className="tt-card focus">
      <div className="tt-eyebrow">Total tokens · 2025</div>
      <div className="tt-total">{fmtTok(TOTAL_TOKENS)}</div>
      <div className="tt-cost">{fmtUSD(TOTAL_COST)} est.</div>

      <div className="tt-split">
        {TOOLS.map((t) => (
          <div key={t.key} style={{ width: fmtPct(TOOL_TOTALS[t.key] / TOTAL_TOKENS), background: t.color }} />
        ))}
      </div>

      <div className="tt-chips">
        {TOOLS.map((t) => {
          const active = t.key === sel;
          return (
            <button
              key={t.key}
              className="tt-chip"
              onClick={() => setSel(t.key)}
              style={active ? { borderColor: t.color, background: t.color + '22', color: '#f3f6fc' } : undefined}
            >
              <span className="dot" style={{ background: t.color }} />
              {t.label} <span className="pct">{fmtPct(TOOL_TOTALS[t.key] / TOTAL_TOKENS)}</span>
            </button>
          );
        })}
      </div>

      <div className="tt-tabs">
        <div className="tt-seg">
          <button className={tab === 'context' ? 'active' : ''} onClick={() => setTab('context')}>
            Context
          </button>
          <button className={tab === 'models' ? 'active' : ''} onClick={() => setTab('models')}>
            Models
          </button>
        </div>
        <span className="tt-badge">cache hit {fmtPct(cacheHit)}</span>
      </div>

      {tab === 'context' ? (
        <ContextBreakdown tool={tool} {...mockCtxTotals(sel)} />
      ) : (
        <ModelsList tool={tool} toolTokens={toolTotal} models={mockModelBars(sel, toolTotal)} showCost={false} />
      )}
    </div>
  );
}
