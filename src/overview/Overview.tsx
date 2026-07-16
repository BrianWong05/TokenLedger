import { Fragment, useCallback, useRef, useState } from 'react';
import './overview.css';
import Heatmap from './Heatmap';
import ContextBreakdown from './ContextBreakdown';
import ModelsList from './ModelsList';
import BreakdownTable from './BreakdownTable';
import CostBreakdownModal from './CostBreakdownModal';
import TokenTotalHeadline from './TokenTotalHeadline';
import AggTrend from './AggTrend';
import SmallMultiples from './SmallMultiples';
import { TOOLS, RANGES_8B, type ToolMeta } from './meta';
import { REFRESH_PRESETS, type RefreshSec } from './useAutoRefresh';
import { TOOL_ICONS } from './icons';
import { fmtPct } from '../lib/format';
import { formatDisplayCost, RANGE_LABEL_KEY, useOverviewT } from './localize';
import { useSettings } from '../settings/SettingsContext';
import { useOverview } from './useOverview';
import type { LedgerPort } from './ledger';
import type { ClockPort } from './overviewStore';
import { tauriPricing, type PricingPort } from '../pricing/pricing';
import type { SettingsPort } from '../settings/settings';
import OverrideEditor from '../pricing/OverrideEditor';
import type { ModelPricing } from '../types';

// "App · Overview", rebuilt to the dashboard-v2 design and wired to the real
// Ledger through useOverview(): one unbounded daily series powers
// heatmap/trends/tables via client-side slicing; summary and breakdowns re-fetch
// per range; an hourly series serves the Day view. All data derivation lives in
// the store/selectors — this shell only renders the model the hook hands back.
// The window chrome (sidebar wordmark, tab nav, Rescan) is owned by the app
// shell; this tab renders only the design's <main> content, flush on --bg-app.
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
    loading, scanError, fetchError, scanSources,
    refreshSec, setRefreshSec,
    range, setRange,
    from, to, firstIso, lastIso, customFrom, customTo, setCustomRange,
    sel, setSel,
    rangeLabel, tool, grand, toolTotals, visibleTools,
    summary, modelRows, canOpenCostBreakdown, headline,
    panels,
  } = useOverview(ports);

  // Cost routes through the Display Currency; the ≥ (Partial Cost) prefix and the
  // unpriced / cache-estimated markers stay verbatim. "est. at list prices — not
  // billed" is the design's secondary descriptor.
  const headlineCost = (
    <>
      {summary ? formatDisplayCost(summary.cost, summary.hasUnpriced, settings, lang) : '…'}
      <span className="tt-b8-cost-note" title={t('overview.notBilled')}> {t('overview.costNote')}</span>
      {summary?.hasUnpriced && (
        <span className="tt-b8-cost-mark" title={summary.unpricedModels.join(', ')}> · {summary.unpricedModels.length} {t('overview.unpricedMarker')}</span>
      )}
      {summary && summary.cacheEstimatedModels.length > 0 && (
        <span className="tt-b8-cost-mark" title={summary.cacheEstimatedModels.join(', ')}> · {summary.cacheEstimatedModels.length} {t('overview.cacheEst')}</span>
      )}
    </>
  );

  return (
    <div className={'tt' + (loading ? ' tt-loading' : '')}>
      <div className="tt-toolbar">
        <div className="tt-seg">
          {RANGES_8B.map((r) => (
            <button key={r.key} className={range === r.key ? 'active' : ''} onClick={() => setRange(r.key)}>
              {t(RANGE_LABEL_KEY[r.key])}
            </button>
          ))}
        </div>
        <span className="tt-refresh">
          <select
            aria-label={t('overview.autoRefresh')}
            value={refreshSec}
            onChange={(e) => setRefreshSec(Number(e.target.value) as RefreshSec)}
          >
            {REFRESH_PRESETS.map((p) => (
              <option key={p.sec} value={p.sec}>
                {p.label}
              </option>
            ))}
          </select>
          <span className="chev" aria-hidden="true">▾</span>
        </span>
        <span className="tt-avatar" aria-hidden="true">BW</span>
      </div>

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

      <div className="tt-body">
      {(scanError || fetchError) && (
        <div className="tt-error">
          {scanError && fetchError ? `${scanError} · ${fetchError}` : scanError || fetchError}
        </div>
      )}

      {/* HERO: totals + proportion bar + per-source cards */}
      <div className="tt-hero">
        <div className="tt-hero-head">
          <div className="tt-eyebrow">{t('overview.totalTokens')} · {rangeLabel}</div>
          <TokenTotalHeadline total={headline.total} summaryReady={headline.summaryReady} />
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
                style={active ? { borderColor: tl.color, background: tl.color + '14' } : undefined}
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
      </div>

      <div className="tt-b8-grid">
        <div className="tt-b8-col">
          <Heatmap days={panels.heatmap.days} compact />
          <AggTrend data={panels.trend.data} per={panels.trend.per} rangeLabel={rangeLabel} modelTool={panels.trend.modelTool} />
          <SmallMultiples items={panels.sparks} rangeLabel={rangeLabel} />
        </div>

        <div className="tt-b8-col">
          <div className="tt-card">
            <ContextBreakdown
              tool={panels.context.tool}
              ctx={panels.context.ctx}
              view={panels.context.view}
              tree={panels.context.tree}
              execRows={panels.context.execRows}
              meta={panels.context.meta}
            />
          </div>
          <div className="tt-card">
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

      {scanSources.length > 0 && (
        <div className="tt-scan-foot">
          {scanSources.map((s, i) => (
            <Fragment key={s.source}>
              {i > 0 && <span className="sep">·</span>}
              <span>
                {s.source}: {s.eventsInserted} {t('overview.scanIn')} / {s.linesSkipped} {t('overview.scanSkipped')}
              </span>
            </Fragment>
          ))}
        </div>
      )}
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
// has no brand mark.
function ToolIcon({ tool }: { tool: ToolMeta }) {
  const src = TOOL_ICONS[tool.key];
  return (
    <span className="tt-toolicon">
      {src ? (
        <img src={src} alt="" width={13} height={13} />
      ) : (
        <b style={{ color: tool.color }}>{tool.label[0]}</b>
      )}
    </span>
  );
}
