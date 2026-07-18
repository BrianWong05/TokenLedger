import { Fragment, useCallback, useRef, useState } from 'react';
import './overview.css';
import Heatmap from './Heatmap';
import HeatmapModal from './HeatmapModal';
import ContextBreakdown from './ContextBreakdown';
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
import { useT } from '../lib/i18n';
import { useSettings } from '../settings/SettingsContext';
import { useOverview } from './useOverview';
import { tauriLedger, type LedgerPort } from './ledger';
import type { ClockPort } from './overviewStore';
import { heatFilters } from './data';
import { tauriPricing, type PricingPort } from '../pricing/pricing';
import type { SettingsPort } from '../settings/settings';
import OverrideEditor from '../pricing/OverrideEditor';
import type { ModelPricing, Summary } from '../types';

// "App · Overview", rebuilt to the dashboard-v2 design and wired to the real
// Ledger through useOverview(): one unbounded daily series powers
// heatmap/trends/tables via client-side slicing; summary and breakdowns re-fetch
// per range; an hourly series serves the Day view. All data derivation lives in
// the store/selectors — this shell only renders the model the hook hands back,
// plus two on-open fetches it owns directly: the Pricing list for the Override
// editor and the year-window Summary for the Activity enlarge.
// The window chrome (sidebar wordmark, tab nav) is owned by the app shell; the
// last-scan status + Rescan live in this tab's toolbar (dashboard-v2). This tab
// renders the design's <main> content, flush on --bg-app.
export default function Overview({ ports }: { ports?: { ledger?: LedgerPort; clock?: ClockPort; pricing?: PricingPort; settings?: SettingsPort } } = {}) {
  const { settings } = useSettings();
  const { t, lang } = useOverviewT();
  // header.* strings (Rescan, last-scan status) live in the shared shell dictionary.
  const { t: tShell } = useT();
  const [costBreakdownOpen, setCostBreakdownOpen] = useState(false);
  const [heatModalOpen, setHeatModalOpen] = useState(false);
  const heatEnlargeRef = useRef<HTMLElement | null>(null);
  const setHeatEnlargeTarget = useCallback((el: HTMLButtonElement | null) => {
    heatEnlargeRef.current = el;
  }, []);

  // The enlarge's Cost describes exactly its trailing-365-day window, so it
  // gets its own Summary fetched per open (the page Summary is range-scoped
  // and would mis-mark Partial Cost). null renders as a placeholder; a failed
  // fetch just leaves it. The epoch guards a quick close→reopen: only the
  // latest open's response may land, so a stale fetch can't replace the
  // placeholder or outlive a newer answer.
  const ledger = ports?.ledger ?? tauriLedger;
  const [heatSummary, setHeatSummary] = useState<Summary | null>(null);
  const heatFetchEpoch = useRef(0);
  const openHeatModal = useCallback(() => {
    setHeatSummary(null);
    setHeatModalOpen(true);
    const epoch = ++heatFetchEpoch.current;
    ledger.summary(heatFilters()).then(
      (s) => {
        if (heatFetchEpoch.current === epoch) setHeatSummary(s);
      },
      () => {},
    );
  }, [ledger]);

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
    refresh, refreshing, scanAt,
    range, setRange,
    from, to, firstIso, lastIso, customFrom, customTo, setCustomRange,
    sel, setSel,
    rangeLabel, tool, grand, toolTotals, visibleTools,
    summary, modelRows, canOpenCostBreakdown, headline,
    panels,
  } = useOverview(ports);

  const scanLabel = refreshing
    ? tShell('header.scanning')
    : scanAt
      ? `${tShell('header.lastScan')} · ${new Date(scanAt).toLocaleTimeString()}`
      : tShell('header.notScanned');

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
      {/* the toolbar's empty stretch is a window-drag handle (frameless window);
          mousedown on the child controls does not start a drag */}
      <div className="tt-toolbar" data-tauri-drag-region>
        <div className="tt-seg">
          {RANGES_8B.map((r) => (
            <button key={r.key} className={range === r.key ? 'active' : ''} onClick={() => setRange(r.key)}>
              {t(RANGE_LABEL_KEY[r.key])}
            </button>
          ))}
        </div>
        <span className="tt-lastscan">{scanLabel}</span>
        <button
          type="button"
          className="tt-rescan"
          onClick={() => void refresh()}
          disabled={refreshing}
          aria-busy={refreshing}
        >
          <svg className="tt-rescan-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8" />
            <path d="M21 3v5h-5" />
            <path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16" />
            <path d="M8 16H3v5" />
          </svg>
          {tShell('header.rescan')}
        </button>
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
          <Heatmap days={panels.heatmap.days} compact onEnlarge={openHeatModal} enlargeRef={setHeatEnlargeTarget} />
          <AggTrend data={panels.trend.data} per={panels.trend.per} rangeLabel={rangeLabel} modelTool={panels.trend.modelTool} />
          {panels.sparks.length > 0 && <SmallMultiples items={panels.sparks} rangeLabel={rangeLabel} />}
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
      {heatModalOpen && (
        <HeatmapModal
          days={panels.heatmap.days}
          summary={heatSummary}
          returnFocusRef={heatEnlargeRef}
          onClose={() => setHeatModalOpen(false)}
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
        <img src={src} alt="" width={20} height={20} />
      ) : (
        <b style={{ color: tool.color }}>{tool.label[0]}</b>
      )}
    </span>
  );
}
