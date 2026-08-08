import { invoke } from '@tauri-apps/api/core';
import type {
  ScanStatus,
  SourceStatus,
  Summary,
  SeriesPoint,
  BreakdownRow,
  CtxResourceCount,
  CtxBuckets,
  CtxToolRow,
  CtxExecRow,
  Filters,
  ModelPricing,
  RatesPerTok,
  Settings,
  UpdateStatus,
} from './types';

export function scan(): Promise<ScanStatus> {
  return invoke('scan');
}

// Epoch seconds of the last Scan this launch; 0 when none has run yet.
export function fetchLastScan(): Promise<number> {
  return invoke('last_scan');
}

// The most recent scan's per-source statuses, without rescanning — the
// traypanel's read for the Unreadable-Artifact ≥ marker (ADR-0017).
export function fetchScanSources(): Promise<SourceStatus[]> {
  return invoke('scan_sources');
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

// ---- Pricing ----

export function modelPricing(): Promise<ModelPricing[]> {
  return invoke('model_pricing');
}

export function setModelOverride(model: string, rates: RatesPerTok): Promise<void> {
  return invoke('set_model_override', { model, rates });
}

export function deleteModelOverride(model: string): Promise<void> {
  return invoke('delete_model_override', { model });
}

export function refreshPrices(): Promise<void> {
  return invoke('refresh_prices');
}

// ---- Settings ----

export function getSettings(): Promise<Settings> {
  return invoke('get_settings');
}

export function setSettings(settings: Settings): Promise<void> {
  return invoke('set_settings', { settings });
}

export function checkUpdates(): Promise<UpdateStatus> {
  return invoke('check_updates');
}
