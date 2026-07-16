import { CATEGORIES, type ToolMeta } from './meta';
import type { CatTotals } from './data';
import { fmtTok, fmtPct } from '../lib/format';
import { CAT_KEY, useOverviewT } from './localize';

// Real per-tool token breakdown (8b right column): the four canonical token
// categories from the Ledger. Replaces the speculative context-content panel.
export default function TokenBreakdown({ tool, cats }: { tool: ToolMeta; cats: CatTotals }) {
  const { t } = useOverviewT();
  const rows = CATEGORIES.map((c) => ({ ...c, tokens: cats[c.key] }));
  const total = rows.reduce((a, r) => a + r.tokens, 0);
  const denomTotal = Math.max(1, total);
  const max = Math.max(1, ...rows.map((r) => r.tokens));
  // Cache Hit Rate (CONTEXT.md): cacheRead / (input + cacheRead + cacheWrite).
  const denom = cats.input + cats.cacheRead + cats.cacheWrite;
  const hit = denom > 0 ? cats.cacheRead / denom : 0;

  return (
    <>
      <div className="tt-ctx-title">
        <span className="dot" style={{ background: tool.color }} />
        {tool.source} {t('overview.tokenBreakdown')}
      </div>
      <div className="tt-ctx-sub">
        {t('overview.cacheHitRate')} <b>{fmtPct(hit)}</b> · <b>{fmtTok(cats.cacheRead)}</b> {t('overview.reused')} /{' '}
        <b>{fmtTok(total)}</b> {t('overview.total')}
      </div>
      {rows.map((r) => (
        <div className="tt-ctx-row" key={r.key}>
          <span className="bar" style={{ width: (r.tokens / max) * 100 + '%', background: r.color }} />
          <span className="name">
            <span className="dot" style={{ background: r.color }} />
            {t(CAT_KEY[r.key])}
          </span>
          <span className="vals">
            <span className="val">{fmtTok(r.tokens)}</span>
            <span className="rpct">{fmtPct(r.tokens / denomTotal)}</span>
          </span>
        </div>
      ))}
    </>
  );
}
