// The app shell: one persistent window header (traffic-light spacing, wordmark,
// three-tab nav, last-scan + Rescan) owned here, per design screens 1a/1d, plus
// the theme + i18n providers. Tabs are plain React state — no router. Overview
// stays mounted (hidden) across tab switches so its data survives; the Pricing
// and Settings pages mount on demand and pull their own data through their own
// ports, so later waves never edit this file.
import { useEffect, useState } from 'react';
import Overview from './overview/Overview';
import PricingPage from './pricing/PricingPage';
import SettingsPage from './settings/SettingsPage';
import { I18nProvider, useT } from './lib/i18n';
import { applyTheme } from './lib/theme';
import { tauriLedger, type LedgerPort } from './overview/ledger';
import type { ClockPort } from './overview/overviewStore';
import { tauriSettings, type SettingsPort } from './settings/settings';
import type { Settings } from './types';
import './App.css';

export interface AppPorts {
  ledger?: LedgerPort;
  clock?: ClockPort;
  settings?: SettingsPort;
}

type Tab = 'overview' | 'pricing' | 'settings';

const TABS: { key: Tab; strKey: 'nav.overview' | 'nav.pricing' | 'nav.settings' }[] = [
  { key: 'overview', strKey: 'nav.overview' },
  { key: 'pricing', strKey: 'nav.pricing' },
  { key: 'settings', strKey: 'nav.settings' },
];

export default function App({ ports }: { ports?: AppPorts } = {}) {
  const settingsPort = ports?.settings ?? tauriSettings;
  const [settings, setSettings] = useState<Settings | null>(null);

  // Load persisted settings once; theme + language flow from them. Until it
  // lands (or if it fails), the CSS defaults hold: dark palette, English.
  useEffect(() => {
    let alive = true;
    settingsPort.get().then((s) => alive && setSettings(s)).catch(() => {});
    return () => { alive = false; };
  }, [settingsPort]);

  const theme = settings?.theme ?? 'system';
  const lang = settings?.language ?? 'en';

  useEffect(() => applyTheme(theme), [theme]);

  return (
    <I18nProvider lang={lang}>
      <Shell ports={ports} />
    </I18nProvider>
  );
}

function Shell({ ports }: { ports?: AppPorts }) {
  const { t } = useT();
  const [tab, setTab] = useState<Tab>('overview');
  const ledger = ports?.ledger ?? tauriLedger;
  const [scanning, setScanning] = useState(false);
  const [lastScanAt, setLastScanAt] = useState<number | null>(null);

  // ponytail: the header Rescan drives a standalone Ledger scan — the distributed
  // app's always-present scan trigger. The Overview keeps its own refresh until
  // the Overview-retrofit wave consolidates the two scan paths.
  const rescan = () => {
    if (scanning) return;
    setScanning(true);
    ledger.scan()
      .then((s) => setLastScanAt(s.scannedAt || Date.now()))
      .catch(() => {})
      .finally(() => setScanning(false));
  };

  const scanLabel = scanning
    ? t('header.scanning')
    : lastScanAt
      ? `${t('header.lastScan')} · ${new Date(lastScanAt).toLocaleTimeString()}`
      : t('header.notScanned');

  return (
    <div className="tl-shell">
      <header className="tl-header" data-tauri-drag-region>
        {/* clears the native traffic lights (titleBarStyle Overlay) */}
        <span className="tl-traffic" aria-hidden="true" />
        <span className="tl-wordmark">
          <svg width="18" height="18" viewBox="0 0 20 20" fill="currentColor" aria-hidden="true">
            <rect x="2" y="11" width="4" height="7" rx="1" />
            <rect x="8" y="6" width="4" height="12" rx="1" />
            <rect x="14" y="2" width="4" height="16" rx="1" />
          </svg>
          TokenLedger
        </span>
        <nav className="tl-nav">
          {TABS.map((tb) => (
            <button
              key={tb.key}
              className={tb.key === tab ? 'active' : ''}
              aria-current={tb.key === tab ? 'page' : undefined}
              onClick={() => setTab(tb.key)}
            >
              {t(tb.strKey)}
            </button>
          ))}
        </nav>
        <span className="tl-spacer" />
        <span className="tl-lastscan">{scanLabel}</span>
        <button
          type="button"
          className="tl-rescan"
          onClick={rescan}
          disabled={scanning}
          aria-busy={scanning}
        >
          <span className="tl-rescan-icon" aria-hidden="true">↻</span>
          {t('header.rescan')}
        </button>
      </header>

      <div className="tl-body">
        <div className="tl-tab" hidden={tab !== 'overview'}>
          <Overview ports={ports} />
        </div>
        {tab === 'pricing' && <PricingPage />}
        {tab === 'settings' && <SettingsPage />}
      </div>
    </div>
  );
}
