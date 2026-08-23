// The Model detail a Models-card row opens (was the Override editor, which
// startled — a click on a usage figure answered with a pricing form). Shows
// the figures behind the row's bar: each token category's count, share, and
// its cost at the Model's active rate (Override → catalog, resolvedRates).
//
// The per-bucket costs are a frontend recompute, and the backend prices what
// this dialog cannot see: per-day rates (resolve_at's day capture, mid-window
// price changes) and a distinct 1h cache-write rate (the wire's single
// cacheWrite is the 5m rate over merged 5m+1h tokens). So the decomposition
// is GUARDED: bucket costs render only when they sum back to the row's own
// Cost — otherwise every bucket shows an em dash rather than figures that
// disagree with the total above them. An absent rate's bucket is an em dash
// either way, mirroring the backend's Rates::cost which omits that bucket.
// "Set rate…" hands off to the shared Override editor, keeping the Pricing
// fix reachable where the unpriced symptom shows.
import { useRef } from 'react';
// Dialog chrome styles ride with the pricing dialog family (see OverrideEditor
// on why the import lives on the component, not a page).
import '../pricing/pricing.css';
import type { BreakdownRow, ModelPricing, Settings } from '../types';
import { CATEGORIES } from './meta';
import { fmtTok, fmtPct } from '../lib/format';
import { isPartialCost } from '../lib/costCompleteness';
import { CAT_KEY, formatDisplayCost, useOverviewT, USD_IDENTITY } from './localize';
import { useDialogChrome } from './useDialogChrome';
import { resolvedRates, sourceLabel } from '../pricing/pricing.derive';
import DialogHead from '../pricing/DialogHead';

export default function ModelBreakdownModal({
  row,
  model,
  settings = USD_IDENTITY,
  onSetRate,
  onClose,
}: {
  row: BreakdownRow;
  model: ModelPricing;
  settings?: Pick<Settings, 'currency' | 'usdRate'>;
  onSetRate: () => void;
  onClose: () => void;
}) {
  const { t, lang } = useOverviewT();
  const modalRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  // No opener ref: the row is a div WebKit never focused, so the chrome's
  // previously-focused fallback is already the right return target.
  const returnRef = useRef<HTMLElement>(null);
  useDialogChrome({ modalRef, initialFocusRef: closeRef, returnFocusRef: returnRef, onClose });

  const tokens: Record<(typeof CATEGORIES)[number]['key'], number> = {
    input: row.inputTokens,
    output: row.outputTokens,
    cacheRead: row.cacheReadTokens,
    cacheWrite: row.cacheWriteTokens,
  };
  const rates = resolvedRates(model);
  const costs = CATEGORIES.map((c) => {
    const rate = rates?.[c.key];
    return rate == null ? null : tokens[c.key] * rate;
  });
  // The guard: the decomposition is only a decomposition if the priced buckets
  // sum back to the row's Cost (in USD, before display conversion). Tolerance
  // is float noise only — any real divergence means the rate basis differs.
  const priced = costs.reduce<number>((a, c) => a + (c ?? 0), 0);
  const reconciles =
    row.cost !== null && Math.abs(priced - row.cost) <= Math.max(1e-8, row.cost * 1e-6);
  const max = Math.max(1, ...CATEGORIES.map((c) => tokens[c.key]));
  const denom = Math.max(1, row.totalTokens);

  return (
    <div
      className="tl-pr-backdrop"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <span className="tl-modal-dragstrip" aria-hidden="true" data-tauri-drag-region />
      <div
        ref={modalRef}
        className="tl-pr-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="tl-pr-dialog-name"
        tabIndex={-1}
      >
        <DialogHead
          tool={model.tool}
          name={model.model}
          subtitle={`${sourceLabel(model.tool)} · ${t('overview.tokenBreakdown')}`}
          closeLabel={t('overview.close')}
          onClose={onClose}
          closeRef={closeRef}
        />

        <div className="tl-pr-dialog-body">
          <div className="tt-ctx-sub" style={{ marginTop: 0 }}>
            <b>{fmtTok(row.totalTokens)}</b> {t('overview.total')} ·{' '}
            <b>{formatDisplayCost(row.cost, isPartialCost(row), settings, lang)}</b>
            {row.cacheEstimated && (
              <span className="tt-tag" title={t('overview.cacheEstimated')}>{t('overview.cacheEst')}</span>
            )}
          </div>

          <div className="tl-pr-bd-rows">
            {CATEGORIES.map((c, i) => {
              const tk = tokens[c.key];
              const cost = costs[i];
              return (
                <div className="tt-ctx-row" key={c.key}>
                  <span className="bar" style={{ width: (tk / max) * 100 + '%', background: c.color }} />
                  <span className="name">
                    <span className="dot" style={{ background: c.color }} />
                    {t(CAT_KEY[c.key])}
                  </span>
                  <span className="vals">
                    <span className="val">{fmtTok(tk)}</span>
                    <span className="rcost">
                      {reconciles && cost !== null
                        ? formatDisplayCost(cost, false, settings, lang, { adaptivePrecision: true })
                        : '—'}
                    </span>
                    <span className="rpct">{fmtPct(tk / denom)}</span>
                  </span>
                </div>
              );
            })}
          </div>

          <div className="tl-pr-dialog-actions">
            <button type="button" className="tl-pr-secondary" onClick={onSetRate}>
              {t('overview.setRate')}
            </button>
            <span style={{ flex: 1 }} />
            <button type="button" className="tl-pr-primary" onClick={onClose}>
              {t('overview.close')}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
