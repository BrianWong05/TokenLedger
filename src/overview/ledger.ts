// The Ledger seam: "remote but owned". Thin adapter over the Tauri IPC fns in
// src/api.ts, so the store depends on this port instead of @tauri-apps directly
// (lets tests swap in ledger.fake.ts). No logic here.
import { listen } from '@tauri-apps/api/event';
import {
  scan,
  fetchLastScan,
  fetchUnreadableArtifacts,
  exportAntigravity,
  fetchSeries,
  fetchSummary,
  fetchBreakdown,
  fetchCtxResources,
  fetchCtxBuckets,
  fetchCtxTools,
  fetchCtxSkills,
  fetchCtxExec,
} from '../api';
import type {
  Filters,
  ScanStatus,
  SourceUnreadable,
  SeriesPoint,
  Summary,
  BreakdownRow,
  CtxResource,
  CtxBuckets,
  CtxToolRow,
  CtxSkillRow,
  CtxExecRow,
} from '../types';

export interface LedgerPort {
  scan(): Promise<ScanStatus>;
  lastScan(): Promise<number>; // epoch seconds, 0 before the first Scan
  unreadableArtifacts(): Promise<SourceUnreadable[]>; // persisted state, no rescan
  // Runs the export companion (ADR-0018). Not part of any Scan: the caller
  // is a person pressing a button, and the answer is its report.
  exportAntigravity(): Promise<string>;

  series(filters: Filters, bucket: 'day' | 'hour'): Promise<SeriesPoint[]>;
  summary(filters: Filters): Promise<Summary>;
  breakdown(by: 'model' | 'project' | 'tool', filters: Filters): Promise<BreakdownRow[]>;
  ctxResources(filters: Filters): Promise<CtxResource[]>;
  ctxBuckets(filters: Filters): Promise<CtxBuckets[]>;
  ctxTools(filters: Filters): Promise<CtxToolRow[]>;
  ctxSkills(filters: Filters): Promise<CtxSkillRow[]>;
  ctxExec(filters: Filters): Promise<CtxExecRow[]>;
  onPricesRebuilt(cb: () => void): () => void; // subscribe, returns unsubscribe
}

export const tauriLedger: LedgerPort = {
  scan,
  lastScan: fetchLastScan,
  unreadableArtifacts: fetchUnreadableArtifacts,
  exportAntigravity,
  series: fetchSeries,
  summary: fetchSummary,
  breakdown: fetchBreakdown,
  ctxResources: fetchCtxResources,
  ctxBuckets: fetchCtxBuckets,
  ctxTools: fetchCtxTools,
  ctxSkills: fetchCtxSkills,
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
