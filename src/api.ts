import { invoke } from '@tauri-apps/api/core';
import type {
  ScanStatus,
  Summary,
  SeriesPoint,
  BreakdownRow,
  CtxResourceCount,
  CtxBuckets,
  CtxToolRow,
  CtxExecRow,
  Filters,
} from './types';

export function scan(): Promise<ScanStatus> {
  return invoke('scan');
}

export function fetchSummary(filters: Filters): Promise<Summary> {
  return invoke('summary', { filters });
}

export function fetchSeries(
  filters: Filters,
  bucket: 'day' | 'hour',
): Promise<SeriesPoint[]> {
  return invoke('series', { filters, bucket });
}

export function fetchBreakdown(
  by: 'tool' | 'model' | 'project',
  filters: Filters,
): Promise<BreakdownRow[]> {
  return invoke('breakdown', { by, filters });
}

export function fetchCtxResources(filters: Filters): Promise<CtxResourceCount[]> {
  return invoke('ctx_resources', { filters });
}

export function fetchCtxBuckets(filters: Filters): Promise<CtxBuckets[]> {
  return invoke('ctx_buckets', { filters });
}

export function fetchCtxTools(filters: Filters): Promise<CtxToolRow[]> {
  return invoke('ctx_tools', { filters });
}

export function fetchCtxExec(filters: Filters): Promise<CtxExecRow[]> {
  return invoke('ctx_exec', { filters });
}
