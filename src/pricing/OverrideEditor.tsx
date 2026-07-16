// The one Override editor (design 1b), opened from two places: a Pricing-table
// row and a Model selection in the Overview. Entry is per-1M USD; it converts
// ÷1e6 to the per-token RatesPerTok on save (×1e6 when loading an existing
// Override). Empty field => null bucket; at least one non-empty enables Save.
// Data flows through PricingPort; Settings is read only for the currency note.
import { useEffect, useState } from 'react';
import { useT } from '../lib/i18n';
import type { ModelPricing, RatesPerTok, Settings } from '../types';
import type { PricingPort } from './pricing';
import { tauriSettings, type SettingsPort } from '../settings/settings';
import { TOOL_ICONS } from '../overview/icons';
import { fill, originLabel, toolMeta, toolLabel, fmtRate } from './pricing.derive';

const FIELDS: { key: keyof RatesPerTok; labelKey: 'pricing.col.input' | 'pricing.col.output' | 'pricing.col.cacheRead' | 'pricing.col.cacheWrite' }[] = [
  { key: 'input', labelKey: 'pricing.col.input' },
  { key: 'output', labelKey: 'pricing.col.output' },
  { key: 'cacheRead', labelKey: 'pricing.col.cacheRead' },
  { key: 'cacheWrite', labelKey: 'pricing.col.cacheWrite' },
];

// per-token USD -> the per-1M string shown in the field (trims float noise).
function toInput(perTok: number | null | undefined): string {
  if (perTok == null) return '';
  return String(+(perTok * 1_000_000).toFixed(6));
}

export default function OverrideEditor({
  model,
  pricing,
  settings = tauriSettings,
  onClose,
}: {
  model: ModelPricing;
  pricing: PricingPort;
  settings?: SettingsPort;
  onClose: (changed: boolean) => void;
}) {
  const { t } = useT();
  const hasOverride = !!model.overrideRates;
  const catalog = model.catalog;

  const [values, setValues] = useState<Record<keyof RatesPerTok, string>>(() => ({
    input: toInput(model.overrideRates?.input),
    output: toInput(model.overrideRates?.output),
    cacheRead: toInput(model.overrideRates?.cacheRead),
    cacheWrite: toInput(model.overrideRates?.cacheWrite),
  }));
  const [cfg, setCfg] = useState<Pick<Settings, 'currency' | 'usdRate'> | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let alive = true;
    settings.get().then((s) => alive && setCfg({ currency: s.currency, usdRate: s.usdRate })).catch(() => {});
    return () => { alive = false; };
  }, [settings]);

  const parsed = FIELDS.map(({ key }) => {
    const raw = values[key].trim();
    const num = Number(raw);
    return { key, raw, bad: raw !== '' && (!Number.isFinite(num) || num < 0), num };
  });
  const anyBad = parsed.some((p) => p.bad);
  const anyFilled = parsed.some((p) => p.raw !== '');
  const canSave = anyFilled && !anyBad && !busy;

  const close = (changed: boolean) => {
    if (busy) return;
    onClose(changed);
  };

  const save = () => {
    if (!canSave) return;
    const rates: RatesPerTok = { input: null, output: null, cacheRead: null, cacheWrite: null };
    for (const p of parsed) rates[p.key] = p.raw === '' ? null : p.num / 1_000_000;
    setBusy(true);
    pricing.setOverride(model.model, rates).then(() => onClose(true)).catch(() => setBusy(false));
  };

  const remove = () => {
    setBusy(true);
    pricing.deleteOverride(model.model).then(() => onClose(true)).catch(() => setBusy(false));
  };

  const meta = toolMeta(model.tool);
  const icon = meta && TOOL_ICONS[meta.key];
  const tool = toolLabel(model.tool);
  const subtitle = hasOverride
    ? fill(t('pricing.editor.subtitleOverride'), { tool })
    : fill(t('pricing.editor.subtitleSet'), { tool });

  return (
    <div
      className="tl-pr-backdrop"
      onClick={() => close(false)}
      onKeyDown={(e) => { if (e.key === 'Escape') close(false); }}
    >
      <div
        className="tl-pr-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="tl-pr-dialog-name"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="tl-pr-dialog-head">
          <span className="tl-pr-icon">
            {icon ? <img src={icon} alt="" width={15} height={15} /> : <b>{tool[0]}</b>}
          </span>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div className="name" id="tl-pr-dialog-name">{model.model}</div>
            <div className="subtitle">{subtitle}</div>
          </div>
          <button type="button" className="tl-pr-dialog-close" aria-label={t('pricing.editor.close')} onClick={() => close(false)}>
            ✕
          </button>
        </div>

        <div className="tl-pr-dialog-body">
          {hasOverride ? (
            <div className="tl-pr-explain">
              {catalog
                ? fill(t('pricing.editor.overrideExplain'), { origin: originLabel(catalog.origin) })
                : t('pricing.editor.overrideExplainNoCatalog')}
            </div>
          ) : catalog ? (
            <div className="tl-pr-explain">
              {fill(t('pricing.editor.catalogExplain'), { origin: originLabel(catalog.origin) })}
            </div>
          ) : (
            <div className="tl-pr-unpriced-box">
              <span className="dot" />
              <span>
                <b>{t('pricing.editor.unpricedTitle')}</b> {t('pricing.editor.unpricedBody')}
              </span>
            </div>
          )}

          <div className="tl-pr-fields">
            {FIELDS.map(({ key, labelKey }, i) => {
              const p = parsed[i];
              const catRate = catalog?.rates[key];
              const caption = catRate != null
                ? fill(t('pricing.editor.replaces'), { origin: catalog ? originLabel(catalog.origin) : '', price: fmtRate(catRate) })
                : t('pricing.editor.noCatalogRate');
              return (
                <div className="tl-pr-field" key={key}>
                  <label htmlFor={`tl-pr-f-${key}`}>{t(labelKey)}</label>
                  <div className={'tl-pr-input' + (p.bad ? ' invalid' : '')}>
                    <span className="prefix">$</span>
                    <input
                      id={`tl-pr-f-${key}`}
                      inputMode="decimal"
                      aria-label={t(labelKey)}
                      aria-invalid={p.bad || undefined}
                      autoFocus={i === 0}
                      placeholder="0.00"
                      value={values[key]}
                      onChange={(e) => setValues((v) => ({ ...v, [key]: e.target.value }))}
                    />
                    <span className="suffix">/ 1M</span>
                  </div>
                  <div className={'caption' + (p.bad ? ' invalid' : '')}>
                    {p.bad ? t('pricing.editor.invalid') : caption}
                  </div>
                </div>
              );
            })}
          </div>

          {cfg && cfg.currency !== 'USD' && (
            <div className="tl-pr-currency">
              <span className="sign">$</span>
              <span>{fill(t('pricing.editor.currencyNote'), { code: cfg.currency, rate: cfg.usdRate })}</span>
            </div>
          )}

          <div className="tl-pr-dialog-actions">
            {hasOverride && (
              <button type="button" className="tl-pr-remove" onClick={remove} disabled={busy}>
                {t('pricing.editor.remove')}
              </button>
            )}
            <span style={{ flex: 1 }} />
            <button type="button" className="tl-pr-cancel" onClick={() => close(false)}>
              {t('pricing.editor.cancel')}
            </button>
            <button type="button" className="tl-pr-save" onClick={save} disabled={!canSave}>
              {hasOverride ? t('pricing.editor.saveOverride') : t('pricing.editor.saveRate')}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
