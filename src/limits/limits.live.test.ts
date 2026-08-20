// The shared live-check protocol (limits.live.ts): the opt-in gate, the
// per-Source floor, the verdict codec, and the one property the page tests
// cannot pin — a caller that dies mid-check (the Menu Bar Extra panel is
// destroyed on dismissal) leaves an unwritten verdict that the next
// person-asked check past the floor re-records.
import { describe, expect, it } from 'vitest';
import { LIVE_ENABLED_KEY, lastCheckKey, lastFailureKey, type LimitsPort } from './limits';
import { LIVE_FLOOR_MS, runDueLiveChecks, storedFailures, type LiveFailure } from './limits.live';
import { limitsSources } from './limits.derive';

const T0 = 1_786_492_800_000;

function fakePort(
  store: Record<string, string> = {},
  checkLive: (source: string) => Promise<void> = () => Promise.resolve(),
): LimitsPort & { store: Map<string, string>; checked: string[] } {
  const map = new Map(Object.entries(store));
  const checked: string[] = [];
  return {
    store: map,
    checked,
    list: () => Promise.resolve([]),
    checkLive: (s) => {
      checked.push(s);
      return checkLive(s);
    },
    scan: () => Promise.resolve(),
    onLimitsChanged: () => () => {},
    read: (k) => map.get(k) ?? null,
    write: (k, v) => {
      map.set(k, v);
    },
  };
}

const liveKeys = () =>
  limitsSources()
    .filter(({ via }) => via === 'live')
    .map(({ meta }) => meta.key);

describe('runDueLiveChecks', () => {
  it('does nothing at all before the opt-in', () => {
    const port = fakePort();
    expect(runDueLiveChecks(port, T0)).toBeNull();
    expect(port.checked).toEqual([]);
    expect(port.store.size).toBe(0);
  });

  it('checks every live Source once, and the floor holds a repeat', async () => {
    const port = fakePort({ [LIVE_ENABLED_KEY]: 'true' });
    await runDueLiveChecks(port, T0);
    expect([...port.checked].sort()).toEqual([...liveKeys()].sort());
    for (const key of liveKeys()) {
      expect(port.store.get(lastCheckKey(key))).toBe(String(T0));
    }
    expect(runDueLiveChecks(port, T0 + LIVE_FLOOR_MS - 1)).toBeNull();
    expect(port.checked.length).toBe(liveKeys().length);
  });

  it('classifies and records each verdict as it settles', async () => {
    const verdicts: Record<string, LiveFailure | null> = {};
    const port = fakePort({ [LIVE_ENABLED_KEY]: 'true' }, (source) => {
      if (source === 'claude') return Promise.reject(new Error('not signed in: no sign-in found'));
      if (source === 'codex') return Promise.reject(new Error('the vendor answered 500'));
      return Promise.resolve();
    });
    await runDueLiveChecks(port, T0, (source, failure) => {
      verdicts[source] = failure;
    });
    expect(port.store.get(lastFailureKey('claude'))).toBe('signed-out');
    expect(port.store.get(lastFailureKey('codex'))).toBe('error:the vendor answered 500');
    expect(verdicts.claude).toBe('signed-out');
    expect(verdicts.codex).toEqual({ detail: 'the vendor answered 500' });
    expect(verdicts.copilot).toBeNull();
    expect(port.store.get(lastFailureKey('copilot'))).toBe('');
  });

  it('re-records a verdict at the next check past the floor after a caller died mid-check', async () => {
    // First check: nobody awaits and the promise never settles — the panel
    // was destroyed. The stamp and the start-clear land; the verdict never
    // does, so the record inside the floor reads "no failure recorded".
    let dead = true;
    const port = fakePort(
      {
        [LIVE_ENABLED_KEY]: 'true',
        [lastFailureKey('claude')]: 'signed-out',
        [lastCheckKey('claude')]: String(T0 - 24 * 3600 * 1000),
      },
      (source) =>
        dead
          ? new Promise<void>(() => {})
          : source === 'claude'
            ? Promise.reject(new Error('not signed in: no sign-in found'))
            : Promise.resolve(),
    );
    runDueLiveChecks(port, T0);
    expect(port.store.get(lastFailureKey('claude'))).toBe('');
    expect(storedFailures(port, T0 + 5_000)).toEqual({});

    // Past the floor the next person-asked moment checks again, and the
    // verdict comes back — the lost write costs at most one floor's width.
    dead = false;
    await runDueLiveChecks(port, T0 + LIVE_FLOOR_MS);
    expect(port.store.get(lastFailureKey('claude'))).toBe('signed-out');
    expect(storedFailures(port, T0 + LIVE_FLOOR_MS + 5_000)).toEqual({ claude: 'signed-out' });
  });
});

describe('storedFailures', () => {
  it('replays only the verdicts the floor still holds, decoded', () => {
    const port = fakePort({
      [lastFailureKey('claude')]: 'signed-out',
      [lastCheckKey('claude')]: String(T0),
      [lastFailureKey('codex')]: 'error:exit 3',
      [lastCheckKey('codex')]: String(T0),
      // Past the floor: says nothing about now, must not be replayed.
      [lastFailureKey('copilot')]: 'signed-out',
      [lastCheckKey('copilot')]: String(T0 - LIVE_FLOOR_MS),
    });
    expect(storedFailures(port, T0 + 1_000)).toEqual({
      claude: 'signed-out',
      codex: { detail: 'exit 3' },
    });
  });
});
