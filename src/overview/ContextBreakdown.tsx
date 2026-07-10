import type { CtxTotals, ToolMeta } from './data';
import { fmtTok, fmtPct } from '../lib/format';

const PRIMARY = [
  { key: 'messages', label: 'Messages' },
  { key: 'system', label: 'System prompt', info: "Estimated from each session's first call" },
  { key: 'reasoning', label: 'Reasoning' },
] as const;

const SECONDARY = [
  { key: 'toolcalls', label: 'Tool calls' },
  { key: 'agents', label: 'Custom agents' },
  { key: 'mcp', label: 'MCP servers' },
  { key: 'skills', label: 'Skills' },
] as const;

// Context-window breakdown for one source, from real scan-time attribution.
// Primary rows partition billed context (input + cache read + cache write);
// secondary rows are overlapping subsets of Messages. null renders "—" —
// this source's logs can't attribute that category (never 0).
export default function ContextBreakdown({
  tool,
  ctx,
  meta,
}: {
  tool: ToolMeta;
  ctx: CtxTotals;
  meta: string;
}) {
  const hit = ctx.billed > 0 ? ctx.reused / ctx.billed : 0;
  const denom = Math.max(1, ctx.billed);
  const primary = PRIMARY.map((p) => ({ ...p, tokens: ctx[p.key] }));
  const primaryMax = Math.max(1, ...primary.map((p) => p.tokens ?? 0));
  const unattributed = primary.every((p) => p.tokens == null);

  return (
    <>
      <div className="tt-ctx-title">
        <span className="dot" style={{ background: tool.color }} />
        {tool.source} Context Breakdown
      </div>
      <div className="tt-ctx-sub">
        Cache hit rate <b>{fmtPct(hit)}</b> · <b>{fmtTok(ctx.reused)}</b> reused /{' '}
        <b>{fmtTok(ctx.billed)}</b> input · est.
      </div>
      {primary.map((p) => (
        <div className="tt-ctx-row" key={p.key}>
          {p.tokens != null && (
            <span
              className="bar"
              style={{ width: (p.tokens / primaryMax) * 100 + '%', background: tool.color }}
            />
          )}
          <span className="name">
            <span className="dot" style={{ background: tool.color }} />
            {p.label}
            {'info' in p && p.info && (
              <span className="aff" title={p.info}>
                ⓘ
              </span>
            )}
          </span>
          <span className="vals">
            {p.tokens == null ? (
              <span className="val">—</span>
            ) : (
              <>
                <span className="val">{fmtTok(p.tokens)}</span>
                <span className="rpct">{fmtPct(p.tokens / denom)}</span>
              </>
            )}
          </span>
        </div>
      ))}
      <div style={{ height: 1, background: 'rgba(255,255,255,.06)', margin: '8px 4px' }} />
      {SECONDARY.map((s) => (
        <div className="tt-ctx-row muted" key={s.key}>
          <span className="name">
            <span className="dot" style={{ background: '#4a5262' }} />
            {s.label}
          </span>
          <span className="vals">
            <span className="val">{ctx[s.key] == null ? '—' : fmtTok(ctx[s.key]!)}</span>
          </span>
        </div>
      ))}
      {unattributed ? (
        <div className="tt-ctx-meta" title="This source's logs don't record message content">
          no content attribution for this source
        </div>
      ) : (
        meta && <div className="tt-ctx-meta">{meta}</div>
      )}
    </>
  );
}
