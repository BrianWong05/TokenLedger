// Pricing tab (design 1a/1g): the rate workbench. Lists every Model seen in the
// Ledger with its resolved List Price, catalog origin, Override, and pricing
// state — never usage or cost (glossary rule). Data flows through PricingPort;
// the row action and the Overview both open the shared OverrideEditor. The shell
// mounts <PricingPage /> with no props, so ports default to the real tauri seams.
import { useCallback, useEffect, useMemo, useState } from 'react';
import './pricing.css';
import { useT } from '../lib/i18n';
import type { ModelPricing, RatesPerTok } from '../types';
import { tauriPricing, type PricingPort } from './pricing';
import { tauriLedger, type LedgerPort } from '../overview/ledger';
import type { SettingsPort } from '../settings/settings';
import { TOOL_ICONS } from '../overview/icons';
import OverrideEditor from './OverrideEditor';
import {
  modelState, resolvedRates, filterModels, chipCounts, fmtRate,
  toolMeta, toolLabel, originLabel, fill, type PriceFilter,
} from './pricing.derive';

const CHIPS: { key: PriceFilter; labelKey: 'pricing.chip.all' | 'pricing.chip.unpriced' | 'pricing.chip.override' | 'pricing.chip.est'; count: (c: ReturnType<typeof chipCounts>) => number }[] = [
  { key: 'all', labelKey: 'pricing.chip.all', count: (c) => c.all },
  { key: 'unpriced', labelKey: 'pricing.chip.unpriced', count: (c) => c.unpriced },
  { key: 'override', labelKey: 'pricing.chip.override', count: (c) => c.override },
  { key: 'est', labelKey: 'pricing.chip.est', count: (c) => c.est },
];

const RATE_KEYS: (keyof RatesPerTok)[] = ['input', 'output', 'cacheRead', 'cacheWrite'];

export default function PricingPage({
  ports,
}: {
  ports?: { pricing?: PricingPort; ledger?: LedgerPort; settings?: SettingsPort };
} = {}) {
  const pricing = ports?.pricing ?? tauriPricing;
  const ledger = ports?.ledger ?? tauriLedger;
  const { t } = useT();

  const [models, setModels] = useState<ModelPricing[] | null>(null);
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState<PriceFilter>('all');
  const [editor, setEditor] = useState<ModelPricing | null>(null);
  const [scanning, setScanning] = useState(false);

  const reload = useCallback(() => {
    // Promise.resolve guards a port whose IPC throws synchronously when no Tauri
    // runtime is present (the shell mounts this tab port-less under test).
    try {
      Promise.resolve(pricing.list()).then(setModels).catch(() => {});
    } catch { /* no runtime */ }
  }, [pricing]);

  useEffect(() => {
    reload();
    try {
      return pricing.onPricesRebuilt(reload);
    } catch {
      return undefined;
    }
  }, [pricing, reload]);

  const counts = useMemo(() => (models ? chipCounts(models) : { all: 0, unpriced: 0, override: 0, est: 0 }), [models]);
  const rows = useMemo(() => (models ? filterModels(models, query, filter) : []), [models, query, filter]);

  const scanNow = () => {
    if (scanning) return;
    setScanning(true);
    ledger.scan().then(reload).catch(() => {}).finally(() => setScanning(false));
  };

  return (
    <div className="tl-page tl-page-pricing">
      {models === null ? (
        <LoadingSkeleton note={t('pricing.loading')} title={t('pricing.modelRates')} />
      ) : models.length === 0 ? (
        <EmptyState
          title={t('pricing.empty')}
          sub={t('pricing.emptySub')}
          scanLabel={t('pricing.scanNow')}
          scanning={scanning}
          onScan={scanNow}
        />
      ) : (
        <>
          <div className="tl-pr-toolbar">
            <div className="tl-pr-search">
              <svg width="13" height="13" viewBox="0 0 14 14" aria-hidden="true">
                <circle cx="6" cy="6" r="4.6" stroke="var(--muted)" strokeWidth="1.4" fill="none" />
                <line x1="9.6" y1="9.6" x2="13" y2="13" stroke="var(--muted)" strokeWidth="1.4" strokeLinecap="round" />
              </svg>
              <input
                type="search"
                aria-label={t('pricing.filterPlaceholder')}
                placeholder={t('pricing.filterPlaceholder')}
                value={query}
                onChange={(e) => setQuery(e.target.value)}
              />
            </div>
            <div className="tl-pr-chips" role="group">
              {CHIPS.map((c) => (
                <button
                  key={c.key}
                  type="button"
                  className={'tl-pr-chip' + (filter === c.key ? ' active' : '')}
                  aria-pressed={filter === c.key}
                  onClick={() => setFilter(c.key)}
                >
                  {t(c.labelKey)}
                  <span className="n">{c.count(counts)}</span>
                </button>
              ))}
            </div>
            <span className="tl-pr-spacer" />
            <span className="tl-pr-catnote">{t('pricing.catalogNote')}</span>
          </div>

          {counts.unpriced > 0 && (
            <div className="tl-pr-banner">
              <span className="dot" />
              <span className="msg">{fill(t('pricing.banner'), { n: counts.unpriced })}</span>
              <span className="tl-pr-spacer" />
              <button type="button" className="tl-pr-banner-review" onClick={() => setFilter('unpriced')}>
                {t('pricing.bannerReview')}
              </button>
            </div>
          )}

          <div className="tl-pr-card">
            <div className="tl-pr-card-head">
              <span className="title">{t('pricing.modelRates')}</span>
              <span className="note">{t('pricing.tableNote')}</span>
            </div>

            <div className="tl-pr-grid">
              <span className="tl-pr-colhead">{t('pricing.col.model')}</span>
              <span className="tl-pr-colhead">{t('pricing.col.rateSource')}</span>
              <span className="tl-pr-colhead num">{t('pricing.col.input')}</span>
              <span className="tl-pr-colhead num">{t('pricing.col.output')}</span>
              <span className="tl-pr-colhead num">{t('pricing.col.cacheRead')}</span>
              <span className="tl-pr-colhead num">{t('pricing.col.cacheWrite')}</span>
              <span />
            </div>

            {rows.map((m) => (
              <ModelRow key={m.model} m={m} onEdit={() => setEditor(m)} t={t} />
            ))}

            {rows.length === 0 && (
              <div className="tl-pr-none">
                <div className="h">{t('pricing.noMatch')}</div>
                <div className="sub">{t('pricing.noMatchSub')}</div>
                <button type="button" onClick={() => { setQuery(''); setFilter('all'); }}>
                  {t('pricing.clearFilters')}
                </button>
              </div>
            )}

            <div className="tl-pr-foot">
              <span>{fill(t('pricing.count'), { shown: rows.length, total: models.length, unpriced: counts.unpriced })}</span>
              <span>{t('pricing.resolutionOrder')}</span>
            </div>
          </div>
        </>
      )}

      {editor && (
        <OverrideEditor
          model={editor}
          pricing={pricing}
          settings={ports?.settings}
          onClose={(changed) => { setEditor(null); if (changed) reload(); }}
        />
      )}
    </div>
  );
}

function ModelRow({ m, onEdit, t }: { m: ModelPricing; onEdit: () => void; t: ReturnType<typeof useT>['t'] }) {
  const state = modelState(m);
  const resolved = resolvedRates(m);
  const meta = toolMeta(m.tool);
  const icon = meta && TOOL_ICONS[meta.key];

  const source =
    state === 'unpriced'
      ? { cls: 'unpriced', text: t('pricing.badge.unpriced') }
      : state === 'override'
        ? { cls: 'override', text: t('pricing.chip.override') }
        : { cls: 'neutral', text: originLabel(m.catalog!.origin) };

  const action =
    state === 'unpriced'
      ? { cls: 'set', text: t('pricing.act.set') }
      : state === 'override'
        ? { cls: 'edit', text: t('pricing.act.edit') }
        : { cls: 'override', text: t('pricing.act.override') };

  return (
    <div className={'tl-pr-grid tl-pr-row' + (state === 'unpriced' ? ' unpriced' : '')}>
      <div className="tl-pr-model">
        <span className={'tl-pr-icon ' + m.tool}>
          {icon ? <img src={icon} alt="" width={14} height={14} /> : <b style={{ color: meta?.color }}>{toolLabel(m.tool)[0]}</b>}
        </span>
        <div style={{ minWidth: 0 }}>
          <div className="name">{m.model}</div>
          <div className="tool">
            <span className="dot" style={{ background: meta?.color ?? 'var(--muted)' }} />
            {toolLabel(m.tool)}
          </div>
        </div>
      </div>

      <div className="tl-pr-badges">
        <span className={'tl-pr-badge ' + source.cls}>{source.text}</span>
        {state === 'est' && <span className="tl-pr-badge cache-est">{t('pricing.badge.cacheEst')}</span>}
      </div>

      {RATE_KEYS.map((key, i) => {
        if (state === 'unpriced') return <span key={key} className="tl-pr-rate faint">—</span>;
        const isEst = state === 'est' && i >= 2;
        return (
          <span key={key} className={'tl-pr-rate' + (isEst ? ' est' : '')}>
            {(isEst ? '≈ ' : '') + fmtRate(resolved![key])}
          </span>
        );
      })}

      <button type="button" className={'tl-pr-act ' + action.cls} onClick={onEdit}>
        {action.text}
      </button>
    </div>
  );
}

function LoadingSkeleton({ note, title }: { note: string; title: string }) {
  return (
    <>
      <div className="tl-pr-toolbar">
        <span className="tl-pr-skel" style={{ width: 250, height: 35 }} />
        <span className="tl-pr-skel" style={{ width: 216, height: 31 }} />
        <span className="tl-pr-spacer" />
        <span className="tl-pr-loading-note">
          <span
            style={{
              width: 12, height: 12, border: '1.6px solid var(--border)', borderTopColor: 'var(--accent)',
              borderRadius: '50%', display: 'inline-block',
            }}
          />
          {note}
        </span>
      </div>
      <div className="tl-pr-card">
        <div className="tl-pr-card-head">
          <span className="title">{title}</span>
        </div>
        {[0, 1, 2, 3, 4].map((i) => (
          <div className="tl-pr-grid tl-pr-row" key={i}>
            <div className="tl-pr-model">
              <span className="tl-pr-skel" style={{ width: 24, height: 24, borderRadius: 7 }} />
              <span className="tl-pr-skel" style={{ width: 130, height: 11 }} />
            </div>
            <span className="tl-pr-skel" style={{ width: 70, height: 18, borderRadius: 6 }} />
            {[0, 1, 2, 3].map((j) => (
              <span key={j} className="tl-pr-skel" style={{ justifySelf: 'end', width: 44, height: 11 }} />
            ))}
            <span className="tl-pr-skel" style={{ justifySelf: 'end', width: 58, height: 24, borderRadius: 7 }} />
          </div>
        ))}
      </div>
    </>
  );
}

function EmptyState({
  title, sub, scanLabel, scanning, onScan,
}: {
  title: string; sub: string; scanLabel: string; scanning: boolean; onScan: () => void;
}) {
  return (
    <div className="tl-pr-empty">
      <svg width="40" height="40" viewBox="0 0 40 40" fill="none" aria-hidden="true">
        <rect x="6" y="22" width="7" height="12" rx="2" stroke="var(--border)" strokeWidth="2" />
        <rect x="17" y="14" width="7" height="20" rx="2" stroke="var(--border)" strokeWidth="2" />
        <rect x="28" y="6" width="7" height="28" rx="2" stroke="var(--muted)" strokeWidth="2" />
      </svg>
      <div className="h">{title}</div>
      <div className="sub">{sub}</div>
      <button type="button" onClick={onScan} disabled={scanning} aria-busy={scanning}>
        <span aria-hidden="true">↻</span>
        {scanLabel}
      </button>
    </div>
  );
}
