import { describe, expect, it } from 'vitest';
import { isNewerVersion, isPending, isStaged } from './updateFlow';
import type { UpdateStatus } from '../types';

const status = (state: UpdateStatus['state']): UpdateStatus => ({ state, version: '9.9.9' });

describe('isNewerVersion', () => {
  it('reads a climb as news and everything else as silence', () => {
    expect(isNewerVersion('1.4.2', '1.0.0')).toBe(true);
    expect(isNewerVersion('2.0.0', '1.9.9')).toBe(true);
    // The one a string comparison gets wrong: 10 is a bigger number than 9,
    // even though "1" sorts before "9".
    expect(isNewerVersion('1.10.0', '1.9.0')).toBe(true);
    // Same version: the ordinary run, nothing to announce.
    expect(isNewerVersion('1.4.2', '1.4.2')).toBe(false);
    // A rollback, or a record left by a newer install than this one.
    expect(isNewerVersion('1.0.0', '1.4.2')).toBe(false);
    expect(isNewerVersion('1.9.0', '1.10.0')).toBe(false);
  });
});

describe('update status', () => {
  it('counts only the two actionable states as pending', () => {
    expect(isPending(status('available'))).toBe(true);
    expect(isPending(status('downloaded'))).toBe(true);
    expect(isPending(status('up-to-date'))).toBe(false);
    expect(isPending(status('not-configured'))).toBe(false);
    expect(isPending(null)).toBe(false);
  });

  it('counts only a downloaded update as staged', () => {
    expect(isStaged(status('downloaded'))).toBe(true);
    expect(isStaged(status('available'))).toBe(false);
    expect(isStaged(null)).toBe(false);
  });
});
