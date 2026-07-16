// App-wide live settings. The provider owns the one Settings value (loaded once
// through the port, exactly as the shell did before), and hands every consumer
// an update(patch) that optimistically re-renders, persists the whole object,
// and applies the theme. Language flows out of here too, so the I18nProvider
// wrapped around the tree re-renders on a language change with no reload.
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { applyTheme } from '../lib/theme';
import { DEFAULT_SETTINGS, type SettingsPort } from './settings';
import type { Settings } from '../types';

interface SettingsCtx {
  settings: Settings;
  update: (patch: Partial<Settings>) => void;
  // Whether the persisted value has loaded yet; the first-run gate waits on it
  // so a returning user never flashes the disclosure before their real settings
  // (firstRunDone: true) arrive.
  loaded: boolean;
}

const Ctx = createContext<SettingsCtx | null>(null);

export function SettingsProvider({
  port,
  children,
}: {
  port: SettingsPort;
  children: ReactNode;
}) {
  const [settings, setSettings] = useState<Settings>(DEFAULT_SETTINGS);
  const [loaded, setLoaded] = useState(false);
  // Latest value, so rapid successive updates compose off each other rather than
  // a stale closure, and each persists exactly once.
  const ref = useRef(settings);

  useEffect(() => {
    let alive = true;
    port
      .get()
      .then((s) => {
        if (!alive) return;
        ref.current = s;
        setSettings(s);
        setLoaded(true);
      })
      .catch(() => {
        // Get failed — keep the CSS/English defaults; don't nag with first-run.
      });
    return () => {
      alive = false;
    };
  }, [port]);

  useEffect(() => applyTheme(settings.theme), [settings.theme]);

  const update = useCallback(
    (patch: Partial<Settings>) => {
      const next = { ...ref.current, ...patch };
      ref.current = next;
      setSettings(next);
      port.set(next).catch(() => {});
    },
    [port],
  );

  return <Ctx.Provider value={{ settings, update, loaded }}>{children}</Ctx.Provider>;
}

export function useSettings(): SettingsCtx {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error('useSettings must be used within a SettingsProvider');
  return ctx;
}
