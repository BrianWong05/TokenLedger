// First-run disclosure (design 1e). Shown once ever, over everything, when
// settings.firstRunDone is false — it names the launch-at-login default and
// gives an inline opt-out (ADR-0005). OK persists the choice through the context
// and enrolls/unenrolls accordingly.
import { useState } from 'react';
import { useT } from '../lib/i18n';
import { useSettings } from './SettingsContext';
import { setLaunchAtLogin } from './startup';

export default function FirstRunDialog() {
  const { t } = useT();
  const { settings, update } = useSettings();
  // Defaults to the current setting (ON), the disclosed default.
  const [launch, setLaunch] = useState(settings.launchAtLogin);

  const ok = () => {
    update({ firstRunDone: true, launchAtLogin: launch });
    setLaunchAtLogin(launch);
  };

  return (
    <div className="set-firstrun-backdrop">
      <div className="set-firstrun" role="dialog" aria-modal="true" aria-labelledby="set-firstrun-title">
        <span className="set-firstrun-icon" aria-hidden="true">
          <svg width="20" height="20" viewBox="0 0 20 20" fill="none">
            <rect x="2" y="11" width="4" height="7" rx="1" fill="#fff" />
            <rect x="8" y="6" width="4" height="12" rx="1" fill="#fff" />
            <rect x="14" y="2" width="4" height="16" rx="1" fill="#fff" />
          </svg>
        </span>
        <div id="set-firstrun-title" className="set-firstrun-title">
          {t('settings.firstRun.title')}
        </div>
        <div className="set-firstrun-body">{t('settings.firstRun.body')}</div>
        <div className="set-firstrun-toggle">
          <div className="set-row-text">
            <div className="set-row-title">{t('settings.launch')}</div>
            <div className="set-row-caption">{t('settings.firstRun.launchCaption')}</div>
          </div>
          <button
            type="button"
            role="switch"
            aria-checked={launch}
            aria-label={t('settings.launch')}
            className={'set-toggle' + (launch ? ' on' : '')}
            onClick={() => setLaunch((v) => !v)}
          >
            <span className="set-toggle-knob" aria-hidden="true" />
          </button>
        </div>
        <button type="button" className="set-primary-btn set-firstrun-ok" onClick={ok}>
          {t('settings.firstRun.ok')}
        </button>
        <div className="set-firstrun-footnote">{t('settings.firstRun.footnote')}</div>
      </div>
    </div>
  );
}
