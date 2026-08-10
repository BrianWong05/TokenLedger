import { describe, expect, it } from 'vitest';
import { reportFilename, windowReportCsv, type ReportInput, type ReportUsageRow } from './reportCsv';

const USAGE_COLS_FOR_TEST =
  'input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,total_tokens,' +
  'requests,sessions,cache_hit_rate,cost_usd,cost_basis,unattributed_tokens,cache_estimated';

// A fully-priced row. Tests override only the field under test, so a column's
// rule is stated in one place and every other column stays neutral.
function usageRow(over: Partial<ReportUsageRow> = {}): ReportUsageRow {
  return {
    key: '2026-07-12 .. 2026-08-10',
    inputTokens: 100,
    outputTokens: 50,
    cacheReadTokens: 800,
    cacheWriteTokens: 50,
    totalTokens: 1000,
    requests: 7,
    sessions: 2,
    cost: 1.5,
    hasUnpriced: false,
    unattributedTokens: 0,
    cacheEstimated: false,
    ...over,
  };
}

function reportInput(over: Partial<ReportInput> = {}): ReportInput {
  return {
    generatedIso: '2026-08-10T13:35:12.000Z',
    fromIso: '2026-07-12',
    toIso: '2026-08-10',
    grain: 'day',
    tokensBasis: 'exact',
    displayCurrency: null,
    usdRate: null,
    summary: usageRow(),
    unpricedModels: [],
    cacheEstimatedModels: [],
    time: [],
    sources: [],
    models: [],
    projects: [],
    ctxCategories: [],
    ctxTools: [],
    ctxMcp: [],
    ctxSkills: [],
    ctxExec: [],
    ...over,
  };
}

// Every block is blank-line separated and identified by its first column, so a
// test pulls one out by that column's name without counting lines.
function block(csv: string, firstColumn: string): string[] {
  const found = csv.split('\n\n').find((b) => b.split('\n')[0].startsWith(`${firstColumn},`));
  return found ? found.trimEnd().split('\n') : [];
}

describe('header block', () => {
  it('states the schema, window, grain and basis', () => {
    const head = windowReportCsv(reportInput()).split('\n\n')[0].split('\n');
    expect(head).toContain('tokenledger_report,1');
    expect(head).toContain('generated,2026-08-10T13:35:12.000Z');
    expect(head).toContain('window,2026-07-12,2026-08-10');
    expect(head).toContain('window_grain,day');
    expect(head).toContain('tokens_basis,exact');
    expect(head).toContain('currency,USD');
  });

  it('omits the Display Currency lines when none is set', () => {
    const head = windowReportCsv(reportInput()).split('\n\n')[0];
    expect(head).not.toContain('display_currency');
    expect(head).not.toContain('display_rate');
  });

  it('records the Display Currency and rate without moving Cost off USD', () => {
    const csv = windowReportCsv(reportInput({ displayCurrency: 'AUD', usdRate: 1.52 }));
    const head = csv.split('\n\n')[0];
    expect(head).toContain('display_currency,AUD');
    expect(head).toContain('display_rate,1.52');
    expect(block(csv, 'window')[1]).toContain('1.500000');
  });

  it('marks the window a floor when an Unreadable Artifact could reach it', () => {
    const head = windowReportCsv(reportInput({ tokensBasis: 'floor' })).split('\n\n')[0];
    expect(head).toContain('tokens_basis,floor');
  });
});

describe('summary block', () => {
  it('writes every token category, the derived hit rate and an exact cost', () => {
    const rows = block(windowReportCsv(reportInput()), 'window');
    expect(rows[0]).toBe(`window,${USAGE_COLS_FOR_TEST},unpriced_models,cache_estimated_models`);
    // hit rate is 800 / (100 + 800 + 50)
    expect(rows[1]).toBe('2026-07-12 .. 2026-08-10,100,50,800,50,1000,7,2,0.8421,1.500000,exact,0,false,,');
  });

  it('leaves cost empty and unavailable when nothing in the window is priced', () => {
    const csv = windowReportCsv(
      reportInput({
        summary: usageRow({ cost: null, hasUnpriced: true }),
        unpricedModels: ['local-llama', 'my-model'],
      }),
    );
    const row = block(csv, 'window')[1];
    expect(row).toContain(',,unavailable,');
    expect(row).not.toContain(',0.000000,');
    expect(row).toContain('local-llama my-model');
  });

  it('calls a cost partial when priced usage is mixed with unpriced or unattributed', () => {
    const unpriced = block(windowReportCsv(reportInput({ summary: usageRow({ hasUnpriced: true }) })), 'window')[1];
    expect(unpriced).toContain('1.500000,partial,');

    const unattributed = block(
      windowReportCsv(reportInput({ summary: usageRow({ unattributedTokens: 400 }) })),
      'window',
    )[1];
    expect(unattributed).toContain('1.500000,partial,400,');
  });

  it('keeps a genuinely free window exact rather than calling it a gap', () => {
    const row = block(windowReportCsv(reportInput({ summary: usageRow({ cost: 0 }) })), 'window')[1];
    expect(row).toContain('0.000000,exact,');
  });

  it('reports a zero hit rate for a window with no prompt tokens at all', () => {
    const row = block(
      windowReportCsv(
        reportInput({
          summary: usageRow({ inputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0, totalTokens: 50 }),
        }),
      ),
      'window',
    )[1];
    expect(row).toContain(',0.0000,');
  });
});

describe('reportFilename', () => {
  it('extends the bucket export convention with a range', () => {
    expect(reportFilename('2026-07-12', '2026-08-10')).toBe('usage-2026-07-12_2026-08-10.csv');
  });
});
