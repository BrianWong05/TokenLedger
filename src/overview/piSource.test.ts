import { describe, expect, it } from 'vitest';
import { bucketsFromPoints, seriesToDays, smallMultiples, toolTotalsOfPoints } from './data';
import { seriesPoint } from './seriesPoint';

const piPoint = seriesPoint({
  bucket: '2026-07-10',
  source: 'pi',
  byModel: { 'pi-response-model': 239 },
  inputTokens: 135,
  outputTokens: 62,
  cacheReadTokens: 23,
  cacheWriteTokens: 19,
  totalTokens: 239,
  reasoningTokens: 10,
  cost: 0.000805,
  requests: 3,
  convs: 1,
});

describe('pi Source derivation', () => {
  it('carries pi through Overview totals, Trend, and Activity as the eighth Source', () => {
    expect(toolTotalsOfPoints([piPoint]).pi).toBe(239);

    const buckets = bucketsFromPoints([piPoint], 'day');
    expect(buckets).toHaveLength(1);
    expect(buckets[0].byTool.pi).toBe(239);
    expect(smallMultiples(buckets)).toEqual([
      expect.objectContaining({ key: 'pi', label: 'pi', source: 'pi', total: 239 }),
    ]);

    const day = seriesToDays([piPoint], new Date(2026, 6, 10)).find(
      (candidate) => candidate.iso === '2026-07-10',
    )!;
    expect(day.tokens).toBe(239);
    expect(day.byTool.pi).toBe(239);
  });
});
