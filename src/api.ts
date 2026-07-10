import { invoke } from '@tauri-apps/api/core';
import type {
  ScanStatus,
  Summary,
  TrendPoint,
  SeriesPoint,
  BreakdownRow,
  CtxResourceCount,
  Filters,
  OverrideRates,
} from './types';

export function scan(): Promise<ScanStatus> {
  return invoke('scan');
}

export function fetchSummary(filters: Filters): Promise<Summary> {
  return invoke('summary', { filters });
}

export function fetchTrend(
  filters: Filters,
  bucket: 'day' | 'hour',
): Promise<TrendPoint[]> {
  return invoke('trend', { filters, bucket });
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

export function setPriceOverride(
  model: string,
  rates: OverrideRates,
): Promise<void> {
  return invoke('set_price_override', { model, rates });
}

export function deletePriceOverride(model: string): Promise<void> {
  return invoke('delete_price_override', { model });
}
