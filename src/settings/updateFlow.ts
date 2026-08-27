// The update seam's client side, shared by the Settings banner and the shell's
// update card so one flow serves both: check, then the two user-approved steps
// (download to stage it, restart to apply it).
import { useCallback, useEffect, useState } from 'react';
import type { SettingsPort, UpdateStatus } from './settings';

// The states worth surfacing: an update the reader can act on. 'not-configured'
// and 'up-to-date' are answers, not news.
export function isPending(status: UpdateStatus | null): boolean {
  return status?.state === 'available' || status?.state === 'downloaded';
}

// Staged and waiting for a restart — the one state whose affordance must
// outlive a visit to Settings, because a re-check there reports the staged
// update as merely 'available' again and loses the restart.
export function isStaged(status: UpdateStatus | null): boolean {
  return status?.state === 'downloaded';
}

/** The running app version, or null until it arrives. */
export function useAppVersion(port: SettingsPort): string | null {
  const [version, setVersion] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    port
      .version()
      .then((v) => {
        if (alive) setVersion(v);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [port]);
  return version;
}

export interface UpdateFlow {
  status: UpdateStatus | null;
  /** A check is in flight (the Settings "Check now" button's disabled state). */
  checking: boolean;
  /** A download is in flight (the action button's "Downloading…" state). */
  acting: boolean;
  check: () => void;
  /** Available → download and stage it; downloaded → restart into it. */
  act: () => void;
}

export function useUpdateFlow(port: SettingsPort): UpdateFlow {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [checking, setChecking] = useState(false);
  const [acting, setActing] = useState(false);

  const check = useCallback(() => {
    setChecking(true);
    port
      .checkUpdates()
      .then(setStatus)
      .catch(() => {})
      .finally(() => setChecking(false));
  }, [port]);

  const act = useCallback(() => {
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

  return { status, checking, acting, check, act };
}
