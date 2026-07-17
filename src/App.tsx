// The app shell: one persistent 232px sidebar (traffic-light clearance strip,
// wordmark, three-tab icon nav, then last-scan + full-width Rescan pinned to the
// bottom) owned here, per the dashboard-v2 design, plus the settings + theme + i18n
// providers. Tabs are plain React state — no router. Overview stays mounted (hidden)
// across tab switches so its data survives; the Pricing and Settings pages mount on
// demand. Settings state is owned by SettingsProvider so theme + language changes
// take effect live app-wide.
import { useState, type ReactNode } from 'react';
import Overview from './overview/Overview';
import PricingPage from './pricing/PricingPage';
import SettingsPage from './settings/SettingsPage';
import FirstRunDialog from './settings/FirstRunDialog';
import { SettingsProvider, useSettings } from './settings/SettingsContext';
import { I18nProvider, useT } from './lib/i18n';
import { tauriLedger, type LedgerPort } from './overview/ledger';
import type { ClockPort } from './overview/overviewStore';
import { tauriSettings, type SettingsPort } from './settings/settings';
import type { PricingPort } from './pricing/pricing';
import './App.css';

export interface AppPorts {
  ledger?: LedgerPort;
  clock?: ClockPort;
  settings?: SettingsPort;
  pricing?: PricingPort;
}

type Tab = 'overview' | 'pricing' | 'settings';

// Icons are the design's inline lucide-style marks (layout / circle-percent / gear);
// they inherit color from the button so the nav states can tint them via CSS.
const TABS: { key: Tab; strKey: 'nav.overview' | 'nav.pricing' | 'nav.settings'; icon: ReactNode }[] = [
  {
    key: 'overview',
    strKey: 'nav.overview',
    icon: (
      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <rect width="7" height="9" x="3" y="3" rx="1" />
        <rect width="7" height="5" x="14" y="3" rx="1" />
        <rect width="7" height="9" x="14" y="12" rx="1" />
        <rect width="7" height="5" x="3" y="16" rx="1" />
      </svg>
    ),
  },
  {
    key: 'pricing',
    strKey: 'nav.pricing',
    icon: (
      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <path d="M13.744 17.736a6 6 0 1 1-7.48-7.48" />
        <path d="M15 6h1v4" />
        <path d="m6.134 14.768.866-.5 2 3.464" />
        <circle cx="16" cy="8" r="6" />
      </svg>
    ),
  },
  {
    key: 'settings',
    strKey: 'nav.settings',
    icon: (
      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915" />
        <circle cx="12" cy="12" r="3" />
      </svg>
    ),
  },
];

export default function App({ ports }: { ports?: AppPorts } = {}) {
  const settingsPort = ports?.settings ?? tauriSettings;
  return (
    <SettingsProvider port={settingsPort}>
      <AppInner ports={ports} />
    </SettingsProvider>
  );
}

// Language flows from settings context, so I18nProvider re-renders every string
// the moment the language changes — no reload. The theme is applied inside the
// provider. First-run mounts over everything once the persisted value has loaded
// (so a returning user never flashes the disclosure).
function AppInner({ ports }: { ports?: AppPorts }) {
  const { settings, loaded } = useSettings();
  return (
    <I18nProvider lang={settings.language}>
      <Shell ports={ports} />
      {loaded && !settings.firstRunDone && <FirstRunDialog />}
    </I18nProvider>
  );
}

function Shell({ ports }: { ports?: AppPorts }) {
  const { t } = useT();
  const [tab, setTab] = useState<Tab>('overview');
  const ledger = ports?.ledger ?? tauriLedger;
  const settingsPort = ports?.settings ?? tauriSettings;
  const [scanning, setScanning] = useState(false);
  const [lastScanAt, setLastScanAt] = useState<number | null>(null);

  // ponytail: the sidebar Rescan drives a standalone Ledger scan — the distributed
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
      {/* the sidebar's own background and its empty stretches double as window
          drag handles (frameless window; drag-region only fires on the exact
          element, so each empty surface carries the attribute) */}
      <aside className="tl-sidebar" data-tauri-drag-region>
        {/* clearance for the native macOS traffic lights (titleBarStyle Overlay);
            also the window's drag handle now that the title bar is hidden */}
        <span className="tl-traffic" aria-hidden="true" data-tauri-drag-region />
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
              {tb.icon}
              {t(tb.strKey)}
            </button>
          ))}
        </nav>
        <span className="tl-nav-spacer" data-tauri-drag-region />
        <div className="tl-scanbox">
          <span className="tl-lastscan">{scanLabel}</span>
          <button
            type="button"
            className="tl-rescan"
            onClick={rescan}
            disabled={scanning}
            aria-busy={scanning}
          >
            <svg className="tl-rescan-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8" />
              <path d="M21 3v5h-5" />
              <path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16" />
              <path d="M8 16H3v5" />
            </svg>
            {t('header.rescan')}
          </button>
        </div>
      </aside>

      <main className="tl-main">
        <div className="tl-tab" hidden={tab !== 'overview'}>
          <Overview ports={ports} />
        </div>
        {tab === 'pricing' && (
          <PricingPage ports={{ pricing: ports?.pricing, ledger, settings: settingsPort }} />
        )}
        {tab === 'settings' && <SettingsPage port={settingsPort} />}
      </main>
    </div>
  );
}
