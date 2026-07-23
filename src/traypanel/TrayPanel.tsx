// The Menu Bar Extra panel (design 2b, pixel-faithful — ADR-0007): a small
// frameless webview the tray toggles, styled exactly like the mock. Data goes
// through the same ports the app shell uses; the four actions are Tauri glue.
// UI strings are English-only like the native menu it replaced; number and
// currency formatting still follow the app's locale. Dark-only like the mock.
import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window';
import { dayWindows, panelModel, type PanelModel } from './panelModel';
import { tauriLedger, type LedgerPort } from '../overview/ledger';
import { tauriSettings, type SettingsPort } from '../settings/settings';
import type { Filters } from '../types';
import './TrayPanel.css';

export interface TrayPanelPorts {
  ledger?: LedgerPort;
  settings?: SettingsPort;
}

const PANEL_WIDTH = 300;

// Fire-and-forget IPC for the action rows; harmless outside Tauri (tests).
function ipc(cmd: string) {
  Promise.resolve()
    .then(() => invoke(cmd))
    .catch(() => {});
}

export default function TrayPanel({ ports }: { ports?: TrayPanelPorts } = {}) {
  const ledger = ports?.ledger ?? tauriLedger;
  const settings = ports?.settings ?? tauriSettings;
  const [model, setModel] = useState<PanelModel | null>(null);
  const [scanning, setScanning] = useState(false);
  const bodyRef = useRef<HTMLDivElement>(null);

  const refresh = useCallback(async () => {
    try {
      const w = dayWindows(new Date());
      const today: Filters = { tools: [], models: [], project: null, startTs: w.todayStart, endTs: w.todayEnd };
      const yday: Filters = { tools: [], models: [], project: null, startTs: w.yStart, endTs: w.yEnd };
      const [t, y, rows, s] = await Promise.all([
        ledger.summary(today),
        ledger.summary(yday),
        ledger.breakdown('tool', today),
        settings.get(),
      ]);
      setModel(panelModel(t, y, rows, s, s.language === 'zh-Hant' ? 'zh-Hant' : 'en'));
    } catch {
      // Ledger unavailable (e.g. mid-restart): keep the last model.
    }
  }, [ledger, settings]);

  // Mark the document so TrayPanel.css can force the window transparent
  // over index.css's app background (see the css header comment).
  useEffect(() => {
    document.body.classList.add('tp-window');
    return () => document.body.classList.remove('tp-window');
  }, []);

  // Initial load + refetch every time the tray shows the panel.
  useEffect(() => {
    void refresh();
    let un: (() => void) | undefined;
    Promise.resolve()
      .then(() => listen('panel-shown', () => void refresh()))
      .then((f) => { un = f; })
      .catch(() => {});
    return () => un?.();
  }, [refresh]);

  // Size the window to the rendered content (the row count varies by day).
  useEffect(() => {
    const h = bodyRef.current?.offsetHeight;
    if (!h) return;
    Promise.resolve()
      .then(() => getCurrentWindow().setSize(new LogicalSize(PANEL_WIDTH, h)))
      .catch(() => {});
  }, [model]);

  const rescan = async () => {
    if (scanning) return; // coalesce double-clicks; scans serialize anyway
    setScanning(true);
    try {
      await ledger.scan();
    } catch {
      /* scan errors surface in the Overview, not here */
    }
    await refresh();
    setScanning(false);
  };

  return (
    <div className="tp" ref={bodyRef}>
      <div className="tp-header">
        <div className="tp-caption-row">
          <span className="tp-caption">Today</span>
          {model?.delta && (
            <span className={model.deltaUp ? 'tp-delta up' : 'tp-delta down'}>{model.delta}</span>
          )}
        </div>
        {model?.empty ? (
          <div className="tp-empty">No usage yet</div>
        ) : (
          <div className="tp-cost-row">
            <span className="tp-cost">{model?.cost ?? '…'}</span>
            <span className="tp-sub">{model?.sub ?? ''}</span>
          </div>
        )}
      </div>

      {model && model.rows.length > 0 && (
        <>
          <div className="tp-sep" />
          {model.rows.map((r) => (
            <div className="tp-row" key={r.key}>
              {r.icon ? <img src={r.icon} alt="" width={13} height={13} /> : <span className="tp-icon-gap" />}
              <span className="tp-row-label">{r.label}</span>
              <span className="tp-spacer" />
              <span className="tp-row-tokens">{r.tokens}</span>
              <span className="tp-row-cost">{r.cost}</span>
            </div>
          ))}
        </>
      )}

      <div className="tp-sep" />
      <button className="tp-action" onClick={() => ipc('show_main')}>
        Open TokenLedger
      </button>
      <button className="tp-action" onClick={() => void rescan()} disabled={scanning}>
        Rescan now
        {scanning ? (
          // 1b's refresh glyph, spinning while the scan runs.
          <svg
            className="tp-spin"
            width="12"
            height="12"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-label="scanning"
          >
            <path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8" />
            <path d="M21 3v5h-5" />
            <path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16" />
            <path d="M8 16H3v5" />
          </svg>
        ) : (
          <span className="tp-key">⇧⌘R</span>
        )}
      </button>
      <div className="tp-sep" />
      <button className="tp-action" onClick={() => ipc('open_settings')}>
        Settings…<span className="tp-key">⌘,</span>
      </button>
      <button className="tp-action" onClick={() => ipc('quit_app')}>
        Quit TokenLedger<span className="tp-key">⌘Q</span>
      </button>
    </div>
  );
}
