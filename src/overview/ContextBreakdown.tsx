import { useState } from 'react';
import { execFacets, type CtxTotals, type ExecFacets, type BucketView, type ToolCategory } from './data';
import type { ToolMeta } from './meta';
import type { CtxExecRow } from '../types';
import { fmtTok, fmtPct } from '../lib/format';

const EST_TIP = 'estimated share of billed context (content bytes ÷ 4)';

// Context panel v2 (spec 2026-07-10-context-drilldown): exact usage-field
// primaries with expandable Messages; estimated secondary section with a
// two-level Tool-calls drill-down allocated by stored content weights.
// null renders "—" (source cannot say) — never 0.
export default function ContextBreakdown({
  tool,
  ctx,
  view: v,
  tree,
  meta,
  execRows,
}: {
  tool: ToolMeta;
  ctx: CtxTotals;
  view: BucketView | null;
  tree: ToolCategory[];
  meta: string;
  execRows: CtxExecRow[];
}) {
  const [open, setOpen] = useState<Set<string>>(new Set());
  const [execTab, setExecTab] = useState<'type' | 'exe' | 'cmd'>('type');
  const toggle = (k: string) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(k)) next.delete(k);
      else next.add(k);
      return next;
    });

  const hit = ctx.billed > 0 ? ctx.reused / ctx.billed : 0;
  const denom = Math.max(1, v?.total ?? 0);
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
              cat.tools.map((t) => {
                // Stays in render (not selectView): its input t.tokens only exists
                // once the user expands the tree down to the Bash leaf.
                const facets = t.name === 'Bash' ? execFacets(execRows, t.tokens) : null;
                return (
                  <div key={`leaf:${t.name}`}>
                    {row(`tool:${t.name}`, t.name, t.tokens, {
                      muted: true,
                      indent: 2,
                      info: `${t.calls} calls`,
                      expandable: !!facets,
                    })}
                    {facets && open.has(`tool:${t.name}`) && (
                      <ExecTable facets={facets} tab={execTab} onTab={setExecTab} />
                    )}
                  </div>
                );
              })}
          </div>
        ))}
      {row('agents', 'Custom agents', ctx.agents, { muted: true, info: EST_TIP })}
      {row('mcp', 'MCP servers', ctx.mcp, { muted: true, info: EST_TIP })}
      {row('skills', 'Skills', ctx.skills, { muted: true, info: EST_TIP })}

      {meta && <div className="tt-ctx-meta">{meta}</div>}
    </>
  );
}

const EXEC_TABS = [
  { key: 'type', label: 'By type' },
  { key: 'exe', label: 'Executable' },
  { key: 'cmd', label: 'Command' },
] as const;
const EXEC_TOP_N = 20;

function ExecTable({
  facets,
  tab,
  onTab,
}: {
  facets: ExecFacets;
  tab: 'type' | 'exe' | 'cmd';
  onTab: (t: 'type' | 'exe' | 'cmd') => void;
}) {
  const rows =
    tab === 'type' ? facets.byType : tab === 'exe' ? facets.byExecutable : facets.byCommand;
  const shown = rows.slice(0, EXEC_TOP_N);
  const hidden = rows.length - shown.length;
  return (
    <div className="tt-exec">
      <div className="tt-exec-tabs">
        {EXEC_TABS.map((t) => (
          <button
            key={t.key}
            className={t.key === tab ? 'active' : ''}
            onClick={() => onTab(t.key)}
          >
            {t.label}
          </button>
        ))}
      </div>
      <div className="tt-exec-table">
        <div className="hd">
          <span>Type</span>
          <span>Calls</span>
          <span>Total</span>
        </div>
        {shown.map((r) => (
          <div className="tr" key={r.key}>
            <span className="k" title={r.key}>{r.key}</span>
            <span>{r.calls}</span>
            <span>{fmtTok(r.tokens)}</span>
          </div>
        ))}
        {hidden > 0 && <div className="more">+{hidden} more</div>}
      </div>
    </div>
  );
}
