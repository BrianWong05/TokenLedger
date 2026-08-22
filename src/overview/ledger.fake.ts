// In-memory LedgerPort for tests: canned responses, a call log, per-method
// failNext, and a hold/resolve deferral pattern (resolvable out of order) to
// prove the store's stale-response epoch guard.
import type { LedgerPort } from './ledger';
import type {
  Filters,
  ScanStatus,
  SourceUnreadable,
  SeriesPoint,
  Summary,
  BreakdownRow,
  CtxResource,
  CtxBuckets,
  CtxToolRow,
  CtxSkillRow,
  CtxExecRow,
} from '../types';

interface Deferred {
  resolve: (v: unknown) => void;
  reject: (e: unknown) => void;
  args: unknown[];
}

interface Data {
  scan: ScanStatus;
  lastScan: number;
  dayPoints: SeriesPoint[];
  hourPoints: SeriesPoint[];
  summary: Summary;
  modelRows: BreakdownRow[];
  // Source rows (breakdown('tool')); falls back to modelRows so the callers
  // that only care about one list stay a one-field seed.
  toolRows?: BreakdownRow[];
  projectRows: BreakdownRow[];
  ctxResources: CtxResource[];
  ctxBuckets: CtxBuckets[];
  ctxTools: CtxToolRow[];
  ctxSkills: CtxSkillRow[];
  ctxExec: CtxExecRow[];
  // What the export companion reports back; seeded per test.
  exportReport?: string;
  // The persisted per-scan Unreadable Artifact state. Defaults to deriving
  // from scan.sources; seed it explicitly to express the persisted state
  // DIVERGING from this launch's scan verdict — the case the ≥ floor's
  // one-provenance rule exists for.
  unreadableArtifacts?: SourceUnreadable[];
}

export interface FakeLedger extends LedgerPort {
  data: Data;
  calls: Record<string, unknown[][]>;
  failNext(method: string, err: unknown): void;
  hold(method: string): void;
  held(method: string): Deferred[];
  resolveHeld(method: string, index: number, value?: unknown): void;
  emitPricesRebuilt(): void;
}

const EMPTY_SUMMARY: Summary = {
  inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0,
  // cost 0, never null: a window with no usage in it cost zero (queries::summary).
  totalTokens: 0, requests: 0, cost: 0, hasUnpriced: false, unattributedTokens: 0,
  unpricedModels: [], cacheEstimatedModels: [], cacheHitRate: 0, convs: 0,
};

export function makeFakeLedger(seed: Partial<Data> = {}): FakeLedger {
  const data: Data = {
    scan: { sources: [], scannedAt: 0, ingestRev: 0 },
    lastScan: 0,
    dayPoints: [], hourPoints: [], summary: EMPTY_SUMMARY,
    modelRows: [], projectRows: [], ctxResources: [], ctxBuckets: [],
    ctxTools: [], ctxSkills: [], ctxExec: [],
    ...seed,
  };
  const calls: Record<string, unknown[][]> = {
    scan: [], lastScan: [], unreadableArtifacts: [], exportAntigravity: [], series: [], summary: [], breakdown: [],
    ctxResources: [], ctxBuckets: [], ctxTools: [], ctxSkills: [], ctxExec: [],
  };
  const fails = new Map<string, unknown>();
  const holds = new Set<string>();
  const heldMap: Record<string, Deferred[]> = {};
  const priceCbs = new Set<() => void>();

  const cannedFor = (method: string, args: unknown[]): unknown => {
    switch (method) {
      case 'scan': return data.scan;
      case 'lastScan': return data.lastScan;
      case 'unreadableArtifacts': return data.unreadableArtifacts ?? data.scan.sources.filter((s) => s.artifactsUnreadable > 0);
      case 'exportAntigravity': return data.exportReport ?? 'exported 0 Session(s), 0 generation(s)';
      case 'series': return args[1] === 'hour' ? data.hourPoints : data.dayPoints;
      case 'summary': return data.summary;
      case 'breakdown':
        if (args[0] === 'project') return data.projectRows;
        if (args[0] === 'tool') return data.toolRows ?? data.modelRows;
        return data.modelRows;
      case 'ctxResources': return data.ctxResources;
      case 'ctxBuckets': return data.ctxBuckets;
      case 'ctxTools': return data.ctxTools;
      case 'ctxSkills': return data.ctxSkills;
      default: return data.ctxExec;
    }
  };

  const respond = (method: string, args: unknown[]): Promise<unknown> => {
    calls[method].push(args);
    if (fails.has(method)) {
      const e = fails.get(method);
      fails.delete(method);
      return Promise.reject(e);
    }
    if (holds.has(method)) {
      return new Promise((resolve, reject) => {
        (heldMap[method] ??= []).push({ resolve, reject, args });
      });
    }
    return Promise.resolve(cannedFor(method, args));
  };

  return {
    data,
    calls,
    scan: () => respond('scan', []) as Promise<ScanStatus>,
    lastScan: () => respond('lastScan', []) as Promise<number>,
    unreadableArtifacts: () => respond('unreadableArtifacts', []) as Promise<SourceUnreadable[]>,
    exportAntigravity: () => respond('exportAntigravity', []) as Promise<string>,
    series: (filters: Filters, bucket: 'day' | 'hour') =>
      respond('series', [filters, bucket]) as Promise<SeriesPoint[]>,
    summary: (filters: Filters) => respond('summary', [filters]) as Promise<Summary>,
    breakdown: (by: 'model' | 'project' | 'tool', filters: Filters) =>
      respond('breakdown', [by, filters]) as Promise<BreakdownRow[]>,
    ctxResources: (filters: Filters) =>
      respond('ctxResources', [filters]) as Promise<CtxResource[]>,
    ctxBuckets: (filters: Filters) =>
      respond('ctxBuckets', [filters]) as Promise<CtxBuckets[]>,
    ctxTools: (filters: Filters) => respond('ctxTools', [filters]) as Promise<CtxToolRow[]>,
    ctxSkills: (filters: Filters) => respond('ctxSkills', [filters]) as Promise<CtxSkillRow[]>,
    ctxExec: (filters: Filters) => respond('ctxExec', [filters]) as Promise<CtxExecRow[]>,
    onPricesRebuilt: (cb: () => void) => {
      priceCbs.add(cb);
      return () => priceCbs.delete(cb);
    },
    failNext: (method, err) => fails.set(method, err),
    hold: (method) => holds.add(method),
    held: (method) => heldMap[method] ?? [],
    resolveHeld: (method, index, value) => {
      const d = heldMap[method]?.[index];
      if (!d) throw new Error(`no held ${method}[${index}]`);
      d.resolve(value !== undefined ? value : cannedFor(method, d.args));
    },
    emitPricesRebuilt: () => priceCbs.forEach((cb) => cb()),
  };
}
