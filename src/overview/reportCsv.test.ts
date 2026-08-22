import { describe, expect, it } from 'vitest';
import { reportFilename, windowReportCsv, type ReportInput, type ReportUsageRow } from './reportCsv';

const USAGE_COLS_FOR_TEST =
  'input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,total_tokens,' +
  'requests,sessions,cache_hit_rate,cost_usd,cost_basis,unattributed_tokens,cache_estimated';

// The time block drops the one column that does not add up down the page.
const TIME_COLS_FOR_TEST = USAGE_COLS_FOR_TEST.replace(',sessions,', ',');

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

  it('flags a Cache-Estimated window as exact Cost and lists the Models', () => {
    const csv = windowReportCsv(
      reportInput({
        summary: usageRow({ cacheEstimated: true }),
        cacheEstimatedModels: ['gpt-5-codex'],
      }),
    );
    const row = block(csv, 'window')[1];
    expect(row).toContain(',exact,0,true,');
    expect(row.endsWith(',gpt-5-codex')).toBe(true);
  });
});

// esc is the file's only branching helper, and every other fixture is
// comma-free, so without these its quoting is asserted nowhere.
describe('field quoting', () => {
  const keyRow = (key: string) => block(windowReportCsv(reportInput({ summary: usageRow({ key }) })), 'window')[1];

  it('quotes a key carrying a comma so the row keeps its column count', () => {
    expect(keyRow('/Users/me/src/app,v2')).toBe(
      '"/Users/me/src/app,v2",100,50,800,50,1000,7,2,0.8421,1.500000,exact,0,false,,',
    );
  });

  it('doubles an embedded quote rather than ending the field early', () => {
    expect(keyRow('say "hi"')).toContain('"say ""hi"""');
  });

  it('quotes a bare carriage return, which a parser would otherwise read as a new record', () => {
    expect(keyRow('jul\raug')).toContain('"jul\raug"');
  });

  it('leaves an ordinary key unquoted', () => {
    expect(keyRow('2026-07-12').split(',')[0]).toBe('2026-07-12');
  });
});

// block() above reads the file by these two invariants. Asserting them by name
// means a layout change fails here rather than as a puzzling miss elsewhere.
describe('file structure', () => {
  it('separates every block by a single blank line', () => {
    const csv = windowReportCsv(reportInput());
    expect(csv).toContain('\n\n');
    for (const b of csv.split('\n\n')) {
      expect(b.startsWith('\n')).toBe(false);
      expect(b.trimEnd().length).toBeGreaterThan(0);
    }
  });

  it('ends with exactly one trailing newline', () => {
    const csv = windowReportCsv(reportInput());
    expect(csv.endsWith('\n')).toBe(true);
    expect(csv.endsWith('\n\n')).toBe(false);
  });
});

describe('reportFilename', () => {
  it('extends the bucket export convention with a range', () => {
    expect(reportFilename('2026-07-12', '2026-08-10')).toBe('usage-2026-07-12_2026-08-10.csv');
  });
});

describe('usage blocks', () => {
  it('names the time block after the window grain', () => {
    for (const grain of ['hour', 'day', 'week', 'month'] as const) {
      const csv = windowReportCsv(reportInput({ grain, time: [usageRow({ key: '2026-07-12' })] }));
      expect(block(csv, grain)[0]).toBe(`${grain},${TIME_COLS_FOR_TEST}`);
      expect(block(csv, grain)[1]).toContain('2026-07-12,100,50,800,50,1000');
    }
  });

  // Sessions are counted distinct per window, so a Session spanning days is
  // counted in each day it touches. Every other column in the time block adds
  // up down the page; this one does not, and a column that cannot be summed
  // has no business sitting in the block a spreadsheet sums.
  it('omits sessions from the time block while the whole-window blocks keep it', () => {
    const csv = windowReportCsv(
      reportInput({
        time: [usageRow({ key: '2026-07-12' })],
        sources: [usageRow({ key: 'claude' })],
      }),
    );
    expect(block(csv, 'day')[0].split(',')).not.toContain('sessions');
    expect(block(csv, 'source')[0].split(',')).toContain('sessions');
    expect(block(csv, 'window')[0].split(',')).toContain('sessions');
    // The cells shift with the header rather than leaving a hole: requests is
    // the last count before the hit rate.
    expect(block(csv, 'day')[1]).toBe('2026-07-12,100,50,800,50,1000,7,0.8421,1.500000,exact,0,false');
  });

  it('scopes a Model row to the tool that ran it', () => {
    const csv = windowReportCsv(reportInput({ models: [usageRow({ key: 'claude-opus-5', source: 'claude' })] }));
    expect(block(csv, 'model')[0]).toBe(`model,source,${USAGE_COLS_FOR_TEST}`);
    expect(block(csv, 'model')[1].startsWith('claude-opus-5,claude,')).toBe(true);
  });

  it('writes Source and Project blocks without a source column', () => {
    const csv = windowReportCsv(
      reportInput({ sources: [usageRow({ key: 'claude' })], projects: [usageRow({ key: '/Users/me/dev' })] }),
    );
    expect(block(csv, 'source')[0]).toBe(`source,${USAGE_COLS_FOR_TEST}`);
    expect(block(csv, 'project')[0]).toBe(`project,${USAGE_COLS_FOR_TEST}`);
  });

  it('quotes a Project path holding a comma or a quote', () => {
    const csv = windowReportCsv(
      reportInput({ projects: [usageRow({ key: '/Users/me/a,b' }), usageRow({ key: '/Users/me/say "hi"' })] }),
    );
    const rows = block(csv, 'project');
    expect(rows[1].startsWith('"/Users/me/a,b",')).toBe(true);
    expect(rows[2].startsWith('"/Users/me/say ""hi""",')).toBe(true);
  });

  it('omits a block entirely when the window has no rows for it', () => {
    const csv = windowReportCsv(reportInput());
    expect(block(csv, 'source')).toEqual([]);
    expect(block(csv, 'model')).toEqual([]);
    expect(block(csv, 'project')).toEqual([]);
    expect(block(csv, 'day')).toEqual([]);
  });

  // SeriesPoint carries `cost: number` with a separate hasUnpriced flag, so a
  // fully-Unpriced bucket cannot say "unavailable" from the data alone. The
  // caller (Task 4) resolves it to cost: null before serializing; this pins the
  // file's side of that contract — a 0 that means "unknown" never appears.
  it('writes an empty cost for a bucket the caller marked unavailable', () => {
    const csv = windowReportCsv(
      reportInput({ time: [usageRow({ key: '2026-07-12', cost: null, hasUnpriced: true })] }),
    );
    const row = block(csv, 'day')[1];
    expect(row).toContain(',,unavailable,');
    expect(row).not.toContain(',0.000000,');
  });

  it('writes cache_estimated on time rows the same way as Source rows', () => {
    const csv = windowReportCsv(
      reportInput({
        time: [usageRow({ key: '2026-07-12', cacheEstimated: false })],
        sources: [usageRow({ key: 'claude', cacheEstimated: true })],
      }),
    );
    const lastCell = (row: string) => row.slice(row.lastIndexOf(',') + 1);
    expect(lastCell(block(csv, 'day')[1])).toBe('false');
    expect(lastCell(block(csv, 'source')[1])).toBe('true');
  });
});

describe('context blocks', () => {
  const ctx = () =>
    reportInput({
      ctxCategories: [
        { source: 'claude', category: 'messages', estTokens: 900, basis: 'exact' },
        { source: 'claude', category: 'system', estTokens: 60, basis: 'exact' },
        { source: 'claude', category: 'reasoning', estTokens: 40, basis: 'exact' },
        { source: 'claude', category: 'toolcalls', estTokens: 500, basis: 'estimated' },
        { source: 'claude', category: 'agents', estTokens: 120, basis: 'estimated' },
        { source: 'claude', category: 'mcp', estTokens: 80, basis: 'estimated' },
        { source: 'claude', category: 'skills', estTokens: 30, basis: 'estimated' },
        { source: 'codex', category: 'messages', estTokens: 300, basis: 'exact' },
      ],
      ctxTools: [{ source: 'claude', category: 'File', name: 'Read', estTokens: 200, calls: 40 }],
      ctxMcp: [{ source: 'claude', name: 'chrome-devtools', estTokens: 80, calls: 12 }],
      ctxSkills: [{ source: 'claude', name: 'playground:playground', estTokens: 30, uses: 3 }],
      // cmd carries the whole two-word signature, exactly as exec_class emits
      // it and as the Bash drill-down displays it — not the argument alone.
      ctxExec: [{ source: 'claude', exe: 'git', cmd: 'git commit', estTokens: 90, calls: 25 }],
    });

  it('stacks every reporting Source rather than only the selected one', () => {
    const rows = block(windowReportCsv(ctx()), 'context');
    expect(rows[0]).toBe('context,source,est_tokens,basis');
    expect(rows).toContain('messages,claude,900,exact');
    expect(rows).toContain('messages,codex,300,exact');
  });

  it('marks the exact partition apart from the overlapping estimates', () => {
    const rows = block(windowReportCsv(ctx()), 'context');
    expect(rows).toContain('system,claude,60,exact');
    expect(rows).toContain('reasoning,claude,40,exact');
    for (const category of ['toolcalls', 'agents', 'mcp', 'skills']) {
      expect(rows.some((r) => r.startsWith(`${category},claude,`) && r.endsWith(',estimated'))).toBe(true);
    }
  });

  it('emits no total row in any Context block', () => {
    const csv = windowReportCsv(ctx());
    for (const first of ['context', 'tool', 'mcp_server', 'skill', 'bash']) {
      expect(block(csv, first).some((r) => r.toLowerCase().startsWith('total,'))).toBe(false);
    }
    // Counting is the assertion that holds: a total row appended under any
    // other label — or with the empty first cell a fold row carries — would
    // pass the check above. Header plus exactly the rows handed in, no more.
    const input = ctx();
    for (const [first, rows] of [
      ['context', input.ctxCategories],
      ['tool', input.ctxTools],
      ['mcp_server', input.ctxMcp],
      ['skill', input.ctxSkills],
      ['bash', input.ctxExec],
    ] as const) {
      expect(block(csv, first)).toHaveLength(rows.length + 1);
    }
  });

  // esc guards twelve text cells across these five blocks, and every other
  // fixture here is comma-free, so without this its removal from any of them
  // passes the suite. `sed -i 's/a,b/c/' f` reduces to the cmd below, which is
  // an ordinary command that carries a comma into the first column.
  it("quotes a comma in a Context first column, and in the Model block's source", () => {
    const csv = windowReportCsv(
      reportInput({
        ctxExec: [{ source: 'claude', exe: 'sed', cmd: 'sed s/a,b/c/', estTokens: 12, calls: 1 }],
        models: [{ ...usageRow({ key: 'gpt-5-codex' }), source: 'codex,cli' }],
      }),
    );
    expect(block(csv, 'bash')[1]).toBe('"sed s/a,b/c/",claude,sed,12,1');
    expect(block(csv, 'model')[1].startsWith('gpt-5-codex,"codex,cli",')).toBe(true);
  });

  it('writes tools with their category, MCP servers with calls, skills with uses', () => {
    const csv = windowReportCsv(ctx());
    expect(block(csv, 'tool')[0]).toBe('tool,source,category,est_tokens,calls');
    expect(block(csv, 'tool')[1]).toBe('Read,claude,File,200,40');
    expect(block(csv, 'mcp_server')[0]).toBe('mcp_server,source,est_tokens,calls');
    expect(block(csv, 'mcp_server')[1]).toBe('chrome-devtools,claude,80,12');
    expect(block(csv, 'skill')[0]).toBe('skill,source,est_tokens,uses');
    expect(block(csv, 'skill')[1]).toBe('playground:playground,claude,30,3');
  });

  it('writes a Bash row as the displayed signature plus its executable', () => {
    const rows = block(windowReportCsv(ctx()), 'bash');
    expect(rows[0]).toBe('bash,source,exe,est_tokens,calls');
    expect(rows[1]).toBe('git commit,claude,git,90,25');
  });

  it('yields no rows at all for a Source that cannot attribute Context', () => {
    const csv = windowReportCsv(reportInput());
    for (const first of ['context', 'tool', 'mcp_server', 'skill', 'bash']) {
      expect(block(csv, first)).toEqual([]);
    }
  });
});
