import { contextBreakdown, fmtTok, fmtPct, type ToolMeta } from './mock';

// Context-window breakdown for one source (Context tab in 8a, right column in 8b).
// showBars renders the faint proportion bar behind primary rows (8b style).
export default function ContextBreakdown({
  tool,
  toolTokens,
  showBars = false,
}: {
  tool: ToolMeta;
  toolTokens?: number;
  showBars?: boolean;
}) {
  const ctx = contextBreakdown(tool.key, toolTokens);
  const primaryMax = Math.max(1, ...ctx.primary.map((p) => p.tokens));

  return (
    <>
      <div className="tt-ctx-title">
        <span className="dot" style={{ background: tool.color }} />
        {tool.source} Context Breakdown
      </div>
      <div className="tt-ctx-sub">
        Cache hit rate <b>{fmtPct(ctx.cacheHit)}</b> · <b>{fmtTok(ctx.reused)}</b> reused /{' '}
        <b>{fmtTok(ctx.input)}</b> input
      </div>
      {ctx.primary.map((p) => (
        <div className="tt-ctx-row" key={p.key}>
          {showBars && (
            <span
              className="bar"
              style={{ width: (p.tokens / primaryMax) * 100 + '%', background: tool.color }}
            />
          )}
          <span className="name">
            <span className="dot" style={{ background: tool.color }} />
            {p.label}
            {'expand' in p && p.expand && <span className="aff">›</span>}
            {'info' in p && p.info && <span className="aff">ⓘ</span>}
          </span>
          <span className="vals">
            <span className="val">{fmtTok(p.tokens)}</span>
            <span className="rpct">{fmtPct(p.pct)}</span>
          </span>
        </div>
      ))}
      <div style={{ height: 1, background: 'rgba(255,255,255,.06)', margin: '8px 4px' }} />
      {ctx.secondary.map((s) => (
        <div className="tt-ctx-row muted" key={s.key}>
          <span className="name">
            <span className="dot" style={{ background: '#4a5262' }} />
            {s.label}
            <span className="aff">›</span>
          </span>
          <span className="vals">
            <span className="val">{fmtTok(s.tokens)}</span>
          </span>
        </div>
      ))}
      <div className="tt-ctx-meta">{ctx.meta}</div>
    </>
  );
}
