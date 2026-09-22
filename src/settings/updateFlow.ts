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

// Downloaded, verified, and one relaunch from running. Rust remembers what it
// staged, so a later check reports the download rather than demoting it back
// to 'available' and taking the restart with it.
export function isStaged(status: UpdateStatus | null): boolean {
  return status?.state === 'downloaded';
}

/** What the action button's click will actually do. */
export type UpdateAction = 'download' | 'downloading' | 'restart';

// Decided once for both surfaces: the shell's card and the Settings banner ask
// here rather than each re-deriving the same three cases from `status`. Each
// then names the three in its own dictionary — same decision, own words.
export function updateAction(status: UpdateStatus | null, acting: boolean): UpdateAction {
  if (acting) return 'downloading';
  return isStaged(status) ? 'restart' : 'download';
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
    if (!isPending(status)) return;
    if (isStaged(status)) {
      port.restartApp().catch(() => {});
      return;
    }
    setActing(true);
    port
      .downloadUpdate()
      .then(setStatus)
      .catch(() => {})
      .finally(() => setActing(false));
  }, [status, port]);

  return { status, checking, acting, check, act };
}
