import { useLayoutEffect, useRef, type RefObject } from 'react';

// Shared dialog chrome for the Overview's modals: page scroll lock, a focus
// trap (Tab/Shift-Tab cycle inside the dialog), Escape-to-close, and
// return-focus-to-opener on unmount. Mount-scoped — call it from a component
// that is rendered only while its dialog is open.
//
// Initial focus lands on initialFocusRef (falling back to the dialog itself);
// on close, focus returns to returnFocusRef — WebKit doesn't focus buttons on
// click, so activeElement-at-open is usually <body> and only serves as the
// fallback when no opener ref is connected.

const FOCUSABLE_SELECTOR = [
  'button:not([disabled])',
  '[href]',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

export function useDialogChrome({
  modalRef,
  initialFocusRef,
  returnFocusRef,
  onClose,
}: {
  modalRef: RefObject<HTMLElement | null>;
  initialFocusRef: RefObject<HTMLElement | null>;
  returnFocusRef: RefObject<HTMLElement | null>;
  onClose: () => void;
}): void {
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useLayoutEffect(() => {
    const pageRoot = document.documentElement;
    const pageBody = document.body;
    const previousRootOverflow = pageRoot.style.overflow;
    const previousBodyOverflow = pageBody.style.overflow;
    pageRoot.style.overflow = 'hidden';
    pageBody.style.overflow = 'hidden';

    const previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const modal = modalRef.current;
    (initialFocusRef.current ?? modal)?.focus();

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
}
