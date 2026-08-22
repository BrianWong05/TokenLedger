// One Context readout for a date window: billed totals, exact buckets, and the
// allocated tree / bars. Callers pass a Source; the report walks every Source.
// Null category totals stay null ("—"), never 0.
import type { LedgerContext } from '../types';
import type { Lang } from '../lib/i18n';
import type { SourceKey } from './meta';
import {
  bucketView,
  ctxMeta,
  execSignatures,
  mcpBars,
  skillBars,
  toolTree,
  type BucketView,
  type CtxTotals,
  type ExecSignature,
  type McpBar,
  type SkillBar,
  type ToolCategory,
} from './data';
import type { CtxExecRow, CtxSkillRow } from '../types';

const EMPTY_TOTALS: CtxTotals = {
  billed: 0, reused: 0,
  messages: null, system: null, reasoning: null,
  toolcalls: null, agents: null, mcp: null, skills: null,
};

export function sourceTotals(ctx: LedgerContext, source: string): CtxTotals {
  const t = ctx.totals.find((row) => row.source === source);
  if (!t) return EMPTY_TOTALS;
  return {
    billed: t.billed, reused: t.reused,
    messages: t.messages, system: t.system, reasoning: t.reasoning,
    toolcalls: t.toolcalls, agents: t.agents, mcp: t.mcp, skills: t.skills,
  };
}

export interface SourceContext {
  source: string;
  totals: CtxTotals;
  view: BucketView | null;
  tree: ToolCategory[];
  exec: CtxExecRow[];
  meta: string;
  skills: SkillBar[];
  skillRows: CtxSkillRow[];
  mcp: McpBar[];
  signatures: ExecSignature[];
}

export function sourceContext(ctx: LedgerContext, source: string, lang: Lang = 'en'): SourceContext {
  const totals = sourceTotals(ctx, source);
  const tools = ctx.tools.filter((r) => r.source === source);
  const tree = toolTree(tools, totals.toolcalls);
  const exec = ctx.exec.filter((r) => r.source === source);
  const bash = tree.flatMap((c) => c.tools).find((leaf) => leaf.name === 'Bash') ?? null;
  return {
    source,
    totals,
    view: bucketView(ctx.buckets.find((b) => b.source === source) ?? null),
    tree,
    exec,
    meta: ctxMeta(ctx.resources, source as SourceKey, lang),
    skills: skillBars(ctx.skills, source),
    skillRows: ctx.skills.filter((r) => r.source === source),
    mcp: mcpBars(tools, totals.mcp),
    signatures: execSignatures(exec, bash?.tokens ?? null),
  };
}
