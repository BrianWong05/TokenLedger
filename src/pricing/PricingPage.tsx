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
import { sourceIcon } from '../overview/icons';
import OverrideEditor from './OverrideEditor';
import {
  modelState, resolvedRates, overallRate, filterModels, chipCounts, fmtRate,
  pricingSourceMeta, sourceLabel, originLabel, isRoutedRate, fill,
  sortPricing, defaultDir, type PriceFilter, type SortCol, type SortDir,
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
  // Sort is ephemeral (TOKL-10): opens on Model A–Z, resets each mount. Clicking
  // a column header re-sorts; clicking the active header flips direction.
  const [sort, setSort] = useState<{ col: SortCol; dir: SortDir }>({ col: 'model', dir: 'asc' });
  const [editor, setEditor] = useState<ModelPricing | null>(null);
  const [scanning, setScanning] = useState(false);
  const [refreshing, setRefreshing] = useState(false);

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
  const rows = useMemo(
    () => (models ? sortPricing(filterModels(models, query, filter), sort.col, sort.dir) : []),
    [models, query, filter, sort],
  );
  // Toggle direction on the active column; otherwise switch column at its
  // natural opening direction.
  const onSort = useCallback(
    (col: SortCol) =>
      setSort((s) => (s.col === col ? { col, dir: s.dir === 'asc' ? 'desc' : 'asc' } : { col, dir: defaultDir(col) })),
    [],
  );

  const scanNow = () => {
    if (scanning) return;
    setScanning(true);
    ledger.scan().then(reload).catch(() => {}).finally(() => setScanning(false));
  };

  // The catalogs are otherwise read at launch and when a scan turns up a Model
  // with no rate. This forces a re-read for the case neither covers: a rate
  // published for a Model we already gave up on this run. reload() rather than
  // leaning on prices-rebuilt, so the table updates even if that event is missed.
  const refreshCatalogs = () => {
    if (refreshing) return;
    setRefreshing(true);
    pricing.refresh().then(reload).catch(() => {}).finally(() => setRefreshing(false));
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
          {/* the toolbar pins to the window top and its empty stretch is a
              window-drag handle (frameless window); mousedown on the child
              controls does not start a drag */}
          <div className="tl-pr-toolbar" data-tauri-drag-region>
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
            {/* the empty stretch doubles as the drag handle, like the shell's
                tl-nav-spacer */}
            <span className="tl-pr-spacer" data-tauri-drag-region />
            <span className="tl-pr-catnote">{t('pricing.catalogNote')}</span>
            <button
              type="button"
              className="tl-pr-refresh"
              onClick={refreshCatalogs}
              disabled={refreshing}
              title={t('pricing.refreshTitle')}
            >
              {t(refreshing ? 'pricing.refreshing' : 'pricing.refresh')}
            </button>
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

            {/* header + rows scroll as one unit so the columns stay aligned */}
            <div className="tl-pr-scroll">
              <div className="tl-pr-grid">
                <SortHead col="model" label={t('pricing.col.model')} sort={sort} onSort={onSort} t={t} />
                <SortHead col="source" label={t('pricing.col.rateSource')} sort={sort} onSort={onSort} t={t} />
                <SortHead col="overall" label={t('pricing.col.overall')} num sort={sort} onSort={onSort} t={t} />
                <SortHead col="input" label={t('pricing.col.input')} num sort={sort} onSort={onSort} t={t} />
                <SortHead col="output" label={t('pricing.col.output')} num sort={sort} onSort={onSort} t={t} />
                <SortHead col="cacheRead" label={t('pricing.col.cacheRead')} num sort={sort} onSort={onSort} t={t} />
                <SortHead col="cacheWrite" label={t('pricing.col.cacheWrite')} num sort={sort} onSort={onSort} t={t} />
                <span />
              </div>

              {rows.map((m) => (
                <ModelRow key={m.model} m={m} onEdit={() => setEditor(m)} t={t} />
              ))}
            </div>

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

// A sortable column header (TOKL-10): a sort button showing a direction arrow
// only on the active column. The grid is plain divs (no table/grid roles), where
// aria-sort would be inert — so the current sort state rides in the accessible
// NAME instead, which assistive tech always announces. `num` right-aligns the
// rate columns to match their cells.
function SortHead({
  col, label, num = false, sort, onSort, t,
}: {
  col: SortCol;
  label: string;
  num?: boolean;
  sort: { col: SortCol; dir: SortDir };
  onSort: (col: SortCol) => void;
  t: ReturnType<typeof useT>['t'];
}) {
  const active = sort.col === col;
  const ariaLabel = active
    ? fill(t(sort.dir === 'asc' ? 'pricing.sortedAsc' : 'pricing.sortedDesc'), { col: label })
    : fill(t('pricing.sortBy'), { col: label });
  return (
    <button
      type="button"
      className={'tl-pr-colhead' + (num ? ' num' : '') + (active ? ' sorted' : '')}
      aria-label={ariaLabel}
      onClick={() => onSort(col)}
    >
      {label}
      <span className="tl-pr-sortarrow" aria-hidden="true">{active ? (sort.dir === 'asc' ? '↑' : '↓') : ''}</span>
    </button>
  );
}

function ModelRow({ m, onEdit, t }: { m: ModelPricing; onEdit: () => void; t: ReturnType<typeof useT>['t'] }) {
  const state = modelState(m);
  const resolved = resolvedRates(m);
  const overall = overallRate(m);
  const meta = pricingSourceMeta(m.tool);
  const icon = sourceIcon(meta.icon);

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
          {icon ? <img src={icon} alt="" width={14} height={14} /> : <b style={{ color: meta.color }}>{sourceLabel(m.tool)[0]}</b>}
        </span>
        <div style={{ minWidth: 0 }}>
          <div className="name">{m.model}</div>
          <div className="tool">
            <span className="dot" style={{ background: meta.color }} />
            {sourceLabel(m.tool)}
          </div>
        </div>
      </div>

      <div className="tl-pr-badges">
        <span className={'tl-pr-badge ' + source.cls}>{source.text}</span>
        {/* A Routed Rate is nobody's published price, so it must not sit in this
            column looking like one. Qualified in place, the way Cache est. is. */}
        {isRoutedRate(m) && (
          <span className="tl-pr-badge routed" title={t('pricing.badge.routedTitle')}>
            {t('pricing.badge.routed')}
          </span>
        )}
        {state === 'est' && <span className="tl-pr-badge cache-est">{t('pricing.badge.cacheEst')}</span>}
      </div>

      {/* Overall = sum of the four resolved rates; ≈ when cache-estimated (a
          partial sum), — when the Model has no resolved rate at all. */}
      <span className={'tl-pr-overall' + (state === 'est' ? ' est' : '') + (overall == null ? ' faint' : '')}>
        {overall == null ? '—' : (state === 'est' ? '≈ ' : '') + fmtRate(overall)}
      </span>

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
      <div className="tl-pr-toolbar" data-tauri-drag-region>
        <span className="tl-pr-skel" style={{ width: 250, height: 35 }} />
        <span className="tl-pr-skel" style={{ width: 216, height: 31 }} />
        <span className="tl-pr-spacer" data-tauri-drag-region />
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
        <div className="tl-pr-scroll">
          {[0, 1, 2, 3, 4].map((i) => (
            <div className="tl-pr-grid tl-pr-row" key={i}>
              <div className="tl-pr-model">
                <span className="tl-pr-skel" style={{ width: 24, height: 24, borderRadius: 7 }} />
                <span className="tl-pr-skel" style={{ width: 130, height: 11 }} />
              </div>
              <span className="tl-pr-skel" style={{ width: 70, height: 18, borderRadius: 6 }} />
              {/* Overall + the four rate columns: one skeleton per track, or the
                  action lands under Cache write and the last column sits empty. */}
              {[0, 1, 2, 3, 4].map((j) => (
                <span key={j} className="tl-pr-skel" style={{ justifySelf: 'end', width: 44, height: 11 }} />
              ))}
              <span className="tl-pr-skel" style={{ justifySelf: 'end', width: 58, height: 24, borderRadius: 7 }} />
            </div>
          ))}
        </div>
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
