import { memo } from 'react';
import { CATEGORIES, type ToolMeta } from './meta';
import type { ModelBar } from './data';
import { fmtTok, fmtPct } from '../lib/format';
import { CAT_KEY, formatDisplayCost, overviewT, useOverviewT, USD_IDENTITY } from './localize';
import type { Settings } from '../types';

// Per-model token breakdown for one source. Each bar's filled width is the
// model's share of the source; inner segments are the four token categories.
// Costs render in the Display Currency; settings arrive as a prop (rather than
// via useSettings) so the Pricing-entry test can mount this component bare.
function ModelsList({
  tool,
  toolTokens,
  models,
  showCost = true,
  onModelClick,
  settings = USD_IDENTITY,
}: {
  tool: ToolMeta;
  toolTokens: number;
  models: ModelBar[];
  showCost?: boolean;
  // When set, a Model row is a button that opens the Override editor in place
  // (the Pricing fix reachable where the "unpriced" symptom shows).
  onModelClick?: (model: string) => void;
  settings?: Pick<Settings, 'currency' | 'usdRate'>;
}) {
  const { t, lang } = useOverviewT();
  return (
    <>
      <div className="tt-models-head">
        <div className="lbl">
          <span className="dot" style={{ background: tool.color }} />
          {t('overview.modelsHead')} <span className="count">· {models.filter((m) => m.name !== null).length}</span>
        </div>
        <span className="tot">{fmtTok(toolTokens)}</span>
      </div>
      {models.map((m) => {
        const modelName = m.name;
        const unattributed = modelName === null;
        const activate = modelName === null || onModelClick === undefined
          ? undefined
          : () => onModelClick(modelName);
        const name = unattributed ? t('overview.unattributedUsage') : modelName;
        return (
          <div
            className="tt-model"
            key={m.name ?? 'unattributed-usage'}
            role={activate ? 'button' : undefined}
            tabIndex={activate ? 0 : undefined}
            style={activate ? { cursor: 'pointer' } : undefined}
            onClick={activate}
            onKeyDown={
              activate
                ? (e) => {
                    if (e.key === 'Enter' || e.key === ' ') {
                      e.preventDefault();
                      activate();
                    }
                  }
                : undefined
            }
          >
            <div className="top">
              <span className="name">
                {name}
                {m.cacheEstimated && <span className="tt-tag">{t('overview.cacheEst')}</span>}
              </span>
              <span className="figs">
                <span className="tok">{fmtTok(m.tokens)}</span>
                {showCost && (
                  <span className="cost">
                    {unattributed
                      ? overviewT(lang, 'overview.unavailableCost')
                      : formatDisplayCost(m.cost, false, settings, lang)}
                  </span>
                )}
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
        );
      })}
      <div className="tt-legend" style={{ marginTop: 12, paddingTop: 12, borderTop: '1px solid var(--border-subtle)' }}>
        {CATEGORIES.map((c) => (
          <span className="item" key={c.key}>
            <span className="sw" style={{ background: c.color }} />
            {t(CAT_KEY[c.key])}
          </span>
        ))}
      </div>
    </>
  );
}

// Memoized: props stay identity-stable across the shell's per-tick re-renders.
export default memo(ModelsList);
