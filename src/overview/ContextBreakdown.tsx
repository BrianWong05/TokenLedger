import { useState } from 'react';
import { bucketView, toolTree, type CtxTotals, type ToolMeta } from './data';
import type { CtxBuckets, CtxToolRow } from '../types';
import { fmtTok, fmtPct } from '../lib/format';

const EST_TIP = 'estimated share of billed context (content bytes ÷ 4)';

// Context panel v2 (spec 2026-07-10-context-drilldown): exact usage-field
// primaries with expandable Messages; estimated secondary section with a
// two-level Tool-calls drill-down allocated by stored content weights.
// null renders "—" (source cannot say) — never 0.
export default function ContextBreakdown({
  tool,
  ctx,
  buckets,
  toolRows,
  meta,
}: {
  tool: ToolMeta;
  ctx: CtxTotals;
  buckets: CtxBuckets | null;
  toolRows: CtxToolRow[];
  meta: string;
}) {
  const [open, setOpen] = useState<Set<string>>(new Set());
  const toggle = (k: string) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(k)) next.delete(k);
      else next.add(k);
      return next;
    });

  const v = bucketView(buckets);
  const hit = ctx.billed > 0 ? ctx.reused / ctx.billed : 0;
  const denom = Math.max(1, v?.total ?? 0);
  const tree = toolTree(toolRows, ctx.toolcalls);
  const estTip =
    ctx.messages != null && ctx.system != null
      ? `est. content composition: messages ${fmtTok(ctx.messages)} · system ${fmtTok(ctx.system)}`
      : undefined;

  const row = (
    key: string,
    label: string,
    tokens: number | null,
    opts: { pct?: boolean; muted?: boolean; indent?: 0 | 1 | 2; expandable?: boolean; info?: string; calls?: number } = {},
  ) => (
    <div
      className={
        'tt-ctx-row' + (opts.muted ? ' muted' : '') + (opts.indent ? ` indent-${opts.indent}` : '')
      }
      key={key}
      onClick={opts.expandable ? () => toggle(key) : undefined}
      style={opts.expandable ? { cursor: 'pointer' } : undefined}
      title={opts.info}
    >
      <span className="name">
        <span className="dot" style={{ background: opts.muted ? '#4a5262' : tool.color }} />
        {label}
        {opts.expandable && <span className="aff">{open.has(key) ? '▾' : '›'}</span>}
        {opts.info && !opts.expandable && <span className="aff">ⓘ</span>}
      </span>
      <span className="vals">
        {tokens == null ? (
          <span className="val">—</span>
        ) : (
          <>
            <span className="val">{fmtTok(tokens)}</span>
            {opts.pct && <span className="rpct">{fmtPct(tokens / denom)}</span>}
          </>
        )}
      </span>
    </div>
  );

  return (
    <>
      <div className="tt-ctx-title">
        <span className="dot" style={{ background: tool.color }} />
        {tool.source} Context Breakdown
      </div>
      <div className="tt-ctx-sub">
        Cache hit rate <b>{fmtPct(hit)}</b> · <b>{fmtTok(ctx.reused)}</b> reused /{' '}
        <b>{fmtTok(ctx.billed)}</b> input
      </div>

      {row('messages', 'Messages', v ? v.messages : null, { pct: true, expandable: !!v, info: estTip })}
      {v && open.has('messages') && (
        <>
          {row('history', 'Conversation history', v.history, { indent: 1 })}
          {row('newInput', 'New input', v.newInput, {
            indent: 1,
            info: 'uncached input for the newest turn — user text and fresh tool results',
          })}
          {row('response', 'Assistant response', v.response, { indent: 1 })}
        </>
      )}
      {row('system', 'System prompt', v ? v.system : null, {
        pct: true,
        info: 'first cache write of each session',
      })}
      {row('reasoning', 'Reasoning', v ? v.reasoning : null, { pct: true })}

      <div style={{ height: 1, background: 'rgba(255,255,255,.06)', margin: '8px 4px' }} />

      {row('toolcalls', 'Tool calls', ctx.toolcalls, {
        muted: true,
        expandable: tree.length > 0,
        info: EST_TIP,
      })}
      {open.has('toolcalls') &&
        tree.map((cat) => (
          <div key={cat.label}>
            {row(`cat:${cat.label}`, cat.label, cat.tokens, {
              muted: true,
              indent: 1,
              expandable: cat.tools.length > 0,
            })}
            {open.has(`cat:${cat.label}`) &&
              cat.tools.map((t) =>
                row(`tool:${t.name}`, t.name, t.tokens, {
                  muted: true,
                  indent: 2,
                  info: `${t.calls} calls`,
                }),
              )}
          </div>
        ))}
      {row('agents', 'Custom agents', ctx.agents, { muted: true, info: EST_TIP })}
      {row('mcp', 'MCP servers', ctx.mcp, { muted: true, info: EST_TIP })}
      {row('skills', 'Skills', ctx.skills, { muted: true, info: EST_TIP })}

      {meta && <div className="tt-ctx-meta">{meta}</div>}
    </>
  );
}
