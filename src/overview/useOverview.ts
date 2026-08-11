// Thin React adapter over the framework-free OverviewStore: subscribes via
// useSyncExternalStore, drives the store lifecycle (prices listener, initial
// load, auto-refresh), and memoizes the selectors into a render-ready model so
// the shell derives nothing.
import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import {
  createOverviewStore,
  selectDays,
  selectProfile,
  selectReportInput,
  selectView,
  selectVisibleTools,
  type ClockPort,
} from './overviewStore';
import type { LedgerPort } from './ledger';
import type { Settings } from '../types';
import { useAutoRefresh } from './useAutoRefresh';
import { loadCustomRange, saveCustomRange } from './rangeMemory';
import { publishFirstRecord } from './ledgerExtent';
import type { Range8b, SourceKey } from './meta';
import { useT } from '../lib/i18n';

export function useOverview(ports?: { ledger?: LedgerPort; clock?: ClockPort }) {
  const [store] = useState(() => createOverviewStore(ports));
  const { lang } = useT();

  const snap = useSyncExternalStore(
    useCallback((cb: () => void) => store.subscribe(cb), [store]),
    useCallback(() => store.getSnapshot(), [store]),
  );

  // Prices listener + in-flight teardown on unmount.
  useEffect(() => store.start(), [store]);

  const refreshImpl = useCallback(() => store.refresh(), [store]);
  const { refresh, refreshing } = useAutoRefresh(refreshImpl);

  // Initial scan + load once on mount (same path as manual/auto refresh).
  useEffect(() => {
    void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Re-fill the last custom window once, after real series data has landed. The
  // wait is load-bearing: before it, firstIso is today, so the bounds below
  // would reject a perfectly good window. An empty allPoints is not enough of a
  // signal — the store also settles it to [] when the first load FAILS, before
  // retrying, and spending the one restore there would forget the window for
  // the rest of the session.
  // The range itself stays where the store put it: restoring dates changes
  // nothing on screen until Custom is picked.
  const restored = useRef(false);
  useEffect(() => {
    if (restored.current || !snap.allPoints?.length) return;
    restored.current = true;
    const saved = loadCustomRange();
    if (!saved) return;
    // A window saved against a longer history can start before the Ledger's
    // current first record; clamp rather than point the picker at a day it
    // will not let you select.
    const [from, to] = saved;
    if (to >= snap.firstIso) store.setCustomRange(from < snap.firstIso ? snap.firstIso : from, to);
  }, [snap.allPoints, snap.firstIso, store]);

  // Publish the extent for Settings, which captions each configured Preset with
  // the window it resolves to. Gated on real series data for the same reason the
  // restore above is: before it lands, firstIso is today, which would read as a
  // Ledger with no history at all.
  useEffect(() => {
    if (snap.allPoints?.length) publishFirstRecord(snap.firstIso);
  }, [snap.allPoints, snap.firstIso]);

  // The 365-day heatmap grid depends only on the full series — the store keeps
  // allPoints's reference stable across range/selection, so this never
  // recomputes on those.
  const days = useMemo(() => selectDays(snap), [snap.allPoints]);
  // Same shape as the heatmap: range/selection can't move it, so it recomputes
  // only when the series or its own Session count changes.
  const profile = useMemo(
    () => selectProfile(snap),
    [snap.allPoints, snap.profileSessions],
  );
  // Deps name the data fields rather than the snapshot: an idle 30s tick
  // publishes a new snapshot for scanAt alone, and rebuilding the whole view
  // for a clock label was most of the dashboard's steady-state CPU.
  /* eslint-disable react-hooks/exhaustive-deps */
  // `now` is captured rather than left to selectView's default, and kept beside
  // the view it produced: it decides both the window bounds and — through
  // hourlyDayOf — whether the report's time block is hourly. A second
  // `new Date()` at export time could answer either question differently from
  // the render being exported, so the report reuses this one.
  const { view, viewNow } = useMemo(
    () => {
      const now = new Date();
      return { view: selectView(snap, now, lang), viewNow: now };
    },
    [
      snap.allPoints, snap.hourPoints, snap.summary, snap.modelRows, snap.projectRows,
      snap.ctxResources, snap.ctxBuckets, snap.ctxToolRows, snap.ctxExecRows,
      snap.range, snap.selected, snap.customFrom, snap.customTo,
      snap.from, snap.to, snap.firstIso, snap.lastIso,
      lang,
    ],
  );
  const visibleTools = useMemo(
    () =>
      selectVisibleTools(snap).map((t) => ({
        ...t,
        nModels: snap.modelRows.filter((r) => r.source === t.key && r.key !== null).length,
      })),
    [snap.allPoints, snap.modelRows, snap.range, snap.from, snap.to, snap.lastIso],
  );
  /* eslint-enable react-hooks/exhaustive-deps */

  // The report's input, built from the same snapshot and view this hook renders
  // from — which is what keeps the exported file from disagreeing with the
  // screen. `viewNow` fixes the window and the grain; `generatedAt` is passed
  // separately by the caller and is the instant the file was asked for, which
  // on a tab left open all afternoon is nowhere near `viewNow`. `settings`
  // arrives from the component too: Display Currency lives in SettingsContext,
  // which the shell consumes and this hook does not.
  const reportInput = useCallback(
    (settings: Settings, generatedAt: Date) =>
      selectReportInput(snap, view, settings, viewNow, generatedAt),
    [snap, view, viewNow],
  );

  const setRange = useCallback((r: Range8b) => store.setRange(r), [store]);
  const setSel = useCallback((k: SourceKey) => store.setSelected(k), [store]);
  const setCustomRange = useCallback(
    (from: string, to: string) => {
      saveCustomRange(from, to);
      store.setCustomRange(from, to);
    },
    [store],
  );

  return {
    loading: snap.loading,
    reloading: snap.reloading,
    // Full unbounded daily series — the trend enlarge buckets it for its own
    // local window (the store keeps this reference stable across ticks).
    allPoints: snap.allPoints ?? [],
    scanSources: snap.scanSources,
    scanError: snap.scanError,
    fetchError: snap.fetchError,
    refresh,
    refreshing,
    scanAt: snap.scanAt,
    range: snap.range,
    setRange,
    from: snap.from,
    to: snap.to,
    firstIso: snap.firstIso,
    lastIso: snap.lastIso,
    customFrom: snap.customFrom,
    customTo: snap.customTo,
    setCustomRange,
    reportInput,
    sel: snap.selected,
    setSel,
    rangeLabel: view.rangeLabel,
    tool: view.tool,
    grand: view.grand,
    toolTotals: view.toolTotals,
    visibleTools,
    summary: snap.summary,
    modelRows: snap.modelRows,
    canOpenCostBreakdown: view.canOpenCostBreakdown,
    headline: view.headline,
    unreadable: view.unreadable,
    panels: {
      heatmap: { days },
      profile,
      trend: { data: view.trend, per: view.per, modelTool: view.modelTool },
      sparks: view.sparks,
      context: {
        tool: view.tool,
        ctx: view.ctx,
        view: view.ctxView,
        tree: view.ctxTree,
        execRows: view.selExecRows,
        meta: view.selMeta,
        skills: view.selSkills,
        mcp: view.selMcp,
      },
      tokens: { cats: view.cats },
      models: { toolTokens: view.toolTotals[snap.selected], models: view.selModels },
      table: { dailyRows: view.dailyRows, projectRows: view.projectRows },
    },
  };
}
