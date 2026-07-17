import { useState } from 'react';
import { execFacets, type CtxTotals, type ExecFacets, type BucketView, type ToolCategory } from './data';
import type { ToolMeta } from './meta';
import type { CtxExecRow } from '../types';
import { fmtTok, fmtPct } from '../lib/format';
import { useOverviewT } from './localize';

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
  const { t } = useOverviewT();
  const estShareTip = t('overview.estTip');
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
      ? `${t('overview.estComposition')} ${t('overview.messagesWord')} ${fmtTok(ctx.messages)} · ${t('overview.systemWord')} ${fmtTok(ctx.system)}`
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
        <span className="dot" style={{ background: opts.muted ? 'var(--border-strong)' : tool.color }} />
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
        {tool.source} {t('overview.contextBreakdown')}
      </div>
      <div className="tt-ctx-sub">
        {t('overview.cacheHitRate')} <b>{fmtPct(hit)}</b> · <b>{fmtTok(ctx.reused)}</b> {t('overview.reused')} /{' '}
        <b>{fmtTok(ctx.billed)}</b> {t('overview.ctxInputWord')}
      </div>

      {row('messages', t('overview.messages'), v ? v.messages : null, { pct: true, expandable: !!v, info: estTip })}
      {v && open.has('messages') && (
        <>
          {row('history', t('overview.convHistory'), v.history, { indent: 1 })}
          {row('newInput', t('overview.newInput'), v.newInput, {
            indent: 1,
            info: t('overview.newInputInfo'),
          })}
          {row('response', t('overview.assistantResponse'), v.response, { indent: 1 })}
        </>
      )}
      {row('system', t('overview.systemPrompt'), v ? v.system : null, {
        pct: true,
        info: t('overview.systemInfo'),
      })}
      {row('reasoning', t('overview.reasoning'), v ? v.reasoning : null, { pct: true })}

      <div style={{ height: 1, background: 'var(--border-subtle)', margin: '8px 4px' }} />

      {row('toolcalls', t('overview.toolCalls'), ctx.toolcalls, {
        muted: true,
        expandable: tree.length > 0,
        info: estShareTip,
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
              cat.tools.map((leaf) => {
                // Stays in render (not selectView): its input leaf.tokens only exists
                // once the user expands the tree down to the Bash leaf.
                const facets = leaf.name === 'Bash' ? execFacets(execRows, leaf.tokens) : null;
                return (
                  <div key={`leaf:${leaf.name}`}>
                    {row(`tool:${leaf.name}`, leaf.name, leaf.tokens, {
                      muted: true,
                      indent: 2,
                      info: `${leaf.calls} ${t('overview.calls')}`,
                      expandable: !!facets,
                    })}
                    {facets && open.has(`tool:${leaf.name}`) && (
                      <ExecTable facets={facets} tab={execTab} onTab={setExecTab} />
                    )}
                  </div>
                );
              })}
          </div>
        ))}
      {row('agents', t('overview.customAgents'), ctx.agents, { muted: true, info: estShareTip })}
      {row('mcp', t('overview.mcpServers'), ctx.mcp, { muted: true, info: estShareTip })}
      {row('skills', t('overview.skills'), ctx.skills, { muted: true, info: estShareTip })}

      {meta && <div className="tt-ctx-meta">{meta}</div>}
    </>
  );
}

const EXEC_TABS = [
  { key: 'type', labelKey: 'overview.exec.byType' },
  { key: 'exe', labelKey: 'overview.exec.executable' },
  { key: 'cmd', labelKey: 'overview.exec.command' },
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
  const { t } = useOverviewT();
  const rows =
    tab === 'type' ? facets.byType : tab === 'exe' ? facets.byExecutable : facets.byCommand;
  const shown = rows.slice(0, EXEC_TOP_N);
  const hidden = rows.length - shown.length;
  return (
    <div className="tt-exec">
      <div className="tt-exec-tabs">
        {EXEC_TABS.map((opt) => (
          <button
            key={opt.key}
            className={opt.key === tab ? 'active' : ''}
            onClick={() => onTab(opt.key)}
          >
            {t(opt.labelKey)}
          </button>
        ))}
      </div>
      <div className="tt-exec-table">
        <div className="hd">
          <span>{t('overview.exec.type')}</span>
          <span>{t('overview.exec.calls')}</span>
          <span>{t('overview.exec.total')}</span>
        </div>
        {shown.map((r) => (
          <div className="tr" key={r.key}>
            <span className="k" title={r.key}>{r.key}</span>
            <span>{r.calls}</span>
            <span>{fmtTok(r.tokens)}</span>
          </div>
        ))}
        {hidden > 0 && <div className="more">+{hidden} {t('overview.more')}</div>}
      </div>
    </div>
  );
}
