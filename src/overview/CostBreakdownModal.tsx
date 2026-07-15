import { useEffect, useMemo, useRef } from 'react';
import type { BreakdownRow, Summary } from '../types';
import { buildCostBreakdownView } from './costBreakdown';

interface CostBreakdownModalProps {
  summary: Summary;
  rows: BreakdownRow[];
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

export default function CostBreakdownModal({ summary, rows, onClose }: CostBreakdownModalProps) {
  const view = useMemo(() => buildCostBreakdownView(summary, rows), [summary, rows]);
  const modalRef = useRef<HTMLElement>(null);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
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
      if (previouslyFocused?.isConnected) previouslyFocused.focus();
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
              Estimated total Cost
            </div>
            <div className="tt-cost-modal-total">
              {view.totalCostLabel}
            </div>
            {view.note && <div className="tt-cost-modal-note">{view.note}</div>}
            <div className="tt-cost-modal-disclosure">At API list prices — not billed</div>
          </div>
          <button
            ref={closeButtonRef}
            type="button"
            className="tt-cost-modal-close"
            onClick={onClose}
            aria-label="Close Cost breakdown"
          >
            ×
          </button>
        </header>

        <div className="tt-cost-modal-scroll">
          <div className="tt-cost-modal-columns" aria-hidden="true">
            <span>Model</span>
            <span>Cost</span>
          </div>

          {view.groups.map((group) => (
            <section className="tt-cost-group" key={group.sourceKey}>
              <div className="tt-cost-group-head">
                <span>{group.sourceName}</span>
                <span>{group.costLabel}</span>
              </div>
              {group.models.map((model) => (
                <div className="tt-cost-model" key={model.name}>
                  <span className="tt-cost-model-name">
                    {model.name}
                    {model.cacheEstimated && (
                      <span className="tt-tag" title="Cache-Estimated">
                        cache est.
                      </span>
                    )}
                  </span>
                  <span className={model.unpriced ? 'unpriced' : ''}>{model.costLabel}</span>
                </div>
              ))}
            </section>
          ))}
        </div>
      </section>
    </div>
  );
}
