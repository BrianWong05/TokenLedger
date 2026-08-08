import { useLayoutEffect, useRef, type RefObject } from 'react';

// Shared dialog chrome for the app's modals: page lock (no scroll, no background
// text selection), a focus trap (Tab/Shift-Tab cycle inside the dialog),
// Escape-to-close, and return-focus-to-opener on unmount. Mount-scoped — call
// it from a component that is rendered only while its dialog is open.
//
// Initial focus lands on initialFocusRef (falling back to the dialog itself);
// on close, focus returns to returnFocusRef — WebKit doesn't focus buttons on
// click, so activeElement-at-open is usually <body> and only serves as the
// fallback when no opener ref is connected.

// While a modal is open the page behind it stays inert: no scrolling and no
// text selection (body.tl-dialog-open; index.css lets the dialog itself opt
// back in). Scroll is locked at the input level — wheel/touchmove/scroll-keys
// are swallowed at the document — deliberately NOT via overflow on <html> /
// <body>: WKWebView resets the document scroll offset when the scroller's
// overflow flips to hidden, which made opening a dialog jump the page to the
// top. Scrolls aimed at a scrollable box inside the dialog still go through.
// Standalone export so dialogs that own their own focus/Escape handling (the
// Pricing OverrideEditor) can still lock the page.
const SCROLL_KEYS = new Set([' ', 'ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'PageUp', 'PageDown', 'Home', 'End']);

// True when the event target sits in a scrollable box inside the dialog —
// the only scroll the lock lets through.
function inDialogScroller(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return false;
  const dialog = target.closest('[role="dialog"]');
  if (!dialog) return false;
  for (let el: Element | null = target; el && el !== dialog; el = el.parentElement) {
    const s = getComputedStyle(el);
    if (/(auto|scroll)/.test(s.overflowY + s.overflowX) && el.scrollHeight > el.clientHeight) return true;
  }
  return false;
}

export function useModalPageLock(): void {
  useLayoutEffect(() => {
    const pageBody = document.body;
    pageBody.classList.add('tl-dialog-open');

    const onWheel = (e: WheelEvent) => { if (!inDialogScroller(e.target)) e.preventDefault(); };
    const onTouchMove = (e: TouchEvent) => { if (!inDialogScroller(e.target)) e.preventDefault(); };
    const onKeyDown = (e: KeyboardEvent) => {
      if (!SCROLL_KEYS.has(e.key) || e.metaKey || e.ctrlKey || e.altKey) return;
      if (e.target instanceof Element && e.target.closest('[role="dialog"]')) return;
      e.preventDefault();
    };
    document.addEventListener('wheel', onWheel, { passive: false });
    document.addEventListener('touchmove', onTouchMove, { passive: false });
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('wheel', onWheel);
      document.removeEventListener('touchmove', onTouchMove);
      document.removeEventListener('keydown', onKeyDown);
      pageBody.classList.remove('tl-dialog-open');
    };
  }, []);
}

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

  useModalPageLock();

  useLayoutEffect(() => {
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
      const focusTarget = returnFocusRef.current ?? previouslyFocused;
      if (focusTarget?.isConnected) focusTarget.focus();
    };
  }, []);
}
