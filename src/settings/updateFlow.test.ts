import { describe, expect, it } from 'vitest';
import { isPending, isStaged } from './updateFlow';
import type { UpdateStatus } from '../types';

const status = (state: UpdateStatus['state']): UpdateStatus => ({ state, version: '9.9.9' });

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
