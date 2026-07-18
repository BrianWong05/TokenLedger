// Thin React adapter over the framework-free OverviewStore: subscribes via
// useSyncExternalStore, drives the store lifecycle (prices listener, initial
// load, auto-refresh), and memoizes the selectors into a render-ready model so
// the shell derives nothing.
import { useCallback, useEffect, useMemo, useState, useSyncExternalStore } from 'react';
import {
  createOverviewStore,
  selectDays,
  selectView,
  selectVisibleTools,
  type ClockPort,
} from './overviewStore';
import type { LedgerPort } from './ledger';
import { useAutoRefresh } from './useAutoRefresh';
import type { Range8b, ToolKey } from './meta';
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

  // The 365-day heatmap grid depends only on the full series — the store keeps
  // allPoints's reference stable across range/selection, so this never
  // recomputes on those.
  const days = useMemo(() => selectDays(snap), [snap.allPoints]);
  const view = useMemo(() => selectView(snap, undefined, lang), [snap, lang]);
  const visibleTools = useMemo(
    () =>
      selectVisibleTools(snap).map((t) => ({
        ...t,
        nModels: snap.modelRows.filter((r) => r.source === t.key).length,
      })),
    [snap],
  );

  const setRange = useCallback((r: Range8b) => store.setRange(r), [store]);
  const setSel = useCallback((k: ToolKey) => store.setSelected(k), [store]);
  const setCustomRange = useCallback(
    (from: string, to: string) => store.setCustomRange(from, to),
    [store],
  );

  return {
    loading: snap.loading,
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
    panels: {
      heatmap: { days },
      trend: { data: view.trend, per: view.per, modelTool: view.modelTool },
      sparks: view.sparks,
      context: {
        tool: view.tool,
        ctx: view.ctx,
        view: view.ctxView,
        tree: view.ctxTree,
        execRows: view.selExecRows,
        meta: view.selMeta,
      },
      tokens: { cats: view.cats },
      models: { toolTokens: view.toolTotals[snap.selected], models: view.selModels },
      table: { dailyRows: view.dailyRows, projectRows: view.projectRows },
    },
  };
}
