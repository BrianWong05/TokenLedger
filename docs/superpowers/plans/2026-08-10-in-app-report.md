# In-App Window Report Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an Export control to the Overview toolbar that writes the currently selected window — tokens, Cost, per-Source/Model/Project breakdowns and the Context breakdown — to one CSV.

**Architecture:** A pure serializer (`src/overview/reportCsv.ts`) turns a plain `ReportInput` into a CSV string. A pure store selector (`selectReportInput`) builds that input from the snapshot **and the already-computed `OverviewView`**, so the file is derived from the same object that rendered the screen and cannot disagree with it. `Overview.tsx` hands the string to the existing `ExportPort.saveCsv`, which the Rust `save_csv` command already backs. No Rust changes.

**Tech Stack:** TypeScript, React 18, Vitest, Tauri v2 (existing `save_csv` command only).

## Global Constraints

- **An unknown is an empty cell, never `0`.** Applies to every column in every block. From the spec: "Unpriced is never `$0`, an unattributable Context category is '—' and not zero."
- **All money stays USD.** `cost_usd` is USD in every row. Display Currency appears only as `display_currency` / `display_rate` header lines. Source: CONTEXT.md Display Currency — "Nothing stored ever leaves USD".
- **CSV column and block names are English and untranslated.** Only `overview.export`, `overview.exporting` and `overview.exportFailed` are localized, in both `en` and `zh-Hant`.
- **No Context block emits a total row.** Tool calls, subagents, MCP and skills are overlapping subsets of messages and do not sum to a whole.
- **Nothing is refetched during export.** The serializer runs over state already in the store.
- Number formatting, fixed for the whole file: integers as `String(n)`; `cache_hit_rate` as `toFixed(4)`; `cost_usd` as `toFixed(6)` or empty; booleans as `true` / `false`.
- Run the frontend suite with `npm test`. Type-check with `npm run build`.

**Two spec ambiguities this plan resolves.** The spec's Context tables can be read two ways; these are the bindings:

1. Context category rows use header `context,source,est_tokens,basis` — the first column **holds the category name**, exactly as the `day` column holds a date. There is no separate `category` column.
2. The Bash block uses header `bash,source,exe,est_tokens,calls` — the first column holds the **displayed two-word signature** (`exe` + `' '` + `cmd`, trimmed), with `exe` repeated as its own column so rows can be grouped by executable.

---

### Task 1: Serializer scaffolding, header block, summary block, filename

**Files:**
- Create: `src/overview/reportCsv.ts`
- Test: `src/overview/reportCsv.test.ts`

**Interfaces:**
- Consumes: `Granularity` from `./data`.
- Produces: `ReportUsageRow`, `ReportCtxCategory`, `ReportCtxTool`, `ReportCtxMcp`, `ReportCtxSkill`, `ReportCtxExec`, `ReportInput`, `windowReportCsv(input: ReportInput): string`, `reportFilename(fromIso: string, toIso: string): string`. Tasks 2 and 3 extend `windowReportCsv`; Task 4 constructs `ReportInput`; Task 5 calls `windowReportCsv` and `reportFilename`.

- [ ] **Step 1: Write the failing test**

Create `src/overview/reportCsv.test.ts`:

```tsx
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
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `npx vitest run src/overview/reportCsv.test.ts`
Expected: FAIL — `Failed to resolve import "./reportCsv"`.

- [ ] **Step 3: Write the minimal implementation**

Create `src/overview/reportCsv.ts`:

```ts
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
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `npx vitest run src/overview/reportCsv.test.ts`
Expected: PASS — 9 tests.

- [ ] **Step 5: Commit**

```bash
git add src/overview/reportCsv.ts src/overview/reportCsv.test.ts
git commit -m "feat(report): serialize the window's header and summary blocks

An unknown is an empty cell, never 0 — the CSV form of the rule the UI
already follows, so a spreadsheet's SUM() cannot absorb a gap."
```

---

### Task 2: Time, Source, Model and Project blocks

**Files:**
- Modify: `src/overview/reportCsv.ts`
- Test: `src/overview/reportCsv.test.ts`

**Interfaces:**
- Consumes: `ReportInput`, `ReportUsageRow`, and the module-private `usageCells`, `USAGE_COLS`, `esc` from Task 1.
- Produces: no new exports. `windowReportCsv` emits four more blocks, keyed by first column `hour`|`day`|`week`|`month`, `source`, `model`, `project`.

- [ ] **Step 1: Write the failing test**

Append to `src/overview/reportCsv.test.ts`:

```tsx
describe('usage blocks', () => {
  it('names the time block after the window grain', () => {
    for (const grain of ['hour', 'day', 'week', 'month'] as const) {
      const csv = windowReportCsv(reportInput({ grain, time: [usageRow({ key: '2026-07-12' })] }));
      expect(block(csv, grain)[0]).toBe(`${grain},${USAGE_COLS_FOR_TEST}`);
      expect(block(csv, grain)[1]).toContain('2026-07-12,100,50,800,50,1000');
    }
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
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `npx vitest run src/overview/reportCsv.test.ts`
Expected: FAIL — the new cases fail because `block(csv, 'day')` returns `[]`.

- [ ] **Step 3: Write the minimal implementation**

In `src/overview/reportCsv.ts`, add above `windowReportCsv`:

```ts
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
```

Replace the `return` of `windowReportCsv` with:

```ts
  const blocks = [
    header.join('\n'),
    summary.join('\n'),
    usageBlock(input.grain, input.time),
    usageBlock('source', input.sources),
    usageBlock('model', input.models, true),
    usageBlock('project', input.projects),
  ];
  return blocks.filter((b): b is string => b !== null).join('\n\n') + '\n';
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `npx vitest run src/overview/reportCsv.test.ts`
Expected: PASS — 15 tests.

- [ ] **Step 5: Commit**

```bash
git add src/overview/reportCsv.ts src/overview/reportCsv.test.ts
git commit -m "feat(report): add the time, Source, Model and Project blocks

Each block is identified by its first column and omitted when it has no
rows, so a reader never has to tell 'no rows' from 'no data'."
```

---

### Task 3: Context blocks

**Files:**
- Modify: `src/overview/reportCsv.ts`
- Test: `src/overview/reportCsv.test.ts`

**Interfaces:**
- Consumes: `ReportCtxCategory`, `ReportCtxTool`, `ReportCtxMcp`, `ReportCtxSkill`, `ReportCtxExec` from Task 1.
- Produces: no new exports. `windowReportCsv` emits five more blocks keyed `context`, `tool`, `mcp_server`, `skill`, `bash`.

- [ ] **Step 1: Write the failing test**

Append to `src/overview/reportCsv.test.ts`:

```tsx
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
      ctxSkills: [{ source: 'claude', name: 'superpowers:brainstorming', estTokens: 30, uses: 3 }],
      ctxExec: [{ source: 'claude', exe: 'git', cmd: 'commit', estTokens: 90, calls: 25 }],
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
  });

  it('writes tools with their category, MCP servers with calls, skills with uses', () => {
    const csv = windowReportCsv(ctx());
    expect(block(csv, 'tool')[0]).toBe('tool,source,category,est_tokens,calls');
    expect(block(csv, 'tool')[1]).toBe('Read,claude,File,200,40');
    expect(block(csv, 'mcp_server')[0]).toBe('mcp_server,source,est_tokens,calls');
    expect(block(csv, 'mcp_server')[1]).toBe('chrome-devtools,claude,80,12');
    expect(block(csv, 'skill')[0]).toBe('skill,source,est_tokens,uses');
    expect(block(csv, 'skill')[1]).toBe('superpowers:brainstorming,claude,30,3');
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
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `npx vitest run src/overview/reportCsv.test.ts`
Expected: FAIL — `block(csv, 'context')` returns `[]`.

- [ ] **Step 3: Write the minimal implementation**

In `src/overview/reportCsv.ts`, add above `windowReportCsv`:

```ts
// Context blocks carry no total row, deliberately: tool calls, subagents, MCP
// and skills are overlapping subsets of messages and do not sum to a whole, so
// a total would invite exactly the reading the app declines to offer.
function ctxBlock(header: string, rows: string[]): string | null {
  return rows.length === 0 ? null : [header, ...rows].join('\n');
}
```

Extend the `blocks` array in `windowReportCsv`, after `usageBlock('project', input.projects)`:

```ts
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
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `npx vitest run src/overview/reportCsv.test.ts`
Expected: PASS — 21 tests.

- [ ] **Step 5: Commit**

```bash
git add src/overview/reportCsv.ts src/overview/reportCsv.test.ts
git commit -m "feat(report): add the Context blocks, every Source stacked

No block emits a total row: tool calls, subagents, MCP and skills are
overlapping subsets of messages and do not sum to a whole."
```

---

### Task 4: Store — per-Source usage rows and the `selectReportInput` selector

**Files:**
- Modify: `src/overview/overviewStore.ts` (snapshot interface, initial state, patch-key list at `:125-126`, `runReload` at `:343-363`, new selector after `selectView`)
- Test: `src/overview/overviewStore.test.ts`

**Interfaces:**
- Consumes: `ReportInput`, `ReportUsageRow`, `ReportCtxCategory` from `./reportCsv`; `ctxTotals`, `toolTree`, `mcpBars`, `skillBars`, `windowOf` from `./data`; `OverviewView`, `OverviewSnapshot` from this file; `Settings` from `../bindings/Settings`; `BreakdownRow`, `SeriesPoint` from `../bindings/`.
- Produces: `sourceRows: BreakdownRow[]` on `OverviewSnapshot`, and
  `selectReportInput(s: OverviewSnapshot, view: OverviewView, settings: Settings, now?: Date): ReportInput`.
  Task 5 calls it with the same `view` it renders from.

- [ ] **Step 1: Write the failing test**

Append to `src/overview/overviewStore.test.ts`, reusing the file's existing store/ledger helpers for construction. The assertions are what matter:

```tsx
import { selectReportInput, selectView } from './overviewStore';
import type { Settings } from '../bindings/Settings';

const SETTINGS: Settings = {
  theme: 'system', language: 'en', currency: 'USD', usdRate: 1,
  launchAtLogin: false, autoCheckUpdates: true, firstRunDone: true,
};

describe('selectReportInput', () => {
  it('loads per-Source usage rows with the rest of the window', async () => {
    const { store, ledger } = await mountStoreWithUsage();
    expect(ledger.breakdownCalls.map(([by]) => by)).toContain('tool');
    expect(store.getSnapshot().sourceRows.length).toBeGreaterThan(0);
  });

  it('derives Context for every reporting Source, not only the selected one', async () => {
    const { store } = await mountStoreWithCtxFor(['claude', 'codex']);
    const s = { ...store.getSnapshot(), selected: 'claude' as const };
    const input = selectReportInput(s, selectView(s, NOW), SETTINGS, NOW);
    expect(new Set(input.ctxCategories.map((c) => c.source))).toEqual(new Set(['claude', 'codex']));
  });

  it('leaves out a Source present in the window that reports no Context', async () => {
    // grok has usage in the window but attributes no Context category.
    const { store } = await mountStoreWithCtxFor(['claude'], { alsoSeedUsageFor: ['grok'] });
    const s = store.getSnapshot();
    const input = selectReportInput(s, selectView(s, NOW), SETTINGS, NOW);
    expect(input.ctxCategories.some((c) => c.source === 'grok')).toBe(false);
    expect(input.sources.some((r) => r.key === 'grok')).toBe(true);
  });

  it('resolves a fully-Unpriced bucket to an unavailable cost rather than zero', async () => {
    const { store, unpricedDay } = await mountStoreWithUnpricedDay();
    const s = store.getSnapshot();
    const input = selectReportInput(s, selectView(s, NOW), SETTINGS, NOW);
    expect(input.time.find((r) => r.key === unpricedDay)?.cost).toBe(null);
  });

  it('carries the Display Currency without moving figures off USD', async () => {
    const { store } = await mountStoreWithUsage();
    const s = store.getSnapshot();
    const view = selectView(s, NOW);
    const aud = selectReportInput(s, view, { ...SETTINGS, currency: 'AUD', usdRate: 1.52 }, NOW);
    expect(aud.displayCurrency).toBe('AUD');
    expect(aud.usdRate).toBe(1.52);

    const usd = selectReportInput(s, view, SETTINGS, NOW);
    expect(usd.displayCurrency).toBe(null);
    expect(usd.usdRate).toBe(null);
    expect(usd.summary.cost).toBe(aud.summary.cost);
  });

  it("takes a Total window's dates from the Ledger's own extent", async () => {
    const { store } = await mountStoreWithUsage();
    const s = { ...store.getSnapshot(), range: 'total' as const };
    const input = selectReportInput(s, selectView(s, NOW), SETTINGS, NOW);
    expect(input.fromIso).toBe(s.firstIso);
    expect(input.toIso).toBe(s.lastIso);
  });

  it('marks the window a floor when an Unreadable Artifact could reach it', async () => {
    const { store } = await mountStoreWithUnreadableArtifact();
    const s = store.getSnapshot();
    const input = selectReportInput(s, selectView(s, NOW), SETTINGS, NOW);
    expect(input.tokensBasis).toBe('floor');
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `npx vitest run src/overview/overviewStore.test.ts`
Expected: FAIL — `selectReportInput` is not exported from `./overviewStore`.

- [ ] **Step 3: Add `sourceRows` to the store**

Add `sourceRows: BreakdownRow[];` to the snapshot interface beside `modelRows`, `sourceRows: []` to the initial state at `:137-139`, and `'sourceRows'` to the patch-key list at `:125-126`.

Extend `runReload`'s `Promise.all` at `:343-353` with a ninth call after `hourDay ? L.series(filters, 'hour') : null,`:

```ts
      L.breakdown('tool', filters),
```

and widen the destructure and patch at `:354-357`:

```ts
      .then(([summary, modelRows, projectRows, ctxResources, ctxBuckets, ctxToolRows, ctxSkillRows, ctxExecRows, hour, sourceRows]) =>
        land(() =>
          this.patch({
            summary, modelRows, projectRows, ctxResources, ctxBuckets, ctxToolRows, ctxSkillRows, ctxExecRows, sourceRows,
```

- [ ] **Step 4: Write the selector**

Add after `selectView` in `src/overview/overviewStore.ts`:

```ts
// ---- the window report's input ----

// A BreakdownRow in the report's shape. `key: null` is the Model breakdown's
// Unattributed Usage row; every other breakdown names its key.
function reportRow(row: BreakdownRow, withSource: boolean): ReportUsageRow {
  return {
    key: row.key ?? 'Unattributed usage',
    ...(withSource ? { source: row.source ?? '' } : {}),
    inputTokens: row.inputTokens,
    outputTokens: row.outputTokens,
    cacheReadTokens: row.cacheReadTokens,
    cacheWriteTokens: row.cacheWriteTokens,
    totalTokens: row.totalTokens,
    requests: row.requests,
    sessions: row.convs,
    cost: row.cost,
    hasUnpriced: row.hasUnpriced,
    unattributedTokens: row.unattributedTokens,
    cacheEstimated: row.cacheEstimated,
  };
}

// The time block: one row per bucket, summed across Sources as the Trend sums it.
//
// SeriesPoint carries `cost: number` with a separate hasUnpriced flag, so
// unlike Summary and BreakdownRow it cannot express "no priced tokens". Rather
// than refetch a summary per bucket — which would break the no-refetch property
// this design rests on — a bucket with unpriced usage and no cost resolves to
// null. The lean is deliberate: a bucket genuinely worth $0 that also holds an
// Unpriced Model reads unavailable, erring toward admitting ignorance rather
// than writing a 0 that means "unknown".
function reportTimeRows(pts: SeriesPoint[]): ReportUsageRow[] {
  const byBucket = new Map<string, ReportUsageRow>();
  for (const p of pts) {
    const row = byBucket.get(p.bucket) ?? {
      key: p.bucket,
      inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0,
      totalTokens: 0, requests: 0, sessions: 0, cost: 0,
      hasUnpriced: false, unattributedTokens: 0, cacheEstimated: false,
    };
    row.inputTokens += p.inputTokens;
    row.outputTokens += p.outputTokens;
    row.cacheReadTokens += p.cacheReadTokens;
    row.cacheWriteTokens += p.cacheWriteTokens;
    row.totalTokens += p.totalTokens;
    row.requests += p.requests;
    row.sessions += p.convs;
    row.cost = (row.cost ?? 0) + p.cost;
    row.hasUnpriced ||= p.hasUnpriced;
    row.unattributedTokens += p.unattributedTokens;
    byBucket.set(p.bucket, row);
  }
  return [...byBucket.values()]
    .map((row) => (row.hasUnpriced && row.cost === 0 ? { ...row, cost: null } : row))
    .sort((a, b) => a.key.localeCompare(b.key));
}

const CTX_EXACT = ['messages', 'system', 'reasoning'] as const;
const CTX_ESTIMATED = ['toolcalls', 'agents', 'mcp', 'skills'] as const;

export function selectReportInput(
  s: OverviewSnapshot,
  view: OverviewView,
  settings: Settings,
  now: Date = new Date(),
): ReportInput {
  const win = windowOf(s.range, s.from, s.to, now);
  const summary = s.summary;
  // A Total window is unbounded, so it reports the Ledger's own extent.
  const fromIso = win.fromIso ?? s.firstIso;
  const toIso = win.toIso ?? s.lastIso;

  const ctxCategories: ReportCtxCategory[] = [];
  const ctxTools: ReportInput['ctxTools'] = [];
  const ctxMcp: ReportInput['ctxMcp'] = [];
  const ctxSkills: ReportInput['ctxSkills'] = [];
  const ctxExec: ReportInput['ctxExec'] = [];

  // Every Source present in the window, so the file does not depend on which
  // card happened to be selected. A Source with usage but no Context capability
  // yields no rows rather than a row of zeros.
  for (const key of Object.keys(view.toolTotals).sort()) {
    const ctx = ctxTotals(view.rpts, key);
    for (const category of CTX_EXACT) {
      const v = ctx[category];
      if (v !== null) ctxCategories.push({ source: key, category, estTokens: v, basis: 'exact' });
    }
    for (const category of CTX_ESTIMATED) {
      const v = ctx[category];
      if (v !== null) ctxCategories.push({ source: key, category, estTokens: v, basis: 'estimated' });
    }
    const toolRows = s.ctxToolRows.filter((r) => r.source === key);
    for (const cat of toolTree(toolRows, ctx.toolcalls)) {
      for (const leaf of cat.tools) {
        ctxTools.push({ source: key, category: cat.label, name: leaf.name, estTokens: leaf.tokens, calls: leaf.calls });
      }
    }
    for (const m of mcpBars(toolRows, ctx.mcp)) {
      ctxMcp.push({ source: key, name: m.name, estTokens: m.tokens, calls: m.calls });
    }
    for (const sk of skillBars(s.ctxSkillRows, key)) {
      ctxSkills.push({ source: key, name: sk.name, estTokens: sk.tokens, uses: sk.uses });
    }
    for (const e of s.ctxExecRows.filter((r) => r.source === key)) {
      ctxExec.push({ source: key, exe: e.exe, cmd: e.cmd, estTokens: e.estTokens, calls: e.calls });
    }
  }

  return {
    generatedIso: now.toISOString(),
    fromIso,
    toIso,
    grain: view.per,
    tokensBasis: view.unreadable.length > 0 ? 'floor' : 'exact',
    // USD needs no conversion note; anything else does, and the figures below
    // stay USD either way (CONTEXT.md Display Currency).
    displayCurrency: settings.currency === 'USD' ? null : settings.currency,
    usdRate: settings.currency === 'USD' ? null : settings.usdRate,
    summary: {
      key: `${fromIso} .. ${toIso}`,
      inputTokens: summary?.inputTokens ?? 0,
      outputTokens: summary?.outputTokens ?? 0,
      cacheReadTokens: summary?.cacheReadTokens ?? 0,
      cacheWriteTokens: summary?.cacheWriteTokens ?? 0,
      totalTokens: summary?.totalTokens ?? 0,
      requests: summary?.requests ?? 0,
      sessions: summary?.convs ?? 0,
      cost: summary?.cost ?? null,
      hasUnpriced: summary?.hasUnpriced ?? false,
      unattributedTokens: summary?.unattributedTokens ?? 0,
      cacheEstimated: (summary?.cacheEstimatedModels.length ?? 0) > 0,
    },
    unpricedModels: summary?.unpricedModels ?? [],
    cacheEstimatedModels: summary?.cacheEstimatedModels ?? [],
    time: reportTimeRows(view.rpts),
    sources: s.sourceRows.map((r) => reportRow(r, false)),
    models: s.modelRows.map((r) => reportRow(r, true)),
    projects: s.projectRows.map((r) => reportRow(r, false)),
    ctxCategories,
    ctxTools,
    ctxMcp,
    ctxSkills,
    ctxExec,
  };
}
```

Add the imports this needs at the top of the file: `ReportCtxCategory`, `ReportInput`, `ReportUsageRow` from `./reportCsv`, `Settings` from `../bindings/Settings`, and `BreakdownRow` if not already imported.

- [ ] **Step 5: Run the test to verify it passes**

Run: `npx vitest run src/overview/overviewStore.test.ts`
Expected: PASS.

- [ ] **Step 6: Run the whole frontend suite for regressions**

Run: `npm test`
Expected: PASS. The ninth `breakdown` call must not break existing store tests — if a fake Ledger asserts an exact call count or a fixed `Promise.all` arity, update it to expect `tool` as well.

- [ ] **Step 7: Commit**

```bash
git add src/overview/overviewStore.ts src/overview/overviewStore.test.ts
git commit -m "feat(report): build the report input from the rendered view

selectReportInput takes the OverviewView the screen renders from, so the
file is derived from the same object rather than a second fetch. Context
is derived per Source present in the window, not for the selected card."
```

---

### Task 5: The Export button, its errors, its strings, and the README narrowing

**Files:**
- Modify: `src/overview/useOverview.ts` (the returned shell model at `:106+`)
- Modify: `src/overview/Overview.tsx` (React import at `:1`, destructure at `:104-114`, toolbar at `:179-206`, error band at `:209-213`)
- Modify: `src/overview/overview.css` (the existing `.tt-rescan` rule at `:45-65`)
- Modify: `src/lib/strings/overview.ts`
- Modify: `README.md` (the "Reporting a window of the Ledger" section)
- Test: `src/overview/Overview.test.tsx`

**Interfaces:**
- Consumes: `windowReportCsv`, `reportFilename` (Task 1), `selectReportInput` (Task 4), the existing `ExportPort` from `./export`.
- Produces: `reportInput: (settings: Settings) => ReportInput` on `useOverview`'s return. Nothing later consumes it.

**Read this before writing code.** `Overview.tsx` has **no** `store` or `view` in scope. `useOverview` flattens the snapshot and the view into a shell model and returns only named fields (`:106-131`) — the file's own comment says "this shell only renders the model the hook hands back". So `selectReportInput` cannot be called from the component directly. `useOverview` exposes it instead, as a callback taking `settings` (which lives in `SettingsContext`, consumed by the component at `Overview.tsx:48`, not by the hook).

- [ ] **Step 1: Write the failing test**

Append to `src/overview/Overview.test.tsx`. Copy `makeFakeExporter` from `TrendModal.test.tsx:67-70`, adding the cancelled and rejecting variants:

```tsx
// Fake file-save port: records (filename, contents) instead of opening a dialog.
function makeFakeExporter(result: 'written' | 'cancelled' | 'fails' = 'written') {
  const calls: [string, string][] = [];
  return {
    calls,
    saveCsv: (filename: string, contents: string) => {
      calls.push([filename, contents]);
      if (result === 'fails') return Promise.reject(new Error('disk full'));
      return Promise.resolve(result === 'written');
    },
  };
}

describe('Export', () => {
  it('writes the window under the range it covers', async () => {
    const { container, exporter } = await mountOverview();
    await click(container.querySelector('.tt-export')!);
    expect(exporter.calls).toHaveLength(1);
    const [filename, contents] = exporter.calls[0];
    expect(filename).toMatch(/^usage-\d{4}-\d{2}-\d{2}_\d{4}-\d{2}-\d{2}\.csv$/);
    expect(contents.startsWith('tokenledger_report,1\n')).toBe(true);
  });

  // The property the architecture exists for: the file is a function of the
  // state that rendered, so it cannot report a total the screen never showed.
  it('reports the same total the headline shows', async () => {
    const { container, exporter } = await mountOverview();
    const headline = Number(container.querySelector('.tt-headline-total')!.getAttribute('data-total'));
    await click(container.querySelector('.tt-export')!);
    const [, contents] = exporter.calls[0];
    const summaryRow = contents.split('\n\n').find((b) => b.startsWith('window,input_tokens'))!.split('\n')[1];
    expect(Number(summaryRow.split(',')[5])).toBe(headline);
  });

  it('says nothing when the save dialog is cancelled', async () => {
    const { container } = await mountOverview({ exporter: makeFakeExporter('cancelled') });
    await click(container.querySelector('.tt-export')!);
    expect(container.querySelector('.tt-error')).toBe(null);
  });

  it('surfaces a failed write in the error band', async () => {
    const { container } = await mountOverview({ exporter: makeFakeExporter('fails') });
    await click(container.querySelector('.tt-export')!);
    expect(container.querySelector('.tt-error')?.textContent).toContain('disk full');
  });

  it('offers Export for an empty window, which costs $0.00 rather than nothing', async () => {
    const { container } = await mountOverview({ ledger: makeFakeLedger({ dayPoints: [] }) });
    expect(container.querySelector<HTMLButtonElement>('.tt-export')!.disabled).toBe(false);
  });
});
```

Wire `exporter` through the file's existing mount helper the way `TrendModal.test.tsx:110` does — `<Overview ports={{ ledger, clock: systemClock, pricing: makeFakePricing(), export: exporter }} />` — and return `exporter` from the helper. If the headline element carries no `data-total`, read the rendered figure the way the file's existing headline test does instead of adding an attribute.

- [ ] **Step 2: Run the test to verify it fails**

Run: `npx vitest run src/overview/Overview.test.tsx`
Expected: FAIL — `container.querySelector('.tt-export')` is `null`.

- [ ] **Step 3: Add the strings**

In `src/lib/strings/overview.ts`, beside `'overview.trend.exportCsv'` in the `en` map:

```ts
    'overview.export': 'Export',
    'overview.exporting': 'Exporting…',
    'overview.exportFailed': 'Export failed',
```

and at the matching place in `zh-Hant`:

```ts
    'overview.export': '匯出',
    'overview.exporting': '匯出中…',
    'overview.exportFailed': '匯出失敗',
```

- [ ] **Step 4: Expose the report input from `useOverview`**

In `src/overview/useOverview.ts`, the hook already holds both `snap` and `view`
(it reads `view.rangeLabel` at `:128`). Add a callback beside the other
`useCallback`s, above the `return`:

```ts
  // The report's input, built from the same snapshot and view this hook renders
  // from — which is what keeps the exported file from disagreeing with the
  // screen. `settings` arrives from the component: Display Currency lives in
  // SettingsContext, which the shell consumes and this hook does not.
  const reportInput = useCallback(
    (settings: Settings) => selectReportInput(snap, view, settings),
    [snap, view],
  );
```

and add `reportInput,` to the returned object. Import `selectReportInput` from
`./overviewStore` and `type Settings` from `../bindings/Settings`.

- [ ] **Step 5: Write the component**

In `src/overview/Overview.tsx`, add `useEffect` to the React import at `:1` —
the file currently imports only `Fragment, useCallback, useRef, useState`:

```tsx
import { Fragment, useCallback, useEffect, useRef, useState } from 'react';
```

Add `reportInput` to the `useOverview` destructure at `:104-114`, and state
beside the other local state:

```tsx
  const [exporting, setExporting] = useState(false);
  const [exportError, setExportError] = useState<string | null>(null);
```

Add the handler near `refresh`:

```tsx
  // Serializes the state that is already rendered — no refetch, so a scan
  // landing mid-export cannot produce a file matching neither the screen
  // before it nor the screen after.
  const exportWindow = async () => {
    setExporting(true);
    setExportError(null);
    try {
      const input = reportInput(settings);
      await exporter.saveCsv(reportFilename(input.fromIso, input.toIso), windowReportCsv(input));
    } catch (e) {
      setExportError(`${t('overview.exportFailed')}: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setExporting(false);
    }
  };
```

Add the button in `tt-toolbar`, after the Rescan button that closes at `:205`:

```tsx
        <button
          type="button"
          className="tt-export"
          onClick={() => void exportWindow()}
          // Not disabled on an empty window: a window with no usage is a
          // legitimate report costing $0.00, the one zero that is a figure.
          disabled={exporting || loading}
          aria-busy={exporting}
        >
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
            <path d="M7 10l5 5 5-5" />
            <path d="M12 15V3" />
          </svg>
          {exporting ? t('overview.exporting') : t('overview.export')}
        </button>
```

Fold `exportError` into the existing band at `:209-213`:

```tsx
      {(scanError || fetchError || exportError) && (
        <div className="tt-error">
          {[scanError, fetchError, exportError].filter(Boolean).join(' · ')}
        </div>
      )}
```

Clear it when the window changes:

```tsx
  useEffect(() => setExportError(null), [range, from, to]);
```

In `src/overview/overview.css`, add `.tt-export` to the selector lists of the
existing `.tt-rescan` rules at `:45`, `:60` (`:hover:not(:disabled)`) and `:63`
(`:disabled`), rather than duplicating the declarations.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `npx vitest run src/overview/Overview.test.tsx src/overview/overviewStore.test.ts`
Expected: PASS.

- [ ] **Step 7: Narrow the README's report.rs section**

In `README.md`, replace the first paragraph of "Reporting a window of the Ledger" so the button is the answer and the cargo workflow is the headless path:

```markdown
### Reporting a window of the Ledger

In the app, the Overview's **Export** writes the selected window to one CSV —
the Month preset plus Export is a 30-day report. For the same figures without a
GUI (cron, CI, a script), an ignored workflow writes them to a folder of CSVs.
It runs the same queries the Overview does, so Cost, Partial Cost, Unpriced and
Unattributed Usage, and the Unreadable Artifact floor all carry their usual
meaning; the Ledger is opened read-only and never written.
```

Leave the command, the file list and the environment-variable table unchanged.

- [ ] **Step 8: Run the full suite and type-check**

Run: `npm test && npm run build`
Expected: both PASS.

- [ ] **Step 9: Commit**

```bash
git add src/overview/useOverview.ts src/overview/Overview.tsx src/overview/Overview.test.tsx \
  src/lib/strings/overview.ts src/overview/overview.css README.md
git commit -m "feat(overview): export the selected window as one CSV

The Month preset plus Export is a 30-day report. A failed write joins the
existing error band rather than being swallowed; cancelling says nothing.
The README now names the cargo workflow the headless path."
```

---

## Self-Review

**Spec coverage.** Header block, summary, time/Source/Model/Project blocks, all five Context blocks, the never-`0` rule, USD-with-rate, English column names, no Context total, the filename convention, error handling, disabled-while-loading, enabled-when-empty, the anti-drift test, and the README narrowing each map to a task.

**Naming.** The spec's `reportInput()` became `selectReportInput` to match the file's existing `selectView` / `selectProfile` / `selectVisibleTools` convention.

**Type consistency.** `ReportUsageRow.sessions` is fed from `BreakdownRow.convs` and `Summary.convs`; `cost: number | null` is the single representation of an unavailable Cost across Tasks 1, 2 and 4; `windowReportCsv` and `reportFilename` keep the signatures defined in Task 1 through Task 5; `USAGE_COLS` in the implementation and `USAGE_COLS_FOR_TEST` in the test file are asserted equal by the summary-block header test.

**Three claims checked against the code after drafting, and corrected.** The
`.tt-rescan` styles live in `src/overview/overview.css:45-65`, not `src/App.css`.
`Overview.tsx:1` imports `Fragment, useCallback, useRef, useState` — `useEffect`
has to be added. And `Overview.tsx` has neither `store` nor `view` in scope, so
`selectReportInput` is exposed through `useOverview` as a `settings`-taking
callback rather than called from the component; Task 5 carries that as a
read-this-first note.

**Deviation from the spec, carried deliberately.** The spec's Context tables read two ways; the plan header fixes the category block to `context,source,est_tokens,basis` and the Bash block to `bash,source,exe,est_tokens,calls`, stated up front rather than left to the implementer.
