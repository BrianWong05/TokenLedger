// Mirrors the Rust IPC structs (serde rename_all = "camelCase"). Do not rename.

export type Tool = 'claude' | 'codex' | 'gemini' | 'hermes' | 'grok' | 'antigravity';

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
  cacheEstimatedModels: string[];
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

export interface SeriesPoint {
  bucket: string;                 // 'YYYY-MM-DD' (day) or 'YYYY-MM-DD HH:00' (hour)
  source: string;
  byModel: Record<string, number>; // model -> total tokens within (bucket, source)
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  totalTokens: number;
  reasoningTokens: number | null;
  cost: number;
  requests: number;
  convs: number;
  ctxMessages: number | null;
  ctxSystem: number | null;
  ctxReasoning: number | null;
  ctxToolcalls: number | null;
  ctxAgents: number | null;
  ctxMcp: number | null;
  ctxSkills: number | null;
}

export interface CtxResourceCount {
  source: string;
  kind: string; // 'skill' | 'mcp_server' | 'agent' | 'memory_file'
  count: number;
}

export interface CtxBuckets {
  source: string;
  history: number;         // cache reads + non-first cache writes
  newInput: number;        // fresh input tokens
  system: number | null;   // first cache write per session; null = unknowable
  response: number;        // output − reasoning
  reasoning: number | null;
}

export interface CtxToolRow {
  source: string;
  name: string;
  estTokens: number; // allocation weight (estimated content), not display value
  calls: number;
}

export interface CtxExecRow {
  source: string;
  kind: string;   // classified command kind (git_local, test, compound, …)
  exe: string;    // executable basename
  cmd: string;    // signature: executable + first subcommand
  estTokens: number; // allocation weight, not a display value
  calls: number;
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
  source: string | null;          // set only for by='model'
  reasoningTokens: number | null; // null = not reported by the source(s)
  convs: number;
  cacheEstimated: boolean;
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
