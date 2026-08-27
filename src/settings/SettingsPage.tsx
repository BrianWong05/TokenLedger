// Settings tab (design 1d/1h): four card groups in a 620px column, every change
// persisted immediately through the context (no Save button — the design has
// none). Reads the live Settings from context, and keeps view-local state beside
// it: the rate text field, the app version, the update-check result. Two groups
// own state the Ledger's Settings never holds — the Overview's presets and
// refresh, and the panel-bars pick, which reads the Limits port for the windows
// to offer and web storage for the choice (see PanelBarsRows).
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Reorder, useDragControls } from 'motion/react';
import { useT, type StringKey } from '../lib/i18n';
import { useSettings } from './SettingsContext';
import { setLaunchAtLogin } from './startup';
import {
  MAX_REFRESH_SEC,
  MIN_REFRESH_SEC,
  REFRESH_OFF,
  REFRESH_PRESETS,
  useRefreshSec,
} from '../overview/useAutoRefresh';
import {
  MAX_CUSTOM_PRESETS,
  SHIPPED_DAYS,
  useCustomPresets,
  validDays,
} from '../overview/customPresets';
import { CALENDAR_PRESETS, isoOf, presetWindow } from '../overview/data';
import { useFirstRecord } from '../overview/ledgerExtent';
import { fmtIsoRangeL, PRESET_LABEL_KEY, presetLabelL, spanLabelL, useOverviewT } from '../overview/localize';
import type { CalendarPresetKey, PresetSlot } from '../overview/data';
import {
  MENU_BAR_REFRESH_OFF,
  MENU_BAR_REFRESH_PRESETS,
  type SettingsPort,
} from './settings';
import { tauriLimits, PANEL_WINDOWS_KEY, type LimitsPort } from '../limits/limits';
import {
  cards,
  panelWindows,
  parsePanelPicks,
  windowLabel,
  type ParsedWindow,
} from '../limits/limits.derive';
import { windowText } from '../limits/limitLabel';
import { fill } from '../lib/format';
import type { Platform } from '../lib/platform';
import type { SourceLimits } from '../types';
import { isPending, useAppVersion, useUpdateFlow } from './updateFlow';
import type { Settings } from '../types';
import './settings.css';

// "CODE — English name" per the design ("HKD — Hong Kong dollar"). ISO codes are
// universal, so these names stay English in both languages.
// Exported for readoutCases.test.ts: every code offered here must have rows
// in readout-cases.json, or the Menu Bar Extra could render it as the
// generic fallback while the panel shows a real symbol.
export const CURRENCIES: [string, string][] = [
  ['USD', 'US dollar'],
  ['HKD', 'Hong Kong dollar'],
  ['EUR', 'Euro'],
  ['GBP', 'Pound sterling'],
  ['JPY', 'Japanese yen'],
  ['CNY', 'Chinese yuan'],
  ['TWD', 'New Taiwan dollar'],
  ['SGD', 'Singapore dollar'],
  ['AUD', 'Australian dollar'],
  ['CAD', 'Canadian dollar'],
  ['KRW', 'South Korean won'],
];

const THEMES: [Settings['theme'], StringKey][] = [
  ['system', 'settings.theme.system'],
  ['light', 'settings.theme.light'],
  ['dark', 'settings.theme.dark'],
];

function Toggle({ on, onClick, label }: { on: boolean; onClick: () => void; label: string }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      className={'set-toggle' + (on ? ' on' : '')}
      onClick={onClick}
    >
      <span className="set-toggle-knob" aria-hidden="true" />
    </button>
  );
}

// A <select> hands its option list to the OS, which draws it in system chrome —
// no app styling reaches it. So the list is ours, the same button + menu shape as
// the range picker's JumpMenu, on the settings surface.
const MENU_MAX_H = 264; // keep in step with .set-menu max-height

function Select({ label, value, options, onPick }: {
  label: string;
  value: string;
  options: { v: string; text: string; disabled?: boolean }[];
  onPick(v: string): void;
}) {
  const [open, setOpen] = useState(false);
  const [up, setUp] = useState(false);
  const box = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!box.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') setOpen(false); };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [open]);

  return (
    <div className="set-select-wrap" ref={box}>
      <button
        type="button"
        className={'set-select' + (open ? ' open' : '')}
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        // the chosen value, which a closed menu otherwise only shows as its label
        data-value={value}
        onClick={(e) => {
          // a row near the window bottom opens its list upwards instead, the way
          // the OS list this replaced did
          const r = e.currentTarget.getBoundingClientRect();
          setUp(window.innerHeight - r.bottom < MENU_MAX_H + 16);
          setOpen((v) => !v);
        }}
      >
        {options.find((o) => o.v === value)?.text ?? ''}
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="m6 9 6 6 6-6" />
        </svg>
      </button>
      {open && (
        <div className={'set-menu' + (up ? ' up' : '')} role="menu" aria-label={label}>
          {options.map((o) => (
            <button
              key={o.v}
              type="button"
              role="menuitemradio"
              aria-checked={o.v === value}
              disabled={o.disabled}
              data-value={o.v}
              className={'set-menu-item' + (o.v === value ? ' on' : '')}
              onClick={() => { onPick(o.v); setOpen(false); }}
            >
              {o.text}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

// The exchange-rate row is only mounted when currency isn't USD, so its text
// state re-seeds from the stored rate each time it appears. Invalid input stays
// editable but is never persisted.
function RateRow({ code }: { code: string }) {
  const { t } = useT();
  const { settings, update } = useSettings();
  const [text, setText] = useState(String(settings.usdRate));

  const onChange = (v: string) => {
    setText(v);
    const n = Number(v);
    if (v.trim() !== '' && Number.isFinite(n) && n > 0) update({ usdRate: n });
  };

  return (
    <div className="set-row">
      <div className="set-row-text">
        <div className="set-row-title">{t('settings.rate')}</div>
        <div className="set-row-caption">{t('settings.rate.caption')}</div>
      </div>
      <div className="set-rate">
        <span className="set-rate-side">1 USD =</span>
        <input
          className="set-rate-input"
          inputMode="decimal"
          aria-label={t('settings.rate')}
          value={text}
          onChange={(e) => onChange(e.target.value)}
        />
        <span className="set-rate-side">{code}</span>
      </div>
    </div>
  );
}

// Up to four extra Presets for the Overview's Custom-range picker: one row to
// add one, then a row per configured Preset, captioned with the window it
// resolves to today — the thing a bare "Last quarter" never tells you. Slots
// stay positional in storage, so removing one in the middle leaves a hole for
// the next Add to fill rather than renumbering the Presets after it.
function CustomRangeGroup() {
  const { t } = useT();
  // The calendar periods are named once, in the overview catalog: this group
  // shows the reader the same words the picker will.
  const { t: overviewT } = useOverviewT();
  const [slots, setSlots] = useCustomPresets();
  const [kind, setKind] = useState<'rolling' | CalendarPresetKey>('rolling');
  const firstRecord = useFirstRecord();
  // A dragged row is clamped to this list: the card below is another group's
  // business, and a Preset carried over it reads as a drop target it is not.
  const list = useRef<HTMLDivElement | null>(null);

  const filled = slots.flatMap((s, i) => (s ? [{ slot: s, i }] : []));
  // Duplicates are blocked by definition rather than by resolved window: two
  // slots cannot hold the same period, and none can restate a shipped one.
  const takenDays = new Set([
    ...SHIPPED_DAYS,
    ...slots.flatMap((s) => (s?.key === 'rolling' ? [s.days] : [])),
  ]);
  const has = (key: CalendarPresetKey) => slots.some((s) => s?.key === key);

  const put = (i: number, slot: PresetSlot | null) => {
    const next = slots.slice();
    next[i] = slot;
    setSlots(next);
  };

  // Reorder writes a new order into the slots that already hold Presets, so the
  // holes between them stay exactly where they are: the reader moves a row past
  // the row above or below it, never past an empty position they cannot see.
  // Dragging and the arrow buttons both come through here, as a permutation of
  // the Presets' own ids — never of the slot positions. A position is exactly
  // what a reorder changes, so keying rows by one leaves React holding a row
  // that "never moved" while its contents swapped underneath, and the drag
  // animates a ghost.
  const reorder = (order: string[]) => {
    const at = filled.map((f) => f.i); // ascending, so the nth row keeps the nth position
    const next = slots.slice();
    order.forEach((id, n) => {
      next[at[n]] = filled.find((f) => presetId(f.slot) === id)?.slot ?? null;
    });
    setSlots(next);
  };

  const move = (pos: number, dir: -1 | 1) => {
    // bounds, not truthiness: the first slot is a perfectly good neighbour
    if (pos + dir < 0 || pos + dir >= filled.length) return;
    const order = filled.map((f) => presetId(f.slot));
    const [moved] = order.splice(pos, 1);
    order.splice(pos + dir, 0, moved);
    reorder(order);
  };

  // The day field's text is what Add commits: out of bounds, already taken or
  // not a number at all simply leaves Add unavailable, so nothing unusable is
  // ever stored and the reader keeps what they typed.
  const [text, setText] = useState(() => String(freeDays(takenDays)));
  const days = Number(text);
  const daysOk = text.trim() !== '' && validDays(days) && !takenDays.has(days);
  const canAdd =
    filled.length < MAX_CUSTOM_PRESETS && (kind === 'rolling' ? daysOk : !has(kind));

  const add = () => {
    const i = slots.findIndex((s) => !s);
    if (i < 0) return;
    put(i, kind === 'rolling' ? { key: 'rolling', days } : { key: kind });
    // A calendar period can only be added once, so the row falls back to the one
    // type that is always available, on a count nothing holds yet. The count just
    // added counts as claimed: the stored slots have not caught up yet.
    setKind('rolling');
    setText(String(freeDays(takenDays, kind === 'rolling' ? days : undefined)));
  };

  // The picker's own resolver against the same extent, so a caption states
  // exactly the window its Preset will select — clamped to the first record, and
  // absent altogether for a period that ends before the Ledger even starts.
  // Until the Overview's first load publishes that extent, the epoch stands in
  // and every Preset states its plain calendar period.
  const extentFrom = firstRecord || '1970-01-01';
  const today = isoOf(new Date());

  return (
    <section className="set-group">
      <div className="set-group-label">{t('settings.customRange')}</div>
      <div className="set-row set-row-stack">
        <div className="set-row-text">
          <div className="set-row-title">{t('settings.preset.add')}</div>
          <div className="set-row-caption">{t('settings.preset.caption')}</div>
        </div>
        <div className="set-rate">
          <div className="set-seg" role="group" aria-label={t('settings.preset.type')}>
            <button
              type="button"
              className={kind === 'rolling' ? 'active' : ''}
              aria-pressed={kind === 'rolling'}
              onClick={() => setKind('rolling')}
            >
              {t('settings.preset.rolling')}
            </button>
            {CALENDAR_PRESETS.map((key) => (
              <button
                key={key}
                type="button"
                className={kind === key ? 'active' : ''}
                aria-pressed={kind === key}
                // already configured: adding it twice would be the same window
                disabled={has(key)}
                onClick={() => setKind(key)}
              >
                {overviewT(PRESET_LABEL_KEY[key])}
              </button>
            ))}
          </div>
          {kind === 'rolling' && (
            <>
              <input
                className="set-rate-input"
                inputMode="numeric"
                aria-label={t('settings.preset.dayCount')}
                value={text}
                onChange={(e) => setText(e.target.value)}
              />
              <span className="set-rate-side">{t('settings.preset.daysUnit')}</span>
            </>
          )}
          <button type="button" className="set-btn" disabled={!canAdd} onClick={add}>
            {t('settings.preset.addAction')}
          </button>
        </div>
      </div>

      {filled.length === 0 && (
        <div className="set-row">
          <div className="set-row-caption">{t('settings.preset.none')}</div>
        </div>
      )}
      <Reorder.Group
        as="div"
        ref={list}
        axis="y"
        values={filled.map((f) => presetId(f.slot))}
        onReorder={reorder}
      >
        {filled.map(({ slot, i }, pos) => (
          <PresetRow
            key={presetId(slot)}
            slot={slot}
            id={presetId(slot)}
            first={pos === 0}
            last={pos === filled.length - 1}
            extentFrom={extentFrom}
            today={today}
            bounds={list}
            // its own count is not a duplicate of itself
            takenDays={new Set([...takenDays].filter((d) => slot.key !== 'rolling' || d !== slot.days))}
            onEdit={(d) => put(i, { key: 'rolling', days: d })}
            onMove={(dir) => move(pos, dir)}
            onRemove={() => put(i, null)}
          />
        ))}
      </Reorder.Group>
    </section>
  );
}

// One configured Preset. Its own component because each row owns the drag
// controls that let the grip — and only the grip — start a drag: a row-wide
// drag listener would swallow the presses meant for its three buttons.
function PresetRow({ slot, id, first, last, extentFrom, today, bounds, takenDays, onEdit, onMove, onRemove }: {
  slot: PresetSlot;
  id: string;
  first: boolean;
  last: boolean;
  extentFrom: string;
  today: string;
  bounds: React.RefObject<HTMLDivElement | null>;
  takenDays: ReadonlySet<number>;
  onEdit: (days: number) => void;
  onMove: (dir: -1 | 1) => void;
  onRemove: () => void;
}) {
  const { t } = useT();
  const { t: overviewT, lang } = useOverviewT();
  const drag = useDragControls();
  const win = presetWindow(slot, extentFrom, today);
  const label = presetLabelL(slot, lang);

  return (
    <Reorder.Item
      as="div"
      value={id}
      dragListener={false}
      dragControls={drag}
      // no fling after release and barely any give at the ends: a four-row list
      // inside a card wants the row to sit where it was let go, not coast
      dragMomentum={false}
      dragConstraints={bounds}
      // no give at all past the list: elastic let the row stretch over the card
      // below, which reads as a drop target it is not
      dragElastic={0}
      // A press anywhere but the buttons must not anchor a text selection. The
      // row's own text is already unselectable, but the anchor still lands and
      // the selection then smears across every other group on the page as the
      // pointer moves.
      onMouseDown={(e) => {
        if (!(e.target as HTMLElement).closest('button, input')) e.preventDefault();
      }}
      className="set-row set-row-preset"
      // lifted off the card while it is being carried, so the row being moved
      // is the one under the cursor rather than one more line in the stack
      whileDrag={{ zIndex: 2, boxShadow: '0 8px 24px rgb(0 0 0 / 0.22)' }}
    >
      {/* decorative: dragging is the mouse's way in, and the arrows below are
          the keyboard's, so a screen reader has nothing to do here */}
      <span
        className="set-grip"
        aria-hidden="true"
        onPointerDown={(e) => drag.start(e)}
      >
        <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
          <circle cx="9" cy="6" r="1.6" /><circle cx="15" cy="6" r="1.6" />
          <circle cx="9" cy="12" r="1.6" /><circle cx="15" cy="12" r="1.6" />
          <circle cx="9" cy="18" r="1.6" /><circle cx="15" cy="18" r="1.6" />
        </svg>
      </span>
      <div className="set-row-text">
        <div className="set-row-title">
          {slot.key === 'rolling' ? (
            <>
              {overviewT('overview.preset.lastN')}{' '}
              <DaysField
                days={slot.days}
                taken={takenDays}
                label={`${t('settings.preset.dayCount')} ${label}`}
                onCommit={onEdit}
              />{' '}
              {overviewT('overview.daysUnit')}
            </>
          ) : (
            label
          )}
        </div>
        <div className="set-row-caption">
          {win
            ? `${fmtIsoRangeL(win.from, win.to, lang)} · ${spanLabelL(win.from, win.to, lang)}`
            : t('settings.preset.outside')}
        </div>
      </div>
      <div className="set-row-tools">
        {/* the picker lists these in this order, so moving a row here is the
            only way to reach a Preset sooner in the picker */}
        <button
          type="button"
          className="set-move"
          aria-label={`${t('settings.preset.moveUp')} ${label}`}
          disabled={first}
          onClick={() => onMove(-1)}
        >
          <Chevron up />
        </button>
        <button
          type="button"
          className="set-move"
          aria-label={`${t('settings.preset.moveDown')} ${label}`}
          disabled={last}
          onClick={() => onMove(1)}
        >
          <Chevron />
        </button>
        <button
          type="button"
          className="set-btn"
          aria-label={`${t('settings.preset.remove')} ${label}`}
          onClick={onRemove}
        >
          {t('settings.preset.remove')}
        </button>
      </div>
    </Reorder.Item>
  );
}

// A rolling Preset's day count, edited in place. Unlike the rate row's field
// this commits on blur or Enter rather than on every valid keystroke: a Preset's
// identity IS its count, so committing mid-word would change the row's key,
// remount it, and take the caret out of the reader's hands between digits. A
// count that is out of bounds or already taken is not a Preset, so the field
// falls back to the stored one rather than leaving a number the row does not
// have.
function DaysField({ days, taken, label, onCommit }: {
  days: number;
  taken: ReadonlySet<number>;
  label: string;
  onCommit: (days: number) => void;
}) {
  const [text, setText] = useState(String(days));

  const commit = () => {
    const n = Number(text);
    if (text.trim() !== '' && validDays(n) && !taken.has(n)) onCommit(n);
    else setText(String(days));
  };

  return (
    <input
      className="set-days"
      inputMode="numeric"
      aria-label={label}
      value={text}
      size={Math.max(2, text.length)}
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === 'Enter') e.currentTarget.blur();
        if (e.key === 'Escape') { setText(String(days)); e.currentTarget.blur(); }
      }}
    />
  );
}

// A configured Preset's identity, which is its content: two slots cannot hold
// the same period or the same day count, so this is unique across the list and —
// unlike a slot position — it travels with the row when the order changes. The
// picker keys its buttons the same way.
function presetId(slot: PresetSlot): string {
  return slot.key === 'rolling' ? `rolling-${slot.days}` : slot.key;
}

// The reorder arrows' mark, the same lucide chevron the Select's button carries.
function Chevron({ up }: { up?: boolean }) {
  return (
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d={up ? 'm18 15-6-6-6 6' : 'm6 9 6 6 6-6'} />
    </svg>
  );
}

// The first day count nothing has claimed, so the field always offers a usable
// one. `justAdded` is claimed too where the stored slots have not caught up yet.
function freeDays(taken: ReadonlySet<number>, justAdded?: number): number {
  let n = 14;
  while (taken.has(n) || n === justAdded) n++;
  return n;
}

// Mounted only while the Custom refresh segment is active, so its text state
// re-seeds from the stored seconds each time it appears (RateRow's contract).
// Invalid input stays editable but is never persisted.
function CustomIntervalRow({ sec, onCommit }: { sec: number; onCommit: (n: number) => void }) {
  const { t } = useT();
  const [text, setText] = useState(String(sec));

  const onChange = (v: string) => {
    setText(v);
    const n = Number(v);
    if (Number.isInteger(n) && n >= MIN_REFRESH_SEC && n <= MAX_REFRESH_SEC) onCommit(n);
  };

  return (
    <div className="set-row">
      <div className="set-row-text">
        <div className="set-row-title">{t('settings.refreshCustom')}</div>
        <div className="set-row-caption">{t('settings.refreshCustom.caption')}</div>
      </div>
      <div className="set-rate">
        <input
          className="set-rate-input"
          inputMode="numeric"
          aria-label={t('settings.refreshCustom')}
          value={text}
          onChange={(e) => onChange(e.target.value)}
        />
        <span className="set-rate-side">{t('settings.refreshCustom.unit')}</span>
      </div>
    </div>
  );
}

function UpdatesGroup({ port }: { port: SettingsPort }) {
  const { t } = useT();
  const { settings, update } = useSettings();
  const version = useAppVersion(port);
  // The banner button drives the user-approved install: download when an update
  // is merely available, then restart to apply it once downloaded.
  const { status, checking, acting, check, act } = useUpdateFlow(port);

  // Populate the last-known state when the tab opens; the button re-checks.
  useEffect(() => {
    check();
  }, [check]);

  const showBanner = isPending(status);

  let caption: ReactNode = null;
  if (status?.state === 'not-configured') {
    caption = t('settings.updates.unconfigured');
  } else if (status?.state === 'up-to-date') {
    caption = <span className="set-ok">{t('settings.updates.upToDate')}</span>;
  } else if (status?.state === 'downloaded') {
    caption = `${status.version} ${t('settings.updates.downloadedNote')}`;
  } else if (status?.state === 'available') {
    caption = `${status.version} ${t('settings.updates.availableNote')}`;
  }

  return (
    <section className="set-group">
      <div className="set-group-label">{t('settings.updates')}</div>

      {showBanner && (
        <div className="set-banner" role="status">
          <span className="set-banner-dot" aria-hidden="true" />
          <div className="set-banner-text">
            <div className="set-banner-title">
              TokenLedger {status?.version} {t('settings.updates.isReady')}
            </div>
            <div className="set-banner-sub">
              {t('settings.updates.downloadedBg')} ·{' '}
              <span className="set-link">{t('settings.updates.releaseNotes')}</span>
            </div>
          </div>
          <button
            type="button"
            className="set-primary-btn"
            onClick={act}
            disabled={acting}
          >
            {t('settings.updates.restart')}
          </button>
        </div>
      )}

      <div className="set-row">
        <div className="set-row-text">
          <div className="set-row-title">{t('settings.autoCheck')}</div>
          <div className="set-row-caption">{t('settings.autoCheck.caption')}</div>
        </div>
        <Toggle
          on={settings.autoCheckUpdates}
          label={t('settings.autoCheck')}
          onClick={() => update({ autoCheckUpdates: !settings.autoCheckUpdates })}
        />
      </div>

      <div className="set-row">
        <div className="set-row-text">
          <div className="set-row-title">
            {t('settings.version')} {version ?? '…'}
          </div>
          <div className="set-row-caption">{caption}</div>
        </div>
        <button type="button" className="set-btn" onClick={check} disabled={checking}>
          {t('settings.checkNow')}
        </button>
      </div>
    </section>
  );
}

/**
 * Which windows each Source shows on the panel's collapsed card — one toggle
 * row per Source that has Readings, pressed for the bars the panel draws.
 *
 * The pressed set is the panel's own answer (`panelWindows`), not a second copy
 * of the default rule, so an untouched Source shows exactly the pair it shows
 * over there; the first click materialises that pair and toggles inside it.
 * Sources with nothing recorded are left out: the panel has no card for them,
 * so there is nothing here to choose between.
 */
function PanelBarsRows({ limits, platform }: { limits: LimitsPort; platform: Platform }) {
  const { t } = useT();
  const [stored, setStored] = useState<SourceLimits[]>([]);
  const [picks, setPicks] = useState<Record<string, string[]>>(() =>
    parsePanelPicks(limits.read(PANEL_WINDOWS_KEY)),
  );

  useEffect(() => {
    let alive = true;
    Promise.resolve(limits.list())
      .then((s) => alive && setStored(Array.isArray(s) ? s : []))
      // No Readings, no rows — the Limits tab is where a Source's trouble is
      // explained, and repeating it in a preference row would only mislead.
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [limits]);

  // The clock only orders the default pair's pool tie-break; a preference row
  // has nothing to keep ticking for, so it reads once per load.
  const rows = useMemo(
    () =>
      cards(stored, Math.floor(Date.now() / 1000), 'left')
        .filter((c) => c.windows.length > 0)
        .map((c) => {
          const parsed: ParsedWindow[] = c.windows.map((w) => ({
            w,
            label: windowLabel(w.key, c.source),
          }));
          return { card: c, parsed };
        }),
    [stored],
  );

  if (!rows.length) return null;

  /**
   * The pick as stored, after an edit to the windows this Source reports today.
   *
   * A key the Source has stopped reporting draws no pill, so it cannot be part
   * of `next` — and rewriting the pick to `next` alone would delete it on the
   * first unrelated click. A vendor window can go missing for a while and come
   * back, so a preference for one outlives its Readings: the absent keys ride
   * along at the end, where they are invisible until the window returns.
   */
  const save = (source: string, next: string[], parsed: ParsedWindow[]) => {
    const reported = new Set(parsed.map((p) => p.w.key));
    const absent = (picks[source] ?? []).filter((key) => !reported.has(key));
    const all = { ...picks, [source]: [...next, ...absent] };
    setPicks(all);
    limits.write(PANEL_WINDOWS_KEY, JSON.stringify(all));
  };

  // Switching one on appends it, so it lands where the panel will draw it last
  // — never silently ahead of bars that were already chosen.
  const toggle = (source: string, key: string, shown: string[], parsed: ParsedWindow[]) =>
    save(source, shown.includes(key) ? shown.filter((k) => k !== key) : [...shown, key], parsed);

  const move = (source: string, key: string, shown: string[], parsed: ParsedWindow[], dir: -1 | 1) => {
    const at = shown.indexOf(key);
    const to = at + dir;
    if (at < 0 || to < 0 || to >= shown.length) return;
    const next = shown.slice();
    next.splice(to, 0, ...next.splice(at, 1));
    save(source, next, parsed);
  };

  return (
    <>
      <div className="set-row">
        <div className="set-row-text">
          <div className="set-row-title">{t('settings.panelBars')}</div>
          <div className="set-row-caption">
            {fill(t('settings.panelBars.caption'), { k: platform === 'macos' ? '⌥' : 'Alt' })}
          </div>
        </div>
      </div>
      {rows.map(({ card, parsed }) => {
        // Chosen bars lead, in the order the panel draws them; the rest wait
        // behind them. The strip IS the running order, so there is no second
        // control to keep in step with the first.
        const chosen = panelWindows(parsed, picks[card.source]);
        const shown = chosen.map((p) => p.w.key);
        const off = parsed.filter((p) => !shown.includes(p.w.key));
        return (
          <div className="set-row set-row-stack" key={card.source}>
            <div className="set-row-title">{card.meta.label}</div>
            <div className="set-seg set-seg-wrap" role="group" aria-label={card.meta.label}>
              {/* `contents` so the items stay flex children of the segment: the
                  group is a drag context, not a box. */}
              <Reorder.Group
                as="div"
                axis="x"
                values={shown}
                onReorder={(next) => save(card.source, next as string[], parsed)}
                style={{ display: 'contents' }}
              >
                {chosen.map(({ w, label }) => (
                  <BarPill
                    key={w.key}
                    value={w.key}
                    // The very words the meter will print, from the panel's own
                    // labeller — picking by one name and being shown another is
                    // what happens when these are assembled twice.
                    label={windowText(t, label).text}
                    onToggle={() => toggle(card.source, w.key, shown, parsed)}
                    onMove={(dir) => move(card.source, w.key, shown, parsed, dir)}
                  />
                ))}
              </Reorder.Group>
              {off.map(({ w, label }) => (
                <button
                  key={w.key}
                  type="button"
                  aria-pressed={false}
                  onClick={() => toggle(card.source, w.key, shown, parsed)}
                >
                  {windowText(t, label).text}
                </button>
              ))}
            </div>
          </div>
        );
      })}
    </>
  );
}

// How far a press may travel and still be a press. Below a pointer's own noise
// on a trackpad, above nothing — the drag it has to be told apart from moves a
// pill's width.
const PRESS_SLOP_PX = 4;

/**
 * One chosen bar: a pressed segment button that is also the drag handle for its
 * own place in the running order.
 *
 * Its own component for the drag/click seam. A pointer that moved is a reorder
 * and a pointer that did not is a toggle, and the browser sends the same click
 * for both — so without a rule the pill switches itself off every time it is
 * dragged, which is the one thing a person dragging it cannot have meant. The
 * rule is the press's own distance, measured here rather than taken from
 * motion's drag callbacks: this element decides what its click meant, and a
 * wiggle too small to have reordered anything stays a toggle either way.
 *
 * The Preset rows solve the same seam with a grip that owns the drag, and that
 * would be the pattern to copy if a pill had room for one — it does not, and a
 * grip inside the button would take the button's own click anyway.
 *
 * Holding the platform's Alt (⌥) with ← or → is the same move from the
 * keyboard, which drag alone leaves with no way to order anything at all (the
 * Preset rows' move buttons are there for exactly that reader). Keyboard
 * activation is judged on `detail`, not on coordinates: a keyboard click carries
 * none, and reads as 0/0 — which is a long way from wherever the last press
 * landed, so distance alone would swallow it. `detail === 0` is what a keyboard
 * click is, and it toggles, as the space bar on a pressed button must.
 */
function BarPill({
  value,
  label,
  onToggle,
  onMove,
}: {
  value: string;
  label: string;
  onToggle: () => void;
  onMove: (dir: -1 | 1) => void;
}) {
  const pressedAt = useRef<{ x: number; y: number } | null>(null);

  return (
    <Reorder.Item
      as="button"
      value={value}
      type="button"
      className="active"
      aria-pressed={true}
      dragMomentum={false}
      dragElastic={0}
      whileDrag={{ zIndex: 2 }}
      onPointerDown={(e) => {
        pressedAt.current = { x: e.clientX, y: e.clientY };
      }}
      onClick={(e) => {
        const from = pressedAt.current;
        pressedAt.current = null;
        const travelled = from && Math.hypot(e.clientX - from.x, e.clientY - from.y) > PRESS_SLOP_PX;
        if (e.detail !== 0 && travelled) return;
        onToggle();
      }}
      onKeyDown={(e) => {
        const dir = e.altKey && e.key === 'ArrowLeft' ? -1 : e.altKey && e.key === 'ArrowRight' ? 1 : null;
        if (dir === null) return;
        e.preventDefault();
        onMove(dir);
      }}
    >
      {label}
    </Reorder.Item>
  );
}

export default function SettingsPage({
  port,
  limits = tauriLimits,
  // The panel-bars caption names a modifier key, and the two desktops spell it
  // differently; the shell already knows which one this is (lib/platform.ts).
  platform = 'macos',
}: {
  port: SettingsPort;
  limits?: LimitsPort;
  platform?: Platform;
}) {
  const { t } = useT();
  const { settings, update } = useSettings();
  const [refreshSec, setRefreshSec] = useRefreshSec();
  // A custom value can equal a preset, so a view-local flag disambiguates which
  // segment is active; Custom is also active whenever the value isn't a preset
  // or Off — Off is not a duration, so it is neither a preset nor a custom one.
  const isPreset = REFRESH_PRESETS.some((p) => p.sec === refreshSec);
  const [customOpen, setCustomOpen] = useState(false);
  const off = refreshSec === REFRESH_OFF && !customOpen;
  const customActive = customOpen || (!isPreset && refreshSec !== REFRESH_OFF);
  const menuBarOff = settings.menuBarRefreshSec === MENU_BAR_REFRESH_OFF;

  return (
    <div className="tl-page tl-page-settings">
      {/* the window's top strip doubles as its drag handle (frameless window);
          pinned so scrolled groups never reach the window top, where a drag
          would select their text instead of moving the window */}
      <span className="tl-set-dragstrip" aria-hidden="true" data-tauri-drag-region />
      <div className="set-col">
        <section className="set-group">
          <div className="set-group-label">{t('settings.appearance')}</div>
          <div className="set-row">
            <div className="set-row-text">
              <div className="set-row-title">{t('settings.theme')}</div>
              <div className="set-row-caption">{t('settings.theme.caption')}</div>
            </div>
            <div className="set-seg" role="group" aria-label={t('settings.theme')}>
              {THEMES.map(([key, strKey]) => (
                <button
                  key={key}
                  type="button"
                  className={settings.theme === key ? 'active' : ''}
                  aria-pressed={settings.theme === key}
                  onClick={() => update({ theme: key })}
                >
                  {t(strKey)}
                </button>
              ))}
            </div>
          </div>
          <div className="set-row">
            <div className="set-row-text">
              <div className="set-row-title">{t('settings.language')}</div>
              <div className="set-row-caption">{t('settings.language.caption')}</div>
            </div>
            <Select
              label={t('settings.language')}
              value={settings.language}
              options={[
                { v: 'en', text: 'English' },
                { v: 'zh-Hant', text: '繁體中文' },
              ]}
              onPick={(v) => update({ language: v as Settings['language'] })}
            />
          </div>
        </section>

        <CustomRangeGroup />

        <section className="set-group">
          <div className="set-group-label">{t('settings.currencySection')}</div>
          <div className="set-row">
            <div className="set-row-text">
              <div className="set-row-title">{t('settings.currency')}</div>
              <div className="set-row-caption">{t('settings.currency.caption')}</div>
            </div>
            <Select
              label={t('settings.currency')}
              value={settings.currency}
              options={CURRENCIES.map(([code, name]) => ({ v: code, text: `${code} — ${name}` }))}
              onPick={(v) => update({ currency: v })}
            />
          </div>
          {settings.currency !== 'USD' && <RateRow code={settings.currency} />}
        </section>

        <section className="set-group">
          <div className="set-group-label">{t('settings.startup')}</div>
          <div className="set-row">
            <div className="set-row-text">
              <div className="set-row-title">{t('settings.launch')}</div>
              <div className="set-row-caption">{t('settings.launch.caption')}</div>
            </div>
            <Toggle
              on={settings.launchAtLogin}
              label={t('settings.launch')}
              onClick={() => {
                const next = !settings.launchAtLogin;
                update({ launchAtLogin: next });
                setLaunchAtLogin(next);
              }}
            />
          </div>
        </section>

        <section className="set-group">
          <div className="set-group-label">{t('settings.scanning')}</div>
          <div className="set-row">
            <div className="set-row-text">
              <div className="set-row-title">{t('settings.refresh')}</div>
              <div className="set-row-caption">{t('settings.refresh.caption')}</div>
            </div>
            <div className="set-seg set-seg-mono" role="group" aria-label={t('settings.refresh')}>
              <button
                type="button"
                className={off ? 'active' : ''}
                aria-pressed={off}
                onClick={() => {
                  setRefreshSec(REFRESH_OFF);
                  setCustomOpen(false);
                }}
              >
                {t('settings.refresh.off')}
              </button>
              {REFRESH_PRESETS.map((p) => {
                const active = refreshSec === p.sec && !customActive;
                return (
                  <button
                    key={p.sec}
                    type="button"
                    className={active ? 'active' : ''}
                    aria-pressed={active}
                    onClick={() => {
                      setRefreshSec(p.sec);
                      setCustomOpen(false);
                    }}
                  >
                    {p.label}
                  </button>
                );
              })}
              <button
                type="button"
                className={customActive ? 'active' : ''}
                aria-pressed={customActive}
                onClick={() => setCustomOpen(true)}
              >
                {t('settings.refresh.custom')}
              </button>
            </div>
          </div>
          {/* Off stops this window scanning on its own, and nothing else — said
              plainly, because "off" on a recording app reads as "stop recording" */}
          {off && (
            <div className="set-row">
              <div className="set-row-caption">{t('settings.refresh.offNote')}</div>
            </div>
          )}
          {customActive && <CustomIntervalRow sec={refreshSec} onCommit={setRefreshSec} />}
        </section>

        {/* The second of the two timers a reader owns, and the one that runs
            with no window open: it paces the scans that keep the figures beside
            the icon current. Unlike the auto-refresh above it is persisted in
            the Ledger's settings, because the Rust loop honouring it outlives
            every webview. */}
        <section className="set-group">
          <div className="set-group-label">{t('settings.menuBar')}</div>
          <div className="set-row">
            <div className="set-row-text">
              <div className="set-row-title">{t('settings.menuBarRefresh')}</div>
              <div className="set-row-caption">{t('settings.menuBarRefresh.caption')}</div>
            </div>
            <div
              className="set-seg set-seg-mono"
              role="group"
              aria-label={t('settings.menuBarRefresh.aria')}
            >
              <button
                type="button"
                className={menuBarOff ? 'active' : ''}
                aria-pressed={menuBarOff}
                onClick={() => update({ menuBarRefreshSec: MENU_BAR_REFRESH_OFF })}
              >
                {t('settings.menuBarRefresh.off')}
              </button>
              {MENU_BAR_REFRESH_PRESETS.map((p) => {
                const active = settings.menuBarRefreshSec === p.sec;
                return (
                  <button
                    key={p.sec}
                    type="button"
                    className={active ? 'active' : ''}
                    aria-pressed={active}
                    onClick={() => update({ menuBarRefreshSec: p.sec })}
                  >
                    {p.label}
                  </button>
                );
              })}
            </div>
          </div>
          {/* Off paces the bar back to the resident floor; it stops no recording */}
          {menuBarOff && (
            <div className="set-row">
              <div className="set-row-caption">{t('settings.menuBarRefresh.offNote')}</div>
            </div>
          )}
          <PanelBarsRows limits={limits} platform={platform} />
        </section>

        <UpdatesGroup port={port} />

        <div className="set-footer-note">{t('settings.footer')}</div>
      </div>
    </div>
  );
}
