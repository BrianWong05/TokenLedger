// The Ledger seam: "remote but owned". Thin adapter over the Tauri IPC fns in
// src/api.ts, so the store depends on this port instead of @tauri-apps directly
// (lets tests swap in ledger.fake.ts). No logic here.
import { listen } from '@tauri-apps/api/event';
import {
  scan,
  fetchSeries,
  fetchSummary,
  fetchBreakdown,
  fetchCtxResources,
  fetchCtxBuckets,
  fetchCtxTools,
  fetchCtxExec,
} from '../api';
import type {
  Filters,
  ScanStatus,
  SeriesPoint,
  Summary,
  BreakdownRow,
  CtxResourceCount,
  CtxBuckets,
  CtxToolRow,
  CtxExecRow,
} from '../types';

export interface LedgerPort {
  scan(): Promise<ScanStatus>;
  series(filters: Filters, bucket: 'day' | 'hour'): Promise<SeriesPoint[]>;
  summary(filters: Filters): Promise<Summary>;
  breakdown(by: 'model' | 'project' | 'tool', filters: Filters): Promise<BreakdownRow[]>;
  ctxResources(filters: Filters): Promise<CtxResourceCount[]>;
  ctxBuckets(filters: Filters): Promise<CtxBuckets[]>;
  ctxTools(filters: Filters): Promise<CtxToolRow[]>;
  ctxExec(filters: Filters): Promise<CtxExecRow[]>;
  onPricesRebuilt(cb: () => void): () => void; // subscribe, returns unsubscribe
}

export const tauriLedger: LedgerPort = {
  scan,
  series: fetchSeries,
  summary: fetchSummary,
  breakdown: fetchBreakdown,
  ctxResources: fetchCtxResources,
  ctxBuckets: fetchCtxBuckets,
  ctxTools: fetchCtxTools,
  ctxExec: fetchCtxExec,
  onPricesRebuilt(cb) {
    // listen() is async; the unsubscribe resolves later, so teardown
    // must await it.
    const un = listen('prices-rebuilt', () => cb());
    return () => {
      un.then((f) => f());
    };
  },
};
