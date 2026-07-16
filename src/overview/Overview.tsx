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
import { fmtPct } from '../lib/format';
import { formatDisplayCost, RANGE_LABEL_KEY, useOverviewT } from './localize';
import { useSettings } from '../settings/SettingsContext';
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
  const { settings } = useSettings();
  const { t, lang } = useOverviewT();
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
    refreshSec, setRefreshSec,
    range, setRange,
    from, to, firstIso, lastIso, customFrom, customTo, setCustomRange,
    sel, setSel,
    rangeLabel, tool, grand, toolTotals, visibleTools,
    summary, modelRows, canOpenCostBreakdown, headline,
    panels,
  } = useOverview(ports);

  const headlineCost = (
    <>
      {summary ? formatDisplayCost(summary.cost, summary.hasUnpriced, settings, lang) : '…'} {t('overview.est')}
      {summary?.hasUnpriced && (
        <span title={summary.unpricedModels.join(', ')}> · {summary.unpricedModels.length} {t('overview.unpricedMarker')}</span>
      )}
      {summary && summary.cacheEstimatedModels.length > 0 && (
        <span title={summary.cacheEstimatedModels.join(', ')}>
          {' '}· {summary.cacheEstimatedModels.length} {t('overview.cacheEst')}
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
                  {t(RANGE_LABEL_KEY[r.key])}
                </button>
              ))}
            </div>
            <div className="tt-refresh">
              <div className="tt-select-wrap">
                <select
                  className="tt-select"
                  aria-label={t('overview.autoRefresh')}
                  value={refreshSec}
                  onChange={(e) => setRefreshSec(Number(e.target.value) as RefreshSec)}
                >
                  {REFRESH_PRESETS.map((p) => (
                    <option key={p.sec} value={p.sec}>
                      {p.sec === 0 ? t('overview.off') : p.label}
                    </option>
                  ))}
                </select>
                <i>▼</i>
              </div>
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
            <span className="lbl">{t('overview.customRange')}</span>
            <input
              type="date"
              value={from}
              min={firstIso}
              max={to}
              onChange={(e) => e.target.value && setCustomRange(e.target.value, customTo)}
            />
            <span className="to">{t('overview.to')}</span>
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
            <div className="tt-eyebrow">{t('overview.totalTokens')} · {rangeLabel}</div>
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
                title={t('overview.showCostBreakdown')}
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
            {TOOLS.map((tl) => (
              <div key={tl.key} style={{ width: fmtPct(toolTotals[tl.key] / grand), background: tl.color }} />
            ))}
          </div>

          <div className="tt-toolcards">
            {visibleTools.map((tl) => {
              const active = tl.key === sel;
              return (
                <button
                  key={tl.key}
                  className={'tt-toolcard' + (active ? ' active' : '')}
                  onClick={() => setSel(tl.key)}
                  style={active ? { borderColor: tl.color, background: tl.color + '1e' } : undefined}
                >
                  <div className="lbl">
                    <ToolIcon tool={tl} />
                    {tl.label}
                  </div>
                  <div className="num">{fmtPct(toolTotals[tl.key] / grand)}</div>
                  {tl.nModels > 0 && (
                    <div className="sub">
                      {tl.nModels} {t(tl.nModels === 1 ? 'overview.modelOne' : 'overview.modelMany')}
                    </div>
                  )}
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
                  settings={settings}
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
