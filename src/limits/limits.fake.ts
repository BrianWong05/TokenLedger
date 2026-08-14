import type { LimitEstimateEvaluation } from '../bindings/LimitEstimateEvaluation';

// An arbitrary instant. No test asserts it: what these fixtures are for is a
// window that needs an evaluation, not an evaluation that needs checking.
const NOW = 1_786_492_800;

/**
 * A tagged evaluation for tests that are not about the estimate. Gathering is
 * the honest default: it is what a Limit says before its evidence exists, and
 * it carries no number for a test to accidentally depend on.
 */
export function makeFakeEstimate(over: Partial<LimitEstimateEvaluation> = {}): LimitEstimateEvaluation {
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
