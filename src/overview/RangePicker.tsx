// The Custom-range picker: presets beside a two-month calendar, opened from the
// Custom segment of whichever toolbar owns it (the Overview's and the usage-trend
// enlarge's both do), anchored under that button. It replaced a full-bleed row of
// two <input type="date">, whose popup the platform draws in its own styling and
// which offered no shortcuts.
//
// The menus are ours rather than <select>s for the same reason: a native option
// list can't take the app's surface, and this one has to sit inside the popover
// without looking like a form control bolted on.
//
// Day cells carry the usage they represent (same sqrt scale as the heatmap), so a
// window can be picked against where the tokens actually are.
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { dailyTotals, isoOf, monthCells, presetsOf } from './data';
import { useCustomPresets } from './customPresets';
import {
  presetLabelL, spanLabelL, RANGE_LABEL_KEY, monthShortL, monthLongL,
  weekdayNarrowL, useOverviewT,
} from './localize';
import { RANGES_8B, type Range8b } from './meta';
import type { Lang } from '../lib/i18n';
import type { SeriesPoint } from '../types';

export interface RangePickerProps {
  /** Rect of the button that opened it; null while closed. */
  at: DOMRect | null;
  /**
   * The button itself. Its own press must not count as an outside click —
   * dismissing on mousedown and reopening on the click that follows would leave
   * the picker permanently open.
   */
  trigger: { current: HTMLElement | null };
  from: string;
  to: string;
  /** Selectable bounds — the first day with data through today. */
  firstIso: string;
  lastIso: string;
  /** Full daily series, for the per-day usage marks. */
  points: SeriesPoint[];
  onPick(from: string, to: string): void;
  onClose(): void;
}

// A heading that opens a menu of months or years. `grid` lays the options out
// three across, so all twelve months fit without scrolling.
function JumpMenu({ label, value, options, onPick, grid, ariaLabel }: {
  label: string;
  value: number;
  options: { v: number; text: string; disabled?: boolean }[];
  onPick(v: number): void;
  grid?: boolean;
  ariaLabel: string;
}) {
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLSpanElement | null>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!box.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);

  return (
    <span className="tt-dp-jump" ref={box}>
      <button
        type="button"
        className={'tt-dp-jump-btn' + (open ? ' open' : '')}
        aria-label={ariaLabel}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        {label}
        <svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="m6 9 6 6 6-6" />
        </svg>
      </button>
      {open && (
        <div className={'tt-dp-menu' + (grid ? ' grid' : '')} role="menu" aria-label={ariaLabel}>
          {options.map((o) => (
            <button
              key={o.v}
              type="button"
              role="menuitemradio"
              aria-checked={o.v === value}
              disabled={o.disabled}
              className={'tt-dp-menu-item' + (o.v === value ? ' on' : '')}
              onClick={() => { onPick(o.v); setOpen(false); }}
            >
              {o.text}
            </button>
          ))}
        </div>
      )}
    </span>
  );
}

function MonthGrid({ y, m, lang, lo, hi, years, selFrom, selTo, heat, onPick, onHover, onJump }: {
  y: number;
  m: number;
  lang: Lang;
  lo: string;
  hi: string;
  years: number[];
  selFrom: string;
  selTo: string;
  heat: (iso: string) => number;
  onPick(iso: string): void;
  onHover(iso: string | null): void;
  onJump(y: number, m: number): void;
}) {
  const { t } = useOverviewT();
  // A month outside the Ledger's extent has no selectable day in it at all, so
  // jumping there would land on a dead grid. A quiet month *inside* the extent
  // is a different thing — its days stay selectable, since a window may span
  // one perfectly well.
  const outside = (ym: string) => ym < lo.slice(0, 7) || ym > hi.slice(0, 7);
  const ymOf = (y: number, m: number) => isoOf(new Date(y, m, 1)).slice(0, 7);

  return (
    <div className="tt-dp-month">
      {/* the heading is the control: month and year each open a menu */}
      <div className="tt-dp-title">
        <JumpMenu
          label={monthLongL(m, lang)}
          ariaLabel={t('overview.month')}
          grid
          value={m}
          options={Array.from({ length: 12 }, (_, i) => ({
            v: i, text: monthShortL(i, lang), disabled: outside(ymOf(y, i)),
          }))}
          onPick={(v) => onJump(y, v)}
        />
        <JumpMenu
          label={String(y)}
          ariaLabel={t('overview.year')}
          value={y}
          options={years.map((yy) => ({ v: yy, text: String(yy), disabled: outside(ymOf(yy, m)) }))}
          onPick={(v) => onJump(v, m)}
        />
      </div>
      <div className="tt-dp-grid">
        {[1, 2, 3, 4, 5, 6, 0].map((dow) => (
          // Monday-first, so the two weekend columns sit together on the right
          <div key={dow} className="tt-dp-dow">{weekdayNarrowL(dow, lang)}</div>
        ))}
        {monthCells(y, m).map((iso, i) => {
          if (!iso) return <div key={i} />;
          const off = iso < lo || iso > hi;
          const inSel = iso >= selFrom && iso <= selTo;
          const h = heat(iso);
          return (
            <button
              key={i}
              type="button"
              data-iso={iso}
              disabled={off}
              tabIndex={iso === selFrom ? 0 : -1}
              aria-pressed={inSel}
              className={'tt-dp-day'
                + (inSel ? ' in' : '')
                + (iso === selFrom ? ' edge start' : '')
                + (iso === selTo ? ' edge end' : '')}
              onMouseEnter={() => onHover(iso)}
              onMouseLeave={() => onHover(null)}
              onFocus={() => onHover(iso)}
              onClick={() => onPick(iso)}
            >
              {Number(iso.slice(8))}
              {h > 0 && <i className="tt-dp-heat" style={{ opacity: 0.25 + h * 0.75 }} aria-hidden="true" />}
            </button>
          );
        })}
      </div>
    </div>
  );
}

export default function RangePicker({ at, trigger, from, to, firstIso, lastIso, points, onPick, onClose }: RangePickerProps) {
  const { t, lang } = useOverviewT();
  const [slots] = useCustomPresets();
  const [anchor, setAnchor] = useState<string | null>(null);
  const [hover, setHover] = useState<string | null>(null);
  // Left month of the pair; the window's end month sits on the right.
  const [view, setView] = useState(() => monthStart(to, -1));
  const wrap = useRef<HTMLDivElement | null>(null);
  const pop = useRef<HTMLDivElement | null>(null);
  const [left, setLeft] = useState(0);
  const [focusAfterPaging, setFocusAfterPaging] = useState<string | null>(null);

  // Abandoning a half-made pick has to drop the pending start, or the next open
  // asks for an end date against a start the reader never chose.
  const close = useCallback(() => { setAnchor(null); onClose(); }, [onClose]);

  // Re-seed the visible months and clear any pending pick whenever it reopens.
  useEffect(() => {
    if (!at) return;
    setAnchor(null);
    setHover(null);
    setView(monthStart(to, -1));
    // `to` is deliberately not a dependency: re-seeding mid-pick would yank the
    // calendar out from under the reader as soon as they committed a range.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [at]);

  useEffect(() => {
    if (!at) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (wrap.current?.contains(t) || trigger.current?.contains(t)) return;
      close();
    };
    // Capture phase: inside the trend enlarge, Escape has to dismiss this and
    // stop, not close the dialog underneath it as well.
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      e.stopPropagation();
      close();
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey, true);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey, true);
    };
  }, [at, close, trigger]);

  // Measure once open: aligned under the trigger, never off the window edge.
  useLayoutEffect(() => {
    if (!at || !pop.current) return;
    setLeft(Math.max(8, Math.min(at.left, window.innerWidth - pop.current.offsetWidth - 12)));
  }, [at]);

  // Hand focus to the day an arrow key paged the calendar to, once it exists.
  useLayoutEffect(() => {
    if (!focusAfterPaging) return;
    wrap.current
      ?.querySelector<HTMLButtonElement>(`.tt-dp-day[data-iso="${focusAfterPaging}"]:not(:disabled)`)
      ?.focus();
    setFocusAfterPaging(null);
  }, [focusAfterPaging]);

  const heat = useMemo(() => {
    const totals = dailyTotals(points);
    const max = Math.max(1, ...Object.values(totals));
    return (iso: string) => Math.sqrt(totals[iso] ?? 0) / Math.sqrt(max);
  }, [points]);

  if (!at) return null;

  const pick = (iso: string) => {
    if (!anchor) { setAnchor(iso); return; }
    const [a, b] = anchor <= iso ? [anchor, iso] : [iso, anchor];
    onPick(a, b);
    setAnchor(null);
    onClose();
  };

  // Mid-pick the grid previews anchor→hover instead of the committed window.
  const [selFrom, selTo] = anchor
    ? (anchor <= (hover ?? anchor) ? [anchor, hover ?? anchor] : [hover ?? anchor, anchor])
    : [from, to];

  const shift = (n: number) => setView((v) => new Date(v.getFullYear(), v.getMonth() + n, 1));
  const right = new Date(view.getFullYear(), view.getMonth() + 1, 1);
  const y0 = Number(firstIso.slice(0, 4));
  const years = Array.from({ length: Number(lastIso.slice(0, 4)) - y0 + 1 }, (_, i) => y0 + i);
  const presets = presetsOf(firstIso, lastIso, slots);

  // Arrow keys walk the grid; a day outside the bounds simply has no button to
  // land on, so the move is a no-op rather than an escape from the window.
  const onGridKey = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const step = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -7, ArrowDown: 7 }[e.key];
    if (!step) return;
    const cur = (e.target as HTMLElement).dataset.iso;
    if (!cur) return;
    e.preventDefault();
    const iso = isoOf(new Date(
      Number(cur.slice(0, 4)), Number(cur.slice(5, 7)) - 1, Number(cur.slice(8)) + step,
    ));
    const next = wrap.current?.querySelector<HTMLButtonElement>(`.tt-dp-day[data-iso="${iso}"]:not(:disabled)`);
    if (next) { next.focus(); return; }
    if (iso < firstIso || iso > lastIso) return;
    // Off the visible pair but still in range: page the calendar, then follow
    // the focus over. Without the hand-off, focus falls to <body> and every
    // further arrow key is dead — the walk would end at the month boundary.
    setView(monthStart(iso, step > 0 ? -1 : 0));
    setFocusAfterPaging(iso);
  };

  return (
    // The popover renders inside its trigger's row, which in the Trend enlarge
    // is a "deep" drag region — the calendar's own surface must not move the
    // window, so it opts out for its whole subtree.
    <div className="tt-dp" ref={wrap} data-tauri-drag-region="false">
      <div
        className="tt-dp-pop"
        ref={pop}
        role="dialog"
        aria-label={t('overview.customRange')}
        style={{ top: at.bottom + 6, left }}
      >
        <div className="tt-dp-presets">
          {presets.map((x) => (
            <button
              key={x.key === 'rolling' ? `rolling-${x.days}` : x.key}
              type="button"
              className={'tt-dp-preset' + (x.from === from && x.to === to ? ' on' : '')}
              onClick={() => { onPick(x.from, x.to); close(); }}
            >
              {presetLabelL(x, lang)}
            </button>
          ))}
        </div>
        <div className="tt-dp-cal">
          <div className="tt-dp-nav">
            <button type="button" onClick={() => shift(-1)} aria-label={t('overview.prevMonth')}>‹</button>
            {/* mid-pick the hint becomes the running span, so the reader does not
                commit a window and then count it */}
            <span aria-live="polite">
              {anchor && hover
                ? <b>{spanLabelL(selFrom, selTo, lang)}</b>
                : t(anchor ? 'overview.pickEnd' : 'overview.pickStart')}
            </span>
            <button type="button" onClick={() => shift(1)} aria-label={t('overview.nextMonth')}>›</button>
          </div>
          <div className="tt-dp-pair" onKeyDown={onGridKey}>
            {[view, right].map((d, i) => (
              <MonthGrid
                key={i}
                y={d.getFullYear()}
                m={d.getMonth()}
                lang={lang}
                lo={firstIso}
                hi={lastIso}
                years={years}
                selFrom={selFrom}
                selTo={selTo}
                heat={heat}
                onPick={pick}
                onHover={setHover}
                // jumping the right-hand month moves the pair back one, so the
                // month picked is the month landed on
                onJump={(yy, mm) => setView(new Date(yy, mm - i, 1))}
              />
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}

// First of the month `offset` months from the one holding `iso`.
function monthStart(iso: string, offset: number): Date {
  return new Date(Number(iso.slice(0, 4)), Number(iso.slice(5, 7)) - 1 + offset, 1);
}

// The range segments with the picker wired to Custom — one component, because
// the Overview toolbar and the trend enlarge's header must behave identically
// and previously kept two copies of this in step by hand. Each host still owns
// its own window state; only the control is shared.
export function RangeSegments({ range, onRange, from, to, firstIso, lastIso, points, onPick }: {
  range: Range8b;
  onRange(r: Range8b): void;
  from: string;
  to: string;
  firstIso: string;
  lastIso: string;
  points: SeriesPoint[];
  onPick(from: string, to: string): void;
}) {
  const { t } = useOverviewT();
  const [at, setAt] = useState<DOMRect | null>(null);
  const trigger = useRef<HTMLElement | null>(null);
  const close = useCallback(() => setAt(null), []);

  return (
    <>
      <div className="tt-seg">
        {RANGES_8B.map((r) => (
          <button
            key={r.key}
            className={range === r.key ? 'active' : ''}
            aria-haspopup={r.key === 'custom' ? 'dialog' : undefined}
            aria-expanded={r.key === 'custom' ? !!at : undefined}
            onClick={(e) => {
              onRange(r.key);
              // Custom carries the picker: no separate row, and choosing the
              // range and its dates is one click instead of two. Any other
              // range dismisses it.
              const el = r.key === 'custom' ? e.currentTarget : null;
              trigger.current = el;
              setAt((cur) => (cur || !el ? null : el.getBoundingClientRect()));
            }}
          >
            {t(RANGE_LABEL_KEY[r.key])}
          </button>
        ))}
      </div>
      <RangePicker
        at={at}
        trigger={trigger}
        from={from}
        to={to}
        firstIso={firstIso}
        lastIso={lastIso}
        points={points}
        onPick={onPick}
        onClose={close}
      />
    </>
  );
}
