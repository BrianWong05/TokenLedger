import { Fragment, useCallback, useEffect, useRef, useState } from 'react';
import './overview.css';
import Heatmap from './Heatmap';
import HeatmapModal from './HeatmapModal';
import ContextBreakdown from './ContextBreakdown';
import ModelsList from './ModelsList';
import Profile from './Profile';

import BreakdownTable from './BreakdownTable';
import CostBreakdownModal from './CostBreakdownModal';
import TokenTotalHeadline from './TokenTotalHeadline';
import AggTrend from './AggTrend';
import TrendModal from './TrendModal';
import SmallMultiples from './SmallMultiples';
import { RangeSegments } from './RangePicker';
import type { SourceMeta } from './meta';
import { sourceIcon } from './icons';
import { fmtPct, formatCompactTokenTotal } from '../lib/format';
import { countLabel, formatSummaryCost, unreadableReasons, useOverviewT } from './localize';
import { unreadableSourcesIn } from '../lib/tokenCompleteness';
import { hotkeyHint, isHotkey } from '../lib/hotkeys';
import { detectPlatform, type Platform } from '../lib/platform';
import { useT } from '../lib/i18n';
import { useSettings } from '../settings/SettingsContext';
import { useOverview } from './useOverview';
import { tauriLedger, type LedgerPort } from './ledger';
import { tauriExport, type ExportPort } from './export';
import { reportFilename, windowReportCsv } from './reportCsv';
import type { ClockPort } from './overviewStore';
import { heatFilters } from './data';
import { tauriPricing, type PricingPort } from '../pricing/pricing';
import type { SettingsPort } from '../settings/settings';
import OverrideEditor from '../pricing/OverrideEditor';
import type { ModelPricing, Summary } from '../types';

// "App · Overview", rebuilt to the dashboard-v2 design and wired to the real
// Ledger through useOverview(): one unbounded daily series powers
// heatmap/trends/tables via client-side slicing; summary and breakdowns re-fetch
// per range; an hourly series serves any single-day window. All data derivation lives in
// the store/selectors — this shell only renders the model the hook hands back,
// plus two on-open fetches it owns directly: the Pricing list for the Override
// editor and the year-window Summary for the Activity enlarge.
// The window chrome (sidebar wordmark, tab nav) is owned by the app shell; the
// last-scan status + Rescan live in this tab's toolbar (dashboard-v2). This tab
// renders the design's <main> content, flush on --bg-app.
// `visible` is false while another tab shows: the Overview stays mounted so its
// data survives, so it is told rather than able to observe it. Only the headline
// reads it (a tab return rolls the total, #94); default true keeps every other
// mount — tests, prototypes — behaving as if nothing hides it.
export default function Overview({ ports, visible = true, platform = detectPlatform() }: { ports?: { ledger?: LedgerPort; clock?: ClockPort; pricing?: PricingPort; settings?: SettingsPort; export?: ExportPort }; visible?: boolean; platform?: Platform } = {}) {
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
  const [trendModalOpen, setTrendModalOpen] = useState(false);
  const trendEnlargeRef = useRef<HTMLElement | null>(null);
  const setTrendEnlargeTarget = useCallback((el: HTMLButtonElement | null) => {
    trendEnlargeRef.current = el;
  }, []);
  const openTrendModal = useCallback(() => setTrendModalOpen(true), []);

  // The enlarge's Cost describes exactly its trailing-365-day window, so it
  // gets its own Summary fetched per open (the page Summary is range-scoped
  // and would mis-mark Partial Cost). null renders as a placeholder; a failed
  // fetch just leaves it. The epoch guards a quick close→reopen: only the
  // latest open's response may land, so a stale fetch can't replace the
  // placeholder or outlive a newer answer.
  const ledger = ports?.ledger ?? tauriLedger;
  const exporter = ports?.export ?? tauriExport;
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
    loading, reloading, provisional, scanError, fetchError, scanSources, allPoints,
    refresh, refreshing, scanAt,
    range, setRange,
    from, to, firstIso, lastIso, customFrom, customTo, setCustomRange,
    reportInput,
    sel, setSel,
    rangeLabel, tool, grand, toolTotals, visibleTools,
    summary, modelRows, canOpenCostBreakdown, headline, unreadable,
    panels,
  } = useOverview(ports);

  // The window report: one CSV of whatever this tab is showing.
  const [exporting, setExporting] = useState(false);
  const [exportError, setExportError] = useState<string | null>(null);
  // A failure describes the window it was raised for, so a new window retires
  // it — otherwise the band would keep accusing a window that was never asked
  // to export.
  useEffect(() => setExportError(null), [range, from, to]);
  // Serializes the state that is already rendered — no refetch, so a scan
  // landing mid-export cannot produce a file matching neither the screen
  // before it nor the screen after.
  const exportWindow = async () => {
    setExporting(true);
    setExportError(null);
    try {
      // The click is the file's `generated` stamp. The window and its grain
      // still come from the render being exported (useOverview's viewNow), so
      // an idle tab exports the window it is showing, dated when you asked.
      const input = reportInput(settings, new Date());
      // A cancelled save resolves false and says nothing: backing out of the
      // sheet is a decision, not a fault.
      await exporter.saveCsv(reportFilename(input.fromIso, input.toIso), windowReportCsv(input));
    } catch (e) {
      setExportError(`${t('overview.exportFailed')}: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setExporting(false);
    }
  };

  // The ≥ reason (ADR-0017): which Sources hold Unreadable Artifacts whose
  // content could fall in this window — visible count beside the eyebrow,
  // per-Source detail as hover text on the marked figures.
  const unreadableTitle = unreadableReasons(unreadable, lang);
  const unreadableCount = unreadable.reduce((n, u) => n + u.artifactsUnreadable, 0);
  const unreadableKeys = new Set(unreadable.map((u) => u.source));
  const cardUnreadableTitle = (key: string) => {
    const u = unreadable.find((s) => s.source === key);
    return u ? unreadableReasons([u], lang) : undefined;
  };

  // The ≥ marker's only remedy: hand Antigravity's encrypted Sessions to its
  // own running server (ADR-0018), then rescan so the Artifacts it wrote land.
  // Offered beside the count because that is where the marker is explained, and
  // only for the Source the companion knows how to read.
  const [decrypting, setDecrypting] = useState(false);
  const [decryptNote, setDecryptNote] = useState<string | null>(null);
  const canDecrypt = unreadableKeys.has('antigravity');
  const runDecrypt = async () => {
    setDecrypting(true);
    setDecryptNote(null);
    try {
      setDecryptNote(await ledger.exportAntigravity());
      await refresh(); // the exports are only usage once a Scan has read them
    } catch (err) {
      setDecryptNote(String(err));
    } finally {
      setDecrypting(false);
    }
  };

  // Stable identity so the memoized ModelsList skips per-tick re-renders.
  const onModelClick = useCallback((name: string) => openPricing(name, tool.key), [openPricing, tool.key]);

  // The Rescan shortcut, the same chord the Menu Bar Extra's panel carries. It
  // lives here because this tab owns the scan, and it answers from every tab:
  // the shell keeps the Overview mounted (hidden) across tab switches, so a
  // rescan is never more than a keystroke away. `refresh` gates itself, so a
  // scan already running swallows the key exactly as it disables the button.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!isHotkey(e, 'rescan', platform)) return;
      e.preventDefault();
      void refresh();
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [platform, refresh]);

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
      {summary ? formatSummaryCost(summary, settings, lang) : '…'}
      <span className="tt-b8-cost-note" title={t('overview.notBilled')}> {t('overview.costNote')}</span>
      {summary?.hasUnpriced && (
        <span className="tt-b8-cost-mark" title={summary.unpricedModels.join(', ')}> · {summary.unpricedModels.length} {t('overview.unpricedMarker')}</span>
      )}
      {summary && summary.unattributedTokens > 0 && (
        <span className="tt-b8-cost-mark"> · {t('overview.unattributedUsage')}</span>
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
        <RangeSegments
          range={range}
          onRange={setRange}
          from={from}
          to={to}
          firstIso={firstIso}
          lastIso={lastIso}
          points={allPoints}
          onPick={setCustomRange}
        />
        <span className="tt-lastscan">{scanLabel}</span>
        <button
          type="button"
          className="tt-rescan"
          onClick={() => void refresh()}
          disabled={refreshing}
          aria-busy={refreshing}
          title={hotkeyHint('rescan', platform)}
        >
          <svg className="tt-rescan-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8" />
            <path d="M21 3v5h-5" />
            <path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16" />
            <path d="M8 16H3v5" />
          </svg>
          {tShell('header.rescan')}
        </button>
        <button
          type="button"
          className="tt-export"
          onClick={() => void exportWindow()}
          // Not disabled on an empty window: a window with no usage is a
          // legitimate report costing $0.00, the one zero that is a figure.
          // Disabled with no Summary, which is the opposite case — the report
          // input's token fields are non-nullable, so an absent Summary
          // serializes as 0, a figure that would mean "unknown". `loading` does
          // not cover it: that is the series alone.
          //
          // `reloading` is the window having moved ahead of the figures that
          // describe it — switch range and click here inside the reload and the
          // file would state the new window over the old window's Summary,
          // Sources, Models, Projects and Context. The screen shows that gap
          // too, but wears it for an instant; a file keeps it.
          //
          // `provisional` is the same argument one step earlier: the launch
          // paint's figures do describe this window, but from before the launch
          // scan settled. The screen wears that for half a second and corrects
          // itself; a file would state it forever.
          disabled={exporting || loading || reloading || provisional || summary === null}
          aria-busy={exporting}
        >
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
            <path d="M7 10l5 5 5-5" />
            <path d="M12 15V3" />
          </svg>
          {exporting ? t('overview.exporting') : t('overview.export')}
        </button>
      </div>

      <div className="tt-body">
      {(scanError || fetchError || exportError) && (
        <div className="tt-error">
          {[scanError, fetchError, exportError].filter(Boolean).join(' · ')}
        </div>
      )}

      {/* HERO: totals + proportion bar + per-source cards */}
      <div className="tt-hero">
        <div className="tt-hero-head">
          <div className="tt-eyebrow">
            {t('overview.totalTokens')} · {rangeLabel}
            {unreadableCount > 0 && (
              <span className="tt-b8-cost-mark" title={unreadableTitle}>
                {' · '}{countLabel(unreadableCount, 'overview.unreadableSessionOne', 'overview.unreadableSessionMany', lang)}
              </span>
            )}
            {canDecrypt && (
              <button
                type="button"
                className="tt-decrypt"
                onClick={() => void runDecrypt()}
                // Not gated on `refreshing`: that stays true for a full spinner
                // rotation after every scan, which would leave this dead for a
                // second at launch. Overlap is safe — exports land by rename.
                disabled={decrypting}
                aria-busy={decrypting}
                title={t('overview.decryptHint')}
              >
                {decrypting ? t('overview.decrypting') : t('overview.decrypt')}
              </button>
            )}
            {decryptNote && <span className="tt-decrypt-note">{decryptNote}</span>}
          </div>
          {/* windowKey is the same (range, from, to) triple rangeToFilters queries
              by, so the headline rolls for a period switch and holds still for a
              background scan. */}
          <TokenTotalHeadline
            total={headline.total}
            authoritative={headline.authoritative}
            windowKey={`${range}:${from}:${to}`}
            visible={visible}
            incomplete={unreadableTitle || null}
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
          {/* Only Sources with tokens render: a zero-width segment still carries
              the flex gap on both sides, wedging dead space between the segments
              that do have tokens (e.g. Claude first + pi last, five empties between). */}
          {visibleTools.map((tl) => (
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
                title={cardUnreadableTitle(tl.key)}
              >
                <div className="lbl">
                  <SourceIcon tool={tl} />
                  {tl.label}
                </div>
                <div className="num">
                  {/* the space is load-bearing: flex trims it visually, but it keeps
                      the two figures from reading as one number to a screen reader */}
                  {unreadableKeys.has(tl.key) ? '≥ ' : ''}{formatCompactTokenTotal(toolTotals[tl.key], 3)} <span className="pct">{fmtPct(toolTotals[tl.key] / grand)}</span>
                </div>
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
          <Profile profile={panels.profile} />
          <AggTrend data={panels.trend.data} per={panels.trend.per} rangeLabel={rangeLabel} modelTool={panels.trend.modelTool} onEnlarge={openTrendModal} enlargeRef={setTrendEnlargeTarget} />
          {/* The Heatmap always spans a trailing 365 days — it does not follow the
              range segment, so it sits after the range-responsive panels above. */}
          <Heatmap days={panels.heatmap.days} compact onEnlarge={openHeatModal} enlargeRef={setHeatEnlargeTarget} />
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
              skills={panels.context.skills}
              mcp={panels.context.mcp}
            />
          </div>
          <div className="tt-card">
            <ModelsList
              tool={tool}
              toolTokens={panels.models.toolTokens}
              models={panels.models.models}
              settings={settings}
              onModelClick={onModelClick}
            />
          </div>
          {panels.sparks.length > 0 && <SmallMultiples items={panels.sparks} rangeLabel={rangeLabel} />}
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
                {s.artifactsUnreadable > 0 && ` / ${s.artifactsUnreadable} ${t('overview.scanUnreadable')}`}
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
          unreadableTitle={unreadableReasons(
            unreadableSourcesIn(scanSources, heatFilters().startTs ?? null),
            lang,
          )}
          returnFocusRef={heatEnlargeRef}
          onClose={() => setHeatModalOpen(false)}
        />
      )}
      {trendModalOpen && (
        <TrendModal
          allPoints={allPoints}
          firstIso={firstIso}
          lastIso={lastIso}
          unreadable={unreadableSourcesIn(scanSources, null)}
          initialRange={range}
          initialCustomFrom={customFrom}
          initialCustomTo={customTo}
          ledger={ledger}
          exporter={exporter}
          returnFocusRef={trendEnlargeRef}
          onClose={() => setTrendModalOpen(false)}
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

// Brand-icon chip for a Source; falls back to a colored monogram when the Source
// has no brand mark.
function SourceIcon({ tool }: { tool: SourceMeta }) {
  const src = sourceIcon(tool.icon);
  return (
    <span className={'tt-toolicon ' + tool.icon}>
      {src ? (
        <img src={src} alt="" width={20} height={20} />
      ) : (
        <b style={{ color: tool.color }}>{tool.label[0]}</b>
      )}
    </span>
  );
}
