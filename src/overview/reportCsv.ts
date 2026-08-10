// The window report: one CSV over whatever the Overview is showing. Pure — it
// takes a snapshot of already-rendered state and returns a string, so the file
// can never disagree with the screen it was taken from, and the Rust save_csv
// command owns the dialog and the write exactly as it does for a Trend bucket.
//
// One rule governs every cell: an unknown is empty, never 0. That is the CSV
// form of the rule the UI follows — Unpriced is never $0 — and it keeps a
// spreadsheet's SUM() from absorbing a gap.
import type { Granularity } from './data';

// A usage row in the shape every non-Context block shares. `cost: null` is
// Unpriced usage — no priced tokens, so no figure — distinct from a real 0.
export interface ReportUsageRow {
  key: string;
  source?: string; // Model rows only: a Model is scoped to the tool that ran it
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  totalTokens: number;
  requests: number;
  sessions: number;
  cost: number | null;
  hasUnpriced: boolean;
  unattributedTokens: number;
  cacheEstimated: boolean;
}

export interface ReportCtxCategory {
  source: string;
  category: string;
  estTokens: number;
  // messages/system/reasoning partition the billed total; the rest are
  // overlapping subsets estimated from content size.
  basis: 'exact' | 'estimated';
}
export interface ReportCtxTool { source: string; category: string; name: string; estTokens: number; calls: number }
export interface ReportCtxMcp { source: string; name: string; estTokens: number; calls: number }
export interface ReportCtxSkill { source: string; name: string; estTokens: number; uses: number }
export interface ReportCtxExec { source: string; exe: string; cmd: string; estTokens: number; calls: number }

export interface ReportInput {
  generatedIso: string;
  fromIso: string;
  toIso: string;
  grain: Granularity;
  tokensBasis: 'exact' | 'floor';
  // null when Cost already renders in USD; the figures below stay USD either way.
  displayCurrency: string | null;
  usdRate: number | null;
  summary: ReportUsageRow;
  unpricedModels: string[];
  cacheEstimatedModels: string[];
  time: ReportUsageRow[];
  sources: ReportUsageRow[];
  models: ReportUsageRow[];
  projects: ReportUsageRow[];
  ctxCategories: ReportCtxCategory[];
  ctxTools: ReportCtxTool[];
  ctxMcp: ReportCtxMcp[];
  ctxSkills: ReportCtxSkill[];
  ctxExec: ReportCtxExec[];
}

// Same quoting rule as data.ts bucketCsv — Project paths carry commas.
const esc = (s: string) => (/[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s);

// How much of a row's Cost the Ledger could compute, in one word, so a Partial
// Cost is never read as a total.
function costBasis(row: ReportUsageRow): 'exact' | 'partial' | 'unavailable' {
  if (row.cost === null) return 'unavailable';
  return row.hasUnpriced || row.unattributedTokens > 0 ? 'partial' : 'exact';
}

// Well defined in every block precisely because Input excludes cache reads
// (ADR-0001). BreakdownRow does not carry it, so it is derived here rather
// than in three places.
function cacheHitRate(row: ReportUsageRow): number {
  const prompt = row.inputTokens + row.cacheReadTokens + row.cacheWriteTokens;
  return prompt > 0 ? row.cacheReadTokens / prompt : 0;
}

const USAGE_COLS =
  'input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,total_tokens,' +
  'requests,sessions,cache_hit_rate,cost_usd,cost_basis,unattributed_tokens,cache_estimated';

function usageCells(row: ReportUsageRow): string[] {
  return [
    String(row.inputTokens),
    String(row.outputTokens),
    String(row.cacheReadTokens),
    String(row.cacheWriteTokens),
    String(row.totalTokens),
    String(row.requests),
    String(row.sessions),
    cacheHitRate(row).toFixed(4),
    row.cost === null ? '' : row.cost.toFixed(6),
    costBasis(row),
    String(row.unattributedTokens),
    String(row.cacheEstimated),
  ];
}

export function reportFilename(fromIso: string, toIso: string): string {
  return `usage-${fromIso}_${toIso}.csv`;
}

export function windowReportCsv(input: ReportInput): string {
  const header = [
    'tokenledger_report,1',
    `generated,${input.generatedIso}`,
    `window,${input.fromIso},${input.toIso}`,
    `window_grain,${input.grain}`,
    `tokens_basis,${input.tokensBasis}`,
    'currency,USD',
  ];
  if (input.displayCurrency !== null && input.usdRate !== null) {
    header.push(`display_currency,${esc(input.displayCurrency)}`, `display_rate,${input.usdRate}`);
  }

  const summary = [
    `window,${USAGE_COLS},unpriced_models,cache_estimated_models`,
    [
      esc(input.summary.key),
      ...usageCells(input.summary),
      esc(input.unpricedModels.join(' ')),
      esc(input.cacheEstimatedModels.join(' ')),
    ].join(','),
  ];

  return [header.join('\n'), summary.join('\n')].join('\n\n') + '\n';
}
