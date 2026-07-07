// Mirrors the Rust IPC structs (serde rename_all = "camelCase"). Do not rename.

export type Tool = 'claude' | 'codex' | 'gemini' | 'hermes';

export type RangePreset = 'today' | '7d' | '30d' | 'all';

export interface CustomRange {
  start: string; // 'YYYY-MM-DD', inclusive
  end: string;   // 'YYYY-MM-DD', inclusive
}

export type DateRange = RangePreset | CustomRange;

export interface Filters {
  tools: string[];          // empty = all
  models: string[];         // empty = all
  project: string | null;
  startTs?: number;         // epoch seconds, inclusive
  endTs?: number;           // epoch seconds, exclusive
}

export interface Summary {
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number; // 5m + 1h combined
  totalTokens: number;      // hero number
  requests: number;
  cost: number | null;      // null when zero priced tokens in range
  hasUnpriced: boolean;
  unpricedModels: string[];
  cacheHitRate: number;     // 0..1
}

export interface TrendPoint {
  bucket: string;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  totalTokens: number;
  cost: number;
}

export interface BreakdownRow {
  key: string;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  totalTokens: number;
  requests: number;
  cost: number | null;
}

export interface SourceStatus {
  source: string;
  eventsInserted: number;
  linesSkipped: number;
  error: string | null;
}

export interface ScanStatus {
  sources: SourceStatus[];
  scannedAt: number;
}

export interface OverrideRates {
  input: number | null;      // per token; null = 0
  output: number | null;
  cacheRead: number | null;
  cacheWrite: number | null;
}
