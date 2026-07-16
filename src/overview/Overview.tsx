import { useCallback, useRef, useState } from 'react';
import './overview.css';
import Heatmap from './Heatmap';
import ContextBreakdown from './ContextBreakdown';
import TokenBreakdown from './TokenBreakdown';
import ModelsList from './ModelsList';
import BreakdownTable from './BreakdownTable';
import CostBreakdownModal from './CostBreakdownModal';
import TokenTotalHeadline from './TokenTotalHeadline';
import AggTrend from './AggTrend';
import SmallMultiples from './SmallMultiples';
import { TOOLS, RANGES_8B, type ToolMeta } from './meta';
import { TOOL_ICONS } from './icons';
import { fmtPct, formatCost } from '../lib/format';
import { REFRESH_PRESETS, type RefreshSec } from './useAutoRefresh';
import { useOverview } from './useOverview';
import type { LedgerPort } from './ledger';
import type { ClockPort } from './overviewStore';
import { tauriPricing, type PricingPort } from '../pricing/pricing';
import type { SettingsPort } from '../settings/settings';
import OverrideEditor from '../pricing/OverrideEditor';
import type { ModelPricing } from '../types';

// "App · Overview", wired to the real Ledger through useOverview(): one
// unbounded daily series powers heatmap/trends/tables via client-side slicing;
// summary and breakdowns re-fetch per range; an hourly series serves the Day
// view. All data derivation lives in the store/selectors — this shell only
// renders the model the hook hands back. The window header (wordmark, tab nav,
// Rescan) is owned by the app shell; this tab renders only its own toolbar.
export default function Overview({ ports }: { ports?: { ledger?: LedgerPort; clock?: ClockPort; pricing?: PricingPort; settings?: SettingsPort } } = {}) {
  const [costBreakdownOpen, setCostBreakdownOpen] = useState(false);

  // Model-selection entry point into the shared Override editor: fetch a fresh
  // ModelPricing list on open (the Overview may show a Model absent from a stale
  // list, so we always re-list), then open the same editor the Pricing tab uses.
  const pricing = ports?.pricing ?? tauriPricing;
  const [pricingEditor, setPricingEditor] = useState<ModelPricing | null>(null);
  const openPricing = useCallback(
    (name: string, toolKey: string) => {
      pricing.list()
        .then((list) => setPricingEditor(list.find((m) => m.model === name) ?? { model: name, tool: toolKey, overrideRates: null, catalog: null }))
        .catch(() => setPricingEditor({ model: name, tool: toolKey, overrideRates: null, catalog: null }));
    },
    [pricing],
  );
  const costBreakdownFocusTargetRef = useRef<HTMLElement | null>(null);
  const setCostBreakdownFocusTarget = useCallback((element: HTMLElement | null) => {
    costBreakdownFocusTargetRef.current = element;
  }, []);

  const {
    loading, scanError, fetchError,
    refresh, refreshing, refreshSec, setRefreshSec,
    range, setRange,
    from, to, firstIso, lastIso, customFrom, customTo, setCustomRange,
    sel, setSel,
    rangeLabel, tool, grand, toolTotals, visibleTools,
    summary, modelRows, canOpenCostBreakdown, headline,
    panels,
  } = useOverview(ports);

  const headlineCost = (
    <>
      {summary ? formatCost(summary.cost, summary.hasUnpriced) : '…'} est.
      {summary?.hasUnpriced && (
        <span title={summary.unpricedModels.join(', ')}> · {summary.unpricedModels.length} unpriced</span>
      )}
      {summary && summary.cacheEstimatedModels.length > 0 && (
        <span title={summary.cacheEstimatedModels.join(', ')}>
          {' '}· {summary.cacheEstimatedModels.length} cache est.
        </span>
      )}
    </>
  );

  return (
    <div className="tt">
      <div className={'tt-app' + (loading ? ' tt-loading' : '')}>
        <div className="tt-top tt-top-toolbar">
          <div className="tt-top-right">
            <div className="tt-seg">
              {RANGES_8B.map((r) => (
                <button key={r.key} className={range === r.key ? 'active' : ''} onClick={() => setRange(r.key)}>
                  {r.label}
                </button>
              ))}
            </div>
            <div className="tt-refresh">
              <div className="tt-select-wrap">
                <select
                  className="tt-select"
                  aria-label="Auto-refresh interval"
                  value={refreshSec}
                  onChange={(e) => setRefreshSec(Number(e.target.value) as RefreshSec)}
                >
                  {REFRESH_PRESETS.map((p) => (
                    <option key={p.sec} value={p.sec}>
                      {p.sec === 0 ? 'Off' : p.label}
                    </option>
                  ))}
                </select>
                <i>▼</i>
              </div>
              <button
                type="button"
                className={'tt-refresh-btn' + (refreshing ? ' spinning' : '')}
                onClick={() => void refresh()}
                disabled={refreshing}
                aria-label="Refresh"
                aria-busy={refreshing}
                title="Refresh"
              >
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                  <path
                    d="M21 12a9 9 0 1 1-2.64-6.36"
                    stroke="currentColor"
                    strokeWidth="2"
                    strokeLinecap="round"
                  />
                  <path
                    d="M21 3v6h-6"
                    stroke="currentColor"
                    strokeWidth="2"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                  />
                </svg>
              </button>
            </div>
            <span className="tt-avatar">BW</span>
          </div>
        </div>

        {(scanError || fetchError) && (
          <div className="tt-error">
            {scanError && fetchError ? `${scanError} · ${fetchError}` : scanError || fetchError}
          </div>
        )}

        {range === 'custom' && (
          <div className="tt-custom-row">
            <span className="lbl">Custom range</span>
            <input
              type="date"
              value={from}
              min={firstIso}
              max={to}
              onChange={(e) => e.target.value && setCustomRange(e.target.value, customTo)}
            />
            <span className="to">to</span>
            <input
              type="date"
              value={to}
              min={from}
              max={lastIso}
              onChange={(e) => e.target.value && setCustomRange(customFrom, e.target.value)}
            />
          </div>
        )}

        <div className="tt-b8-body">
          <div className="tt-b8-head">
            <div className="tt-eyebrow">Total tokens · {rangeLabel}</div>
            <TokenTotalHeadline
              total={headline.total}
              summaryReady={headline.summaryReady}
            />
            {canOpenCostBreakdown ? (
              <button
                ref={setCostBreakdownFocusTarget}
                type="button"
                className="tt-b8-cost tt-b8-cost-button"
                onClick={() => setCostBreakdownOpen(true)}
                aria-haspopup="dialog"
                title="Show Cost breakdown"
              >
                {headlineCost}
              </button>
            ) : (
              <div ref={setCostBreakdownFocusTarget} className="tt-b8-cost" tabIndex={-1}>
                {headlineCost}
              </div>
            )}
          </div>

          <div className="tt-split">
            {TOOLS.map((t) => (
              <div key={t.key} style={{ width: fmtPct(toolTotals[t.key] / grand), background: t.color }} />
            ))}
          </div>

          <div className="tt-toolcards">
            {visibleTools.map((t) => {
              const active = t.key === sel;
              return (
                <button
                  key={t.key}
                  className={'tt-toolcard' + (active ? ' active' : '')}
                  onClick={() => setSel(t.key)}
                  style={active ? { borderColor: t.color, background: t.color + '1e' } : undefined}
                >
                  <div className="lbl">
                    <ToolIcon tool={t} />
                    {t.label}
                  </div>
                  <div className="num">{fmtPct(toolTotals[t.key] / grand)}</div>
                  {t.nModels > 0 && <div className="sub">{t.nModels} model{t.nModels === 1 ? '' : 's'}</div>}
                </button>
              );
            })}
          </div>

          <div className="tt-b8-grid">
            <div className="tt-b8-col">
              <Heatmap days={panels.heatmap.days} compact />
              <AggTrend data={panels.trend.data} per={panels.trend.per} rangeLabel={rangeLabel} modelTool={panels.trend.modelTool} />
              <SmallMultiples items={panels.sparks} rangeLabel={rangeLabel} />
            </div>

            <div className="tt-b8-col">
              <div>
                <ContextBreakdown
                  tool={panels.context.tool}
                  ctx={panels.context.ctx}
                  view={panels.context.view}
                  tree={panels.context.tree}
                  execRows={panels.context.execRows}
                  meta={panels.context.meta}
                />
              </div>
              <div>
                <TokenBreakdown tool={tool} cats={panels.tokens.cats} />
              </div>
              <div>
                <ModelsList
                  tool={tool}
                  toolTokens={panels.models.toolTokens}
                  models={panels.models.models}
                  onModelClick={(name) => openPricing(name, tool.key)}
                />
              </div>
            </div>
          </div>

          <BreakdownTable dailyRows={panels.table.dailyRows} projectRows={panels.table.projectRows} />
        </div>
      </div>
      {costBreakdownOpen && summary && (
        <CostBreakdownModal
          summary={summary}
          rows={modelRows}
          returnFocusRef={costBreakdownFocusTargetRef}
          onClose={() => setCostBreakdownOpen(false)}
        />
      )}
      {pricingEditor && (
        <OverrideEditor
          model={pricingEditor}
          pricing={pricing}
          settings={ports?.settings}
          onClose={() => setPricingEditor(null)}
        />
      )}
    </div>
  );
}

// Brand-icon chip for a source; falls back to a colored monogram when the tool
// has no brand mark (e.g. Hermes).
function ToolIcon({ tool }: { tool: ToolMeta }) {
  const src = TOOL_ICONS[tool.key];
  return (
    <span className="tt-toolicon">
      {src ? (
        <img src={src} alt="" width={22} height={22} />
      ) : (
        <b style={{ color: tool.color }}>{tool.label[0]}</b>
      )}
    </span>
  );
}
