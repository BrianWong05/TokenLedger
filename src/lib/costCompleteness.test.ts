import { describe, expect, it } from 'vitest';
import { isAllUnattributedCost, isPartialCost } from './costCompleteness';

describe('Cost completeness', () => {
  it('marks priced Cost Partial for either missing-pricing reason', () => {
    expect(isPartialCost({ cost: 3, hasUnpriced: true, unattributedTokens: 0 })).toBe(true);
    expect(isPartialCost({ cost: 3, hasUnpriced: false, unattributedTokens: 12 })).toBe(true);
    expect(isPartialCost({ cost: 3, hasUnpriced: false, unattributedTokens: 0 })).toBe(false);
  });

  it('distinguishes all-Unattributed unavailable Cost from all-Unpriced Cost', () => {
    expect(isAllUnattributedCost({ cost: null, hasUnpriced: false, unattributedTokens: 12 })).toBe(true);
    expect(isAllUnattributedCost({ cost: null, hasUnpriced: true, unattributedTokens: 0 })).toBe(false);
    expect(isAllUnattributedCost({ cost: null, hasUnpriced: true, unattributedTokens: 12 })).toBe(false);
  });
});
