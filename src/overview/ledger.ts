// The Ledger seam: "remote but owned". Thin adapter over the Tauri IPC fns in
// src/api.ts, so the store depends on this port instead of @tauri-apps directly
// (lets tests swap in ledger.fake.ts). No logic here.
import { listen } from '@tauri-apps/api/event';
import {
  scan,
  fetchLastScan,
  fetchScanSources,
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
  SourceStatus,
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
  lastScan(): Promise<number>; // epoch seconds, 0 before the first Scan
  scanSources(): Promise<SourceStatus[]>; // last scan's statuses, no rescan

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
  lastScan: fetchLastScan,
  scanSources: fetchScanSources,
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
