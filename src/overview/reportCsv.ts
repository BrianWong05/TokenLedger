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

// Quoting follows src-tauri/src/report.rs esc, not data.ts bucketCsv: it covers
// \r too. A lone CR emitted unquoted is a record terminator to many parsers, so
// one row silently arrives as two. Quoting is here for Task 2's Project block —
// a path carries commas, and a name copied from a log can carry a stray CR.
const esc = (s: string) => (/[",\n\r]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s);

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

// A usage block, or nothing. An empty block is omitted rather than written as a
// lone header: a reader should not have to tell "no rows" from "no data".
function usageBlock(first: string, rows: ReportUsageRow[], withSource = false): string | null {
  if (rows.length === 0) return null;
  const header = withSource ? `${first},source,${USAGE_COLS}` : `${first},${USAGE_COLS}`;
  const lines = rows.map((row) =>
    [esc(row.key), ...(withSource ? [esc(row.source ?? '')] : []), ...usageCells(row)].join(','),
  );
  return [header, ...lines].join('\n');
}

// Context blocks carry no total row, deliberately: tool calls, subagents, MCP
// and skills are overlapping subsets of messages and do not sum to a whole, so
// a total would invite exactly the reading the app declines to offer.
function ctxBlock(header: string, rows: string[]): string | null {
  return rows.length === 0 ? null : [header, ...rows].join('\n');
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

  const blocks = [
    header.join('\n'),
    summary.join('\n'),
    usageBlock(input.grain, input.time),
    usageBlock('source', input.sources),
    usageBlock('model', input.models, true),
    usageBlock('project', input.projects),
    ctxBlock(
      'context,source,est_tokens,basis',
      input.ctxCategories.map((c) => [esc(c.category), esc(c.source), String(c.estTokens), c.basis].join(',')),
    ),
    ctxBlock(
      'tool,source,category,est_tokens,calls',
      input.ctxTools.map((t) =>
        [esc(t.name), esc(t.source), esc(t.category), String(t.estTokens), String(t.calls)].join(','),
      ),
    ),
    ctxBlock(
      'mcp_server,source,est_tokens,calls',
      input.ctxMcp.map((m) => [esc(m.name), esc(m.source), String(m.estTokens), String(m.calls)].join(',')),
    ),
    ctxBlock(
      'skill,source,est_tokens,uses',
      input.ctxSkills.map((s) => [esc(s.name), esc(s.source), String(s.estTokens), String(s.uses)].join(',')),
    ),
    ctxBlock(
      'bash,source,exe,est_tokens,calls',
      // The first column is the signature as displayed — the executable plus
      // its first non-flag argument (ADR-0011) — with exe repeated so rows can
      // be grouped by executable.
      input.ctxExec.map((e) =>
        [esc(`${e.exe} ${e.cmd}`.trim()), esc(e.source), esc(e.exe), String(e.estTokens), String(e.calls)].join(','),
      ),
    ),
  ];
  return blocks.filter((b): b is string => b !== null).join('\n\n') + '\n';
}
