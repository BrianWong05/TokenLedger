// The app shell: one persistent 232px sidebar (traffic-light clearance strip,
// wordmark, three-tab icon nav) owned here, per the dashboard-v2 design, plus the
// settings + theme + i18n providers. Last-scan status + Rescan live in the Overview
// toolbar (dashboard-v2), not the sidebar. Tabs are plain React state — no router.
// Overview stays mounted (hidden)
// across tab switches so its data survives; the Pricing and Settings pages mount on
// demand. Settings state is owned by SettingsProvider so theme + language changes
// take effect live app-wide.
import { lazy, Suspense, useEffect, useState, type ReactNode } from 'react';
import { getVersion } from '@tauri-apps/api/app';
import Overview from './overview/Overview';
import FirstRunDialog from './settings/FirstRunDialog';
import { SettingsProvider, useSettings } from './settings/SettingsContext';
import { I18nProvider, useT } from './lib/i18n';
import { Mark } from './lib/Mark';
import { detectPlatform, type Platform } from './lib/platform';
import { hotkeyHint, isHotkey } from './lib/hotkeys';
import { tauriLedger, type LedgerPort } from './overview/ledger';
import type { ClockPort } from './overview/overviewStore';
import { tauriSettings, type SettingsPort, type UpdateStatus } from './settings/settings';
import type { PricingPort } from './pricing/pricing';
import type { LimitsPort } from './limits/limits';
import './App.css';

const PricingPage = lazy(() => import('./pricing/PricingPage'));
const LimitsPage = lazy(() => import('./limits/LimitsPage'));
const SettingsPage = lazy(() => import('./settings/SettingsPage'));

export interface AppPorts {
  ledger?: LedgerPort;
  clock?: ClockPort;
  settings?: SettingsPort;
  pricing?: PricingPort;
  limits?: LimitsPort;
}

type Tab = 'overview' | 'pricing' | 'limits' | 'settings';

// Icons are the design's inline lucide-style marks (layout / circle-percent /
// gauge / gear); they inherit color from the button so the nav states can tint
// them via CSS.
const TABS: { key: Tab; strKey: 'nav.overview' | 'nav.pricing' | 'nav.limits' | 'nav.settings'; icon: ReactNode }[] = [
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
    key: 'limits',
    strKey: 'nav.limits',
    icon: (
      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <path d="m12 14 4-4" />
        <path d="M3.34 19a10 10 0 1 1 17.32 0" />
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

export default function App({
  ports,
  platform = detectPlatform(),
}: { ports?: AppPorts; platform?: Platform } = {}) {
  const settingsPort = ports?.settings ?? tauriSettings;
  return (
    <SettingsProvider port={settingsPort}>
      <AppInner ports={ports} platform={platform} />
    </SettingsProvider>
  );
}

// Language flows from settings context, so I18nProvider re-renders every string
// the moment the language changes — no reload. The theme is applied inside the
// provider. First-run mounts over everything once the persisted value has loaded
// (so a returning user never flashes the disclosure).
function AppInner({ ports, platform }: { ports?: AppPorts; platform: Platform }) {
  const { settings, loaded } = useSettings();
  return (
    <I18nProvider lang={settings.language}>
      <Shell ports={ports} platform={platform} />
      {loaded && !settings.firstRunDone && <FirstRunDialog />}
    </I18nProvider>
  );
}

function Shell({ ports, platform }: { ports?: AppPorts; platform: Platform }) {
  const { t } = useT();
  const { settings, loaded } = useSettings();
  const [tab, setTab] = useState<Tab>('overview');
  const ledger = ports?.ledger ?? tauriLedger;
  const settingsPort = ports?.settings ?? tauriSettings;

  // The in-app "update available" notice (TOKL-21): one check when the window
  // mounts (each open is a fresh webview, so a resident app re-checks whenever
  // its window reopens), surfaced as a dot on the Settings nav item plus an
  // Orca-style card bottom-right whose Update button drives the same
  // download → restart flow the Settings banner owns. Visiting Settings
  // retires both (the banner takes over there).
  const [updateReady, setUpdateReady] = useState(false);
  const [updateStatus, setUpdateStatus] = useState<UpdateStatus | null>(null);
  const [updateCardGone, setUpdateCardGone] = useState(false);
  const [updating, setUpdating] = useState(false);
  useEffect(() => {
    if (!loaded || !settings.autoCheckUpdates) return;
    let alive = true;
    settingsPort
      .checkUpdates()
      .then((s) => {
        if (alive && (s.state === 'available' || s.state === 'downloaded')) {
          setUpdateReady(true);
          setUpdateStatus(s);
        }
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [loaded, settings.autoCheckUpdates, settingsPort]);
  useEffect(() => {
    if (tab === 'settings') {
      setUpdateReady(false);
      setUpdateCardGone(true);
    }
  }, [tab]);
  // Available: download and stage it; downloaded: restart into it — the same
  // two steps as the Settings banner, from the card.
  const onUpdate = () => {
    if (updateStatus?.state === 'available') {
      setUpdating(true);
      settingsPort
        .downloadUpdate()
        .then(setUpdateStatus)
        .catch(() => {})
        .finally(() => setUpdating(false));
    } else if (updateStatus?.state === 'downloaded') {
      settingsPort.restartApp().catch(() => {});
    }
  };

  // The "you were auto-updated" toast: an applied update only shows itself as
  // the running version differing from the one the previous run recorded, so
  // remember it in localStorage and announce the jump. First run has no record
  // — nothing to announce, just start the memory.
  const [updatedToast, setUpdatedToast] = useState<{ from: string; to: string } | null>(null);
  useEffect(() => {
    let alive = true;
    // getVersion talks to Tauri directly (not a port); route it through a
    // resolved promise so a synchronous off-runtime failure stays a caught
    // rejection (same guard as the Settings page).
    Promise.resolve()
      .then(getVersion)
      .then((v) => {
        let last: string | null = null;
        try {
          last = localStorage.getItem('tl-last-version');
          localStorage.setItem('tl-last-version', v);
        } catch {
          // Storage unavailable: no memory of the prior run, so no announcement.
        }
        if (alive && last && last !== v) setUpdatedToast({ from: last, to: v });
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);

  // The Menu Bar Extra's "Settings… ⌘," item: the tray shows the window and
  // asks the shell to land on the Settings tab.
  useEffect(() => settingsPort.onOpenSettings(() => setTab('settings')), [settingsPort]);

  // The same chord the panel carries, in the window — one shortcut, one action,
  // wherever it is pressed. Rescan's lives in the Overview, which owns the scan.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!isHotkey(e, 'settings', platform)) return;
      e.preventDefault();
      setTab('settings');
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [platform]);

  return (
    <div className="tl-shell">
      {/* the sidebar's own background and its empty stretches double as window
          drag handles (frameless window; drag-region only fires on the exact
          element, so each empty surface carries the attribute) */}
      <aside className="tl-sidebar" data-tauri-drag-region>
        {/* clearance for the native macOS traffic lights (titleBarStyle Overlay);
            also the window's drag handle now that the title bar is hidden.
            Elsewhere the title bar is real and the lights are in it, so the
            strip would hold nothing but a gap. */}
        {platform === 'macos' && (
          <span className="tl-traffic" aria-hidden="true" data-tauri-drag-region />
        )}
        <span className="tl-wordmark">
          <Mark />
          TokenLedger
        </span>
        <nav className="tl-nav">
          {TABS.map((tb) => (
            <button
              key={tb.key}
              className={tb.key === tab ? 'active' : ''}
              aria-current={tb.key === tab ? 'page' : undefined}
              onClick={() => setTab(tb.key)}
              title={tb.key === 'settings' ? hotkeyHint('settings', platform) : undefined}
            >
              {tb.icon}
              {t(tb.strKey)}
              {tb.key === 'settings' && updateReady && (
                <span className="tl-nav-dot" role="status" aria-label={t('nav.updateReady')} />
              )}
            </button>
          ))}
        </nav>
        <span className="tl-nav-spacer" data-tauri-drag-region />
      </aside>

      <main className="tl-main">
        <div className="tl-tab" hidden={tab !== 'overview'}>
          {/* Hidden, not unmounted — so the Overview cannot see for itself that
              it is back on screen, and the same condition is handed to it. */}
          <Overview ports={ports} visible={tab === 'overview'} platform={platform} />
        </div>
        <Suspense fallback={null}>
          {tab === 'pricing' && (
            <PricingPage ports={{ pricing: ports?.pricing, ledger, settings: settingsPort }} />
          )}
          {/* Remounted on every visit by design: opening the page is one of the
              two moments a live limit check is allowed to happen. */}
          {tab === 'limits' && <LimitsPage ports={{ limits: ports?.limits }} />}
          {tab === 'settings' && <SettingsPage port={settingsPort} />}
        </Suspense>
      </main>

      {(updatedToast !== null || (updateStatus !== null && !updateCardGone)) && (
        <div className="tl-toasts">
          {updatedToast && (
            <div className="tl-toast" role="status">
              <div className="tl-toast-head">
                <span className="tl-toast-title">{t('toast.autoUpdated')}</span>
                <button
                  className="tl-toast-close"
                  aria-label={t('toast.dismiss')}
                  onClick={() => setUpdatedToast(null)}
                >
                  ×
                </button>
              </div>
              <p className="tl-toast-detail">
                TokenLedger {updatedToast.from} → {updatedToast.to}
              </p>
            </div>
          )}
          {updateStatus !== null && !updateCardGone && (
            <div className="tl-toast" role="status">
              <div className="tl-toast-head">
                <span className="tl-toast-title">{t('nav.updateReady')}</span>
                <button
                  className="tl-toast-close"
                  aria-label={t('toast.dismiss')}
                  onClick={() => setUpdateCardGone(true)}
                >
                  ×
                </button>
              </div>
              <p className="tl-toast-detail">
                {updateStatus.state === 'downloaded'
                  ? `${updateStatus.version} ${t('settings.updates.downloadedNote')}`
                  : `TokenLedger ${updateStatus.version} ${t('settings.updates.isReady')}`}
              </p>
              <button className="tl-toast-btn" disabled={updating} onClick={onUpdate}>
                {updating
                  ? t('toast.downloading')
                  : updateStatus.state === 'downloaded'
                    ? t('settings.updates.restart')
                    : t('toast.update')}
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
