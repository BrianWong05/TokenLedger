// Settings tab (design 1d/1h): four card groups in a 620px column, every change
// persisted immediately through the context (no Save button — the design has
// none). Reads the live Settings from context; keeps only view-local state
// (the rate text field, the app version, the update-check result).
import { useCallback, useEffect, useState, type ReactNode } from 'react';
import { getVersion } from '@tauri-apps/api/app';
import { useT, type StringKey } from '../lib/i18n';
import { useSettings } from './SettingsContext';
import { setLaunchAtLogin } from './startup';
import { REFRESH_PRESETS, useRefreshSec } from '../overview/useAutoRefresh';
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

  return (
    <div className="tl-page tl-page-settings">
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
            <select
              className="set-select"
              aria-label={t('settings.language')}
              value={settings.language}
              onChange={(e) => update({ language: e.target.value as Settings['language'] })}
            >
              <option value="en">English</option>
              <option value="zh-Hant">繁體中文</option>
            </select>
          </div>
        </section>

        <section className="set-group">
          <div className="set-group-label">{t('settings.currencySection')}</div>
          <div className="set-row">
            <div className="set-row-text">
              <div className="set-row-title">{t('settings.currency')}</div>
              <div className="set-row-caption">{t('settings.currency.caption')}</div>
            </div>
            <select
              className="set-select"
              aria-label={t('settings.currency')}
              value={settings.currency}
              onChange={(e) => update({ currency: e.target.value })}
            >
              {CURRENCIES.map(([code, name]) => (
                <option key={code} value={code}>
                  {code} — {name}
                </option>
              ))}
            </select>
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
              {REFRESH_PRESETS.map((p) => (
                <button
                  key={p.sec}
                  type="button"
                  className={refreshSec === p.sec ? 'active' : ''}
                  aria-pressed={refreshSec === p.sec}
                  onClick={() => setRefreshSec(p.sec)}
                >
                  {p.label}
                </button>
              ))}
            </div>
          </div>
        </section>

        <UpdatesGroup port={port} />

        <div className="set-footer-note">{t('settings.footer')}</div>
      </div>
    </div>
  );
}
