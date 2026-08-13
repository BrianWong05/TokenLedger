import type { LimitEstimateEvaluation } from '../bindings/LimitEstimateEvaluation';

// 2026-08-12T00:00:00Z, the instant the Limits fixtures are written around.
const NOW = 1_786_492_800;

/**
 * A tagged evaluation for tests that are not about the estimate. Gathering is
 * the honest default: it is what a Limit says before its evidence exists, and
 * it carries no number for a test to accidentally depend on.
 */
export function estimate(over: Partial<LimitEstimateEvaluation> = {}): LimitEstimateEvaluation {
  return {
    state: 'gathering',
    evaluatedAt: NOW,
    nextEvaluationAt: null,
    policyVersion: 'limit-token-estimate-v1',
    explanation: {
      reasonCodes: ['insufficient-recent-epochs'],
      rejections: [],
      qualifyingEpochs: 0,
      requiredEpochs: 3,
      recentCutoffAt: NOW - 7 * 24 * 3600,
      newestCompletedEpochAt: null,
      candidates: [],
      ratioRange: null,
      quantizationIntersection: null,
    },
    ...over,
  };
}
