// The live-check protocol both surfaces share — the Limits page and the Menu
// Bar Extra panel. It owns the floor, the failure classification, and the
// stored-verdict codec (encoded by `runDueLiveChecks`, decoded by
// `storedFailures`), so the two writers of one storage contract cannot drift.
// It lives beside — not inside — limits.ts, which stays the logic-free port
// seam its own header declares.
import { LIVE_ENABLED_KEY, lastCheckKey, lastFailureKey, type LimitsPort } from './limits';
import { limitsSources } from './limits.derive';

// Decision 5's floor: at most one live check per Source per minute, however
// often a surface asks. Stored (not held in a ref) because both surfaces that
// check are unmounted and remounted constantly, and the floor is a promise to
// the vendor, not to a component.
export const LIVE_FLOOR_MS = 60_000;

const ERROR_FAILURE_PREFIX = 'error:';

// A missing or refused credential is the one failure that reads as "sign in
// again"; every other failure is a failure and must not wear that face. The
// Companion marks the signed-out case with this phrase and this module trusts
// that, rather than re-deriving it from a status code that also appears in
// the text of a genuine error.
function signedOut(detail: string): boolean {
  return /\bnot signed in\b/i.test(detail);
}

export type LiveFailure = 'signed-out' | { detail: string };

/**
 * One live check per due `live` Source. Gated internally on the opt-in and
 * the floor; returns null when nothing is due so a caller can skip its own
 * bracketing. The floor has no override: "a person asked" (a page open, a
 * panel open, a Refresh press) is what permits a call at all, never what
 * exempts it. Never rejects.
 *
 * The write protocol: stamp the floor and FORGET the old verdict as the check
 * starts, then record the classified outcome as it settles. The start-clear
 * is deliberate (and pinned by the page's tests): a fresh stamp must never
 * sit beside a verdict from before the check it stamps, or a remount inside
 * the floor reports an outcome the running check has not reached yet. Its
 * cost is a caller that dies mid-flight — the panel is destroyed on
 * dismissal — leaving the verdict unwritten; that window is bounded by the
 * floor, and the next person-asked check past it re-records. So an empty
 * stored verdict means "no failure recorded", never "confirmed healthy".
 */
export function runDueLiveChecks(
  port: LimitsPort,
  nowMs: number,
  onVerdict?: (source: string, failure: LiveFailure | null) => void,
): Promise<void> | null {
  if (port.read(LIVE_ENABLED_KEY) !== 'true') return null;
  const due = limitsSources()
    .filter(({ via }) => via === 'live')
    .map(({ meta }) => meta.key)
    .filter((key) => nowMs - Number(port.read(lastCheckKey(key)) ?? 0) >= LIVE_FLOOR_MS);
  if (!due.length) return null;

  return Promise.all(
    due.map((key) => {
      port.write(lastCheckKey(key), String(nowMs));
      port.write(lastFailureKey(key), '');
      return Promise.resolve(port.checkLive(key)).then(
        () => {
          port.write(lastFailureKey(key), '');
          onVerdict?.(key, null);
        },
        (err: unknown) => {
          const detail = String((err as { message?: string })?.message ?? err ?? '');
          const failure: LiveFailure = signedOut(detail) ? 'signed-out' : { detail };
          port.write(
            lastFailureKey(key),
            failure === 'signed-out' ? failure : ERROR_FAILURE_PREFIX + failure.detail,
          );
          onVerdict?.(key, failure);
        },
      );
    }),
  ).then(
    () => {},
    () => {},
  );
}

/**
 * The stored verdicts the floor is still holding, decoded — the read half of
 * the codec `runDueLiveChecks` writes. Only a verdict inside the floor may be
 * replayed: past it the next person-asked moment will check again anyway, and
 * a stale verdict would meanwhile suppress held Readings — showing
 * yesterday's blip as a current fact.
 */
export function storedFailures(port: LimitsPort, nowMs: number): Record<string, LiveFailure> {
  const remembered: Record<string, LiveFailure> = {};
  for (const { meta } of limitsSources()) {
    if (nowMs - Number(port.read(lastCheckKey(meta.key)) ?? 0) >= LIVE_FLOOR_MS) continue;
    const value = port.read(lastFailureKey(meta.key));
    if (value === 'signed-out') remembered[meta.key] = 'signed-out';
    else if (value?.startsWith(ERROR_FAILURE_PREFIX)) {
      remembered[meta.key] = { detail: value.slice(ERROR_FAILURE_PREFIX.length) };
    }
  }
  return remembered;
}
