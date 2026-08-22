import type { SeriesPoint } from '../types';

// Test fixture for a series point. One place carries the SeriesPoint shape so
// a new field does not fan out across every suite that builds a point.
export function seriesPoint(over: Partial<SeriesPoint> = {}): SeriesPoint {
  return {
    bucket: '2026-07-16',
    source: 'claude',
    byModel: {},
    unattributedTokens: 0,
    hasUnpriced: false,
    cacheEstimated: false,
    inputTokens: 0,
    outputTokens: 0,
    cacheReadTokens: 0,
    cacheWriteTokens: 0,
    totalTokens: 0,
    reasoningTokens: null,
    cost: 0,
    requests: 0,
    convs: 0,
    ...over,
  };
}
