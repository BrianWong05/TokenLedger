// Settings tab (design 1d/1h): four card groups in a 620px column, every change
// persisted immediately through the context (no Save button — the design has
// none). Reads the live Settings from context; keeps only view-local state
// (the rate text field, the app version, the update-check result).
import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { getVersion } from '@tauri-apps/api/app';
import { useT, type StringKey } from '../lib/i18n';
import { useSettings } from './SettingsContext';
import { setLaunchAtLogin } from './startup';
import {
  MAX_REFRESH_SEC,
  MIN_REFRESH_SEC,
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
import type { SettingsPort, UpdateStatus } from './settings';
import type { Settings } from '../types';
import './settings.css';

// "CODE — English name" per the design ("HKD — Hong Kong dollar"). ISO codes are
// universal, so these names stay English in both languages.
const CURRENCIES: [string, string][] = [
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
  const { t: overviewT, lang } = useOverviewT();
  const [slots, setSlots] = useCustomPresets();
  const [kind, setKind] = useState<'rolling' | CalendarPresetKey>('rolling');
  const firstRecord = useFirstRecord();

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
      {filled.map(({ slot, i }) => {
        const win = presetWindow(slot, extentFrom, today);
        const label = presetLabelL(slot, lang);
        return (
          <div className="set-row" key={i}>
            <div className="set-row-text">
              <div className="set-row-title">{label}</div>
              <div className="set-row-caption">
                {win
                  ? `${fmtIsoRangeL(win.from, win.to, lang)} · ${spanLabelL(win.from, win.to, lang)}`
                  : t('settings.preset.outside')}
              </div>
            </div>
            <button
              type="button"
              className="set-btn"
              aria-label={`${t('settings.preset.remove')} ${label}`}
              onClick={() => put(i, null)}
            >
              {t('settings.preset.remove')}
            </button>
          </div>
        );
      })}
    </section>
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
  const [version, setVersion] = useState<string | null>(null);
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [checking, setChecking] = useState(false);

  useEffect(() => {
    let alive = true;
    // getVersion talks to Tauri directly (not a port). Route it through a
    // resolved promise so even a synchronous failure off-runtime (e.g. jsdom)
    // is a caught rejection, never an unhandled throw.
    Promise.resolve()
      .then(getVersion)
      .then((v) => {
        if (alive) setVersion(v);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);

  const check = useCallback(() => {
    setChecking(true);
    port
      .checkUpdates()
      .then(setStatus)
      .catch(() => {})
      .finally(() => setChecking(false));
  }, [port]);

  // The banner button drives the user-approved install: download when an update
  // is merely available, then restart to apply it once downloaded.
  const [acting, setActing] = useState(false);
  const onBannerAction = useCallback(() => {
    if (status?.state === 'available') {
      setActing(true);
      port
        .downloadUpdate()
        .then(setStatus)
        .catch(() => {})
        .finally(() => setActing(false));
    } else if (status?.state === 'downloaded') {
      port.restartApp().catch(() => {});
    }
  }, [status?.state, port]);

  // Populate the last-known state when the tab opens; the button re-checks.
  useEffect(() => {
    check();
  }, [check]);

  const showBanner = status?.state === 'available' || status?.state === 'downloaded';

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
            onClick={onBannerAction}
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

export default function SettingsPage({ port }: { port: SettingsPort }) {
  const { t } = useT();
  const { settings, update } = useSettings();
  const [refreshSec, setRefreshSec] = useRefreshSec();
  // A custom value can equal a preset, so a view-local flag disambiguates which
  // segment is active; Custom is also active whenever the value isn't a preset.
  const isPreset = REFRESH_PRESETS.some((p) => p.sec === refreshSec);
  const [customOpen, setCustomOpen] = useState(false);
  const customActive = customOpen || !isPreset;

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
          {customActive && <CustomIntervalRow sec={refreshSec} onCommit={setRefreshSec} />}
        </section>

        <UpdatesGroup port={port} />

        <div className="set-footer-note">{t('settings.footer')}</div>
      </div>
    </div>
  );
}
