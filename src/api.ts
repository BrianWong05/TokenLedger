import { invoke } from '@tauri-apps/api/core';
import type {
  ScanStatus,
  SourceLimits,
  SourceUnreadable,
  Summary,
  LedgerWindow,
  LedgerContext,
  SeriesPoint,
  BreakdownRow,
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

// Sources currently holding Unreadable Artifacts (ADR-0017), from the
// persisted per-scan state — no rescan, honest from launch. The one
// provenance every ≥ floor marker reads (the Overview's store and the
// Menu Bar Extra's panel alike).
export function fetchUnreadableArtifacts(): Promise<SourceUnreadable[]> {
  return invoke('unreadable_artifacts');
}

// Decrypt Antigravity's `.pb` Sessions by running the export companion
// (ADR-0018): a separate process, started only because someone asked for it.
// Resolves with the companion's own one-line report; the Artifacts it writes
// are picked up by the next Scan like any other file.
export function exportAntigravity(): Promise<string> {
  return invoke('export_antigravity');
}

export function fetchSummary(filters: Filters): Promise<Summary> {
  return invoke('summary', { filters });
}

// One date window of priced facts: Summary plus Model, Project, and Source
// breakdowns. Cost-only callers keep fetchSummary.
export function fetchWindow(filters: Filters): Promise<LedgerWindow> {
  return invoke('window', { filters });
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

// One date window of Context. Overview range reloads use this.
export function fetchContext(filters: Filters): Promise<LedgerContext> {
  return invoke('context', { filters });
}

// ---- Limits ----

// The current state of every Limit the Ledger holds Readings for. Takes no
// Filters: the Limits page is *now*, not a range.
export function fetchLimits(): Promise<SourceLimits[]> {
  return invoke('limits');
}

// Ask a Source's limits Companion for a live reading (ADR-0019): a separate
// process, started only because someone asked for it, that presents the sign-in
// that Source's CLI already stores and asks the vendor — read-only — how much of
// each window is used. Its Readings land in the durable series, so the page reads
// them back through `fetchLimits`; this rejects with the Companion's own failure
// line, which the page classifies.
export function checkLiveLimits(source: string): Promise<void> {
  return invoke('check_live_limits', { source });
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
