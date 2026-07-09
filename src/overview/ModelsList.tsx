import { MODELS, categorySplit, costOf, fmtTok, fmtUSD, fmtPct, type ToolMeta } from './mock';

// Per-model token breakdown for one source. Each bar's filled width is the
// model's share of the source; inner segments are the four token categories.
export default function ModelsList({
  tool,
  toolTokens,
  showCost = true,
}: {
  tool: ToolMeta;
  toolTokens: number;
  showCost?: boolean;
}) {
  const models = MODELS[tool.key];
  return (
    <>
      <div className="tt-models-head">
        <div className="lbl">
          <span className="dot" style={{ background: tool.color }} />
          Models <span className="count">· {models.length}</span>
        </div>
        <span className="tot">{fmtTok(toolTokens)}</span>
      </div>
      {models.map((m) => {
        const modelTok = Math.round(toolTokens * m.share);
        const segs = categorySplit(tool.key, modelTok);
        const segTotal = Math.max(1, segs.reduce((a, c) => a + c.tokens, 0));
        return (
          <div className="tt-model" key={m.name}>
            <div className="top">
              <span className="name">{m.name}</span>
              <span className="figs">
                <span className="tok">{fmtTok(modelTok)}</span>
                {showCost && <span className="cost">{fmtUSD(costOf(modelTok))}</span>}
                <span className="pct">{fmtPct(m.share)}</span>
              </span>
            </div>
            <div className="track">
              <div className="segs" style={{ width: m.share * 100 + '%' }}>
                {segs.map((c) => (
                  <div key={c.key} style={{ width: (c.tokens / segTotal) * 100 + '%', background: c.color }} />
                ))}
              </div>
            </div>
          </div>
        );
      })}
      <div className="tt-legend" style={{ marginTop: 12, paddingTop: 12, borderTop: '1px solid rgba(255,255,255,.06)' }}>
        {categorySplit(tool.key, toolTokens).map((c) => (
          <span className="item" key={c.key}>
            <span className="sw" style={{ background: c.color }} />
            {c.label}
          </span>
        ))}
      </div>
    </>
  );
}
