import type { EstimateEpochSummary } from '../bindings/EstimateEpochSummary';
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

/**
 * A Ready evaluation: `core` epochs inside the stable core and `spare` weighed
 * but left out of it. The two counts are separate on purpose — the row reports
 * core membership, and a fixture where the two agree could not tell the
 * difference.
 */
export function makeReadyEstimate(
  tokensPerPct: number,
  core = 4,
  spare = 0,
): LimitEstimateEvaluation {
  const base = makeFakeEstimate();
  return {
    ...base,
    state: 'ready',
    tokensPerPct,
    explanation: {
      ...base.explanation,
      reasonCodes: [],
      qualifyingEpochs: core,
      newestCompletedEpochAt: NOW - 3600,
      candidates: [
        ...Array.from({ length: core }, (_, i) => epoch(i, true)),
        ...Array.from({ length: spare }, (_, i) => epoch(core + i, false)),
      ],
    },
  };
}

function epoch(i: number, inCore: boolean): EstimateEpochSummary {
  return {
    epochKey: `epoch-${i}`,
    endedAt: NOW - (i + 1) * 24 * 3600,
    movementPoints: 12,
    positiveMovements: 3,
    inCore,
  };
}
