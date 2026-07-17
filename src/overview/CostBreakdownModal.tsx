import { useCallback, useLayoutEffect, useMemo, useRef, useState, type RefObject } from 'react';
import type { BreakdownRow, Summary } from '../types';
import { buildCostBreakdownView } from './costBreakdown';
import { useSettings } from '../settings/SettingsContext';
import { useOverviewT } from './localize';

interface CostBreakdownModalProps {
  summary: Summary;
  rows: BreakdownRow[];
  returnFocusRef: RefObject<HTMLElement>;
  onClose: () => void;
}

const FOCUSABLE_SELECTOR = [
  'button:not([disabled])',
  '[href]',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

export default function CostBreakdownModal({
  summary,
  rows,
  returnFocusRef,
  onClose,
}: CostBreakdownModalProps) {
  const { settings } = useSettings();
  const { t, lang } = useOverviewT();
  const view = useMemo(
    () => buildCostBreakdownView(summary, rows, settings, lang),
    [summary, rows, settings, lang],
  );
  const modalRef = useRef<HTMLElement>(null);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  // The current group lives in permanent chrome outside the scroller. Keeping
  // that row mounted makes its geometry stable in WebKit; the first in-list
  // group head is omitted so the source name is never duplicated.
  const scrollRef = useRef<HTMLDivElement>(null);
  const [pinnedIdx, setPinnedIdx] = useState(0);
  const handleScroll = useCallback(() => {
    const scroller = scrollRef.current;
    if (!scroller) return;
    const viewportTop = scroller.getBoundingClientRect().top;
    let current = 0;
    const sections = scroller.querySelectorAll<HTMLElement>('.tt-cost-group');
    sections.forEach((section, i) => {
      if (i === 0) return;
      const head = section.querySelector<HTMLElement>('.tt-cost-group-head');
      // Switch only after the incoming real head leaves the viewport, avoiding
      // a duplicate source name at the handoff.
      if (head && head.getBoundingClientRect().bottom <= viewportTop) current = i;
    });
    setPinnedIdx(current);
  }, []);
  const pinned = view.groups[pinnedIdx] ?? view.groups[0] ?? null;

  useLayoutEffect(() => {
    const pageRoot = document.documentElement;
    const pageBody = document.body;
    const previousRootOverflow = pageRoot.style.overflow;
    const previousBodyOverflow = pageBody.style.overflow;
    pageRoot.style.overflow = 'hidden';
    pageBody.style.overflow = 'hidden';

    const previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const modal = modalRef.current;
    (closeButtonRef.current ?? modal)?.focus();

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        onCloseRef.current();
        return;
      }
      if (event.key !== 'Tab' || !modal) return;

      const focusable = Array.from(modal.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR));
      if (focusable.length === 0) {
        event.preventDefault();
        modal.focus();
        return;
      }

      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      const active = document.activeElement;
      const focusIsOutside = !active || !modal.contains(active);

      if (event.shiftKey && (active === first || focusIsOutside)) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && (active === last || focusIsOutside)) {
        event.preventDefault();
        first.focus();
      }
    };

    document.addEventListener('keydown', handleKeyDown);
    return () => {
      document.removeEventListener('keydown', handleKeyDown);
      pageRoot.style.overflow = previousRootOverflow;
      pageBody.style.overflow = previousBodyOverflow;
      const focusTarget = returnFocusRef.current ?? previouslyFocused;
      if (focusTarget?.isConnected) focusTarget.focus();
    };
  }, []);

  return (
    <div
      className="tt-cost-modal-backdrop"
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <section
        ref={modalRef}
        className="tt-cost-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="tt-cost-modal-title"
        tabIndex={-1}
      >
        <header className="tt-cost-modal-head">
          <div>
            <div className="tt-cost-modal-eyebrow" id="tt-cost-modal-title">
              {t('overview.estTotalCost')}
            </div>
            <div className="tt-cost-modal-total">
              {view.totalCostLabel}
            </div>
            {view.note && <div className="tt-cost-modal-note">{view.note}</div>}
            <div className="tt-cost-modal-disclosure">{t('overview.notBilled')}</div>
          </div>
          <button
            ref={closeButtonRef}
            type="button"
            className="tt-cost-modal-close"
            onClick={onClose}
            aria-label={t('overview.closeCostBreakdown')}
          >
            ×
          </button>
        </header>

        {/* fixed chrome: always pinned right under the title block, outside the
            scroller so it can never flash or scroll away */}
        <div className="tt-cost-modal-columns" aria-hidden="true">
          <span>{t('overview.col.model')}</span>
          <span>{t('overview.col.cost')}</span>
        </div>

        <div className="tt-cost-list">
          {pinned && (
            <div className="tt-cost-pinned-head">
              <span>{pinned.sourceName}</span>
              <span>{pinned.costLabel}</span>
            </div>
          )}
          <div className="tt-cost-modal-scroll" ref={scrollRef} onScroll={handleScroll}>
          {view.groups.map((group, groupIndex) => (
            <section className="tt-cost-group" key={group.sourceKey}>
              {groupIndex > 0 && (
                <div className="tt-cost-group-head">
                  <span>{group.sourceName}</span>
                  <span>{group.costLabel}</span>
                </div>
              )}
              {group.models.map((model) => (
                <div className="tt-cost-model" key={model.name}>
                  <span className="tt-cost-model-name">
                    {model.name}
                    {model.cacheEstimated && (
                      <span className="tt-tag" title={t('overview.cacheEstimated')}>
                        {t('overview.cacheEst')}
                      </span>
                    )}
                  </span>
                  <span className={model.unpriced ? 'unpriced' : ''}>{model.costLabel}</span>
                </div>
              ))}
            </section>
          ))}
          </div>
        </div>
      </section>
    </div>
  );
}
