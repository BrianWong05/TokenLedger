// The Ledger seam: "remote but owned". Thin adapter over the Tauri IPC fns in
// src/api.ts, so the store depends on this port instead of @tauri-apps directly
// (lets tests swap in ledger.fake.ts). No logic here.
import { listen } from '@tauri-apps/api/event';
import {
  scan,
  fetchLastScan,
  fetchUnreadableArtifacts,
  fetchUnbookedRequests,
  exportAntigravity,
  fetchSeries,
  fetchSummary,
  fetchWindow,
  fetchBreakdown,
  fetchContext,
} from '../api';
import type {
  Filters,
  ScanStatus,
  SourceUnreadable,
  SourceUnbooked,
  SeriesPoint,
  Summary,
  LedgerWindow,
  LedgerContext,
  BreakdownRow,
} from '../types';

export interface LedgerPort {
  scan(): Promise<ScanStatus>;
  lastScan(): Promise<number>; // epoch seconds, 0 before the first Scan
  unreadableArtifacts(): Promise<SourceUnreadable[]>; // persisted state, no rescan
  unbookedRequests(): Promise<SourceUnbooked[]>; // persisted state, no rescan
  // Runs the export companion (ADR-0018). Not part of any Scan: the caller
  // is a person pressing a button, and the answer is its report.
  exportAntigravity(): Promise<string>;

  series(filters: Filters, bucket: 'day' | 'hour'): Promise<SeriesPoint[]>;
  summary(filters: Filters): Promise<Summary>;
  // Date window of priced facts. The Overview store's range reload uses this;
  // Cost-only callers keep summary().
  window(filters: Filters): Promise<LedgerWindow>;
  breakdown(by: 'model' | 'project' | 'tool', filters: Filters): Promise<BreakdownRow[]>;
  // Date window of Context. The Overview store's range reload uses this.
  context(filters: Filters): Promise<LedgerContext>;
  onPricesRebuilt(cb: () => void): () => void; // subscribe, returns unsubscribe
}

export const tauriLedger: LedgerPort = {
  scan,
  lastScan: fetchLastScan,
  unreadableArtifacts: fetchUnreadableArtifacts,
  unbookedRequests: fetchUnbookedRequests,
  exportAntigravity,
  series: fetchSeries,
  summary: fetchSummary,
  window: fetchWindow,
  breakdown: fetchBreakdown,
  context: fetchContext,
  onPricesRebuilt(cb) {
    // listen() is async; the unsubscribe resolves later, so teardown
    // must await it.
    const un = listen('prices-rebuilt', () => cb());
    return () => {
      un.then((f) => f());
    };
  },
};
