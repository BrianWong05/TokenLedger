import { describe, expect, it } from 'vitest';
import { panelModel, periodWindows, seriesBucket, type PanelExtras } from './panelModel';
import { DEFAULT_SETTINGS } from '../settings/settings';
import type { BreakdownRow, SeriesPoint, Summary } from '../types';

function sum(
  totalTokens: number,
  cost: number | null,
  hasUnpriced = false,
  requests = 0,
  unattributedTokens = 0,
): Summary {
  return {
    inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0,
    totalTokens, requests, cost, hasUnpriced, unattributedTokens,
    unpricedModels: [], cacheEstimatedModels: [], cacheHitRate: 0,
  };
}

function brow(
  key: string,
  totalTokens: number,
  cost: number | null,
  hasUnpriced = false,
  unattributedTokens = 0,
): BreakdownRow {
  return {
    key, source: null, inputTokens: 0, outputTokens: 0, cacheReadTokens: 0,
    cacheWriteTokens: 0, totalTokens, requests: 0, cost, reasoningTokens: null,
    convs: 0, cacheEstimated: false, hasUnpriced, unattributedTokens,
  };
}

function mrow(
  key: string,
  source: string,
  totalTokens: number,
  cost: number | null,
  hasUnpriced = false,
): BreakdownRow {
  return { ...brow(key, totalTokens, cost, hasUnpriced), source };
}

// One (bucket, Source) point, the shape `series` returns.
function spt(bucket: string, cost: number, totalTokens = 1_000, hasUnpriced = false): SeriesPoint {
  return {
    bucket, source: 'claude', byModel: {}, unattributedTokens: 0, hasUnpriced,
    inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0,
    totalTokens, reasoningTokens: null, cost, requests: 0, convs: 0,
    ctxMessages: null, ctxSystem: null, ctxReasoning: null, ctxToolcalls: null,
    ctxAgents: null, ctxMcp: null, ctxSkills: null,
  };
}

const NOW = new Date(2026, 5, 15, 10, 30, 0); // June 15, 10:30 local

function extras(over: Partial<PanelExtras> = {}): PanelExtras {
  return {
    period: 'today', now: NOW, models: [], projects: [], series: [], scannedAt: 0, ...over,
  };
}

const S = DEFAULT_SETTINGS;

describe('panelModel', () => {
  it('renders the 2b header: cost, delta vs same-time-yesterday, tokens and requests', () => {
    const m = panelModel(sum(3_400_000, 12.84, false, 1912), sum(1_000_000, 10.0), [], S, 'en');
    expect(m.cost).toBe('$12.84');
    expect(m.delta).toBe('+28.4%'); // 12.84 / 10 → +28.4, one decimal
    expect(m.deltaUp).toBe(true);
    expect(m.sub).toBe('3.4M tok · 1,912 req');
  });

  it('falling pace reads negative and not-up', () => {
    const m = panelModel(sum(3_400_000, 9.0), sum(1_000_000, 10.0), [], S, 'en');
    expect(m.delta).toBe('-10.0%');
    expect(m.deltaUp).toBe(false);
  });

  it('delta hidden when yesterday-so-far had no cost — zero or unpriced', () => {
    expect(panelModel(sum(1, 5), sum(0, 0), [], S, 'en').delta).toBeNull();
    expect(panelModel(sum(1, 5), sum(9, null, true), [], S, 'en').delta).toBeNull();
    expect(panelModel(sum(1, null, true), sum(9, 10), [], S, 'en').delta).toBeNull();
  });

  it('partial cost carries the marker; all-unpriced says unpriced, never $0', () => {
    expect(panelModel(sum(1, 12.8, true), sum(0, null), [], S, 'en').cost).toBe('≥ $12.80');
    expect(panelModel(sum(1, null, true), sum(0, null), [], S, 'en').cost).toBe('unpriced');
  });

  it('distinguishes mixed and all-Unattributed Cost from Unpriced Models', () => {
    expect(panelModel(sum(1, 12.8, false, 0, 1), sum(0, null), [], S, 'en').cost).toBe('≥ $12.80');
    expect(panelModel(sum(1, null, false, 0, 1), sum(0, null), [], S, 'en').cost).toBe('unavailable');
  });

  it('honors Display Currency like every other Cost in the app', () => {
    const m = panelModel(sum(1, 10.0), sum(0, null), [], { ...S, currency: 'HKD', usdRate: 7.8 }, 'en');
    expect(m.cost).toBe('HK$78.00');
  });

  it('empty day is flagged and shows no delta even when yesterday had usage', () => {
    const m = panelModel(sum(0, 0), sum(9, 10.0), [], S, 'en');
    expect(m.empty).toBe(true);
    expect(m.delta).toBeNull(); // never "No usage yet" beside "-100.0%"
  });

  it('source rows: zero-usage absent, cost desc, all-unpriced last by tokens, per-row ≥', () => {
    const m = panelModel(sum(1, 1), sum(0, null), [
      brow('codex', 238_100, 1.11),
      brow('gemini', 0, null),
      brow('grok', 964_200, null, true),
      brow('hermes', 500_000, 2.0),
      brow('claude', 1_800_000, 6.12, true), // mixed → partial
    ], S, 'en');
    expect(m.rows.map((r) => [r.label, r.tokens, r.cost])).toEqual([
      ['Claude', '1.8M', '≥ $6.12'],
      ['Hermes', '500K', '$2.00'],
      ['Codex', '238.1K', '$1.11'],
      ['Grok', '964.2K', 'unpriced'],
    ]);
  });

  it('marks Source rows Partial or unavailable for Unattributed Usage', () => {
    const m = panelModel(sum(150, 1, false, 0, 50), sum(0, null), [
      brow('claude', 100, 1, false, 25),
      brow('codex', 50, null, false, 50),
    ], S, 'en');

    expect(m.rows.map((r) => [r.label, r.cost])).toEqual([
      ['Claude', '≥ $1.00'],
      ['Codex', 'unavailable'],
    ]);
  });

  it('exposes raw values and per-frame formatters for the count-up animation', () => {
    const m = panelModel(sum(3_400_000, 10.0, true, 1912), sum(0, null), [], { ...S, currency: 'HKD', usdRate: 7.8 }, 'en');
    expect(m.costValue).toBe(10.0); // USD, conversion happens in fmtCost
    expect(m.tokensValue).toBe(3_400_000);
    expect(m.requestsText).toBe('1,912');
    expect(m.fmtCost(5.0)).toBe('≥ HK$39.00'); // marker survives every frame
    expect(m.fmtTokens(964_200)).toBe('964.2K');
  });

  it('unknown sources keep their raw key and never disappear', () => {
    const m = panelModel(sum(1, 1), sum(0, null), [brow('weirdtool', 1_000, 1.0)], S, 'en');
    expect(m.rows[0].label).toBe('weirdtool');
    expect(m.rows[0].icon).toBeUndefined();
  });

  it('renders the 2b sections alone when the extra reads are absent', () => {
    const m = panelModel(sum(1_000, 8), sum(0, null), [], S, 'en');
    expect(m.spark).toBeNull();
    expect(m.models).toEqual([]);
    expect(m.modelsOverflow).toBe(0);
    expect(m.stats).toBeNull();
  });
});

describe('panelModel Cost sparkline', () => {
  it('sums Cost per hour across the elapsed day, zero-filling idle hours', () => {
    const m = panelModel(sum(1_000, 8), sum(0, null), [], S, 'en', extras({
      series: [
        spt('2026-06-15 00:00', 1),
        spt('2026-06-15 05:00', 4),
        spt('2026-06-15 05:00', 0.5), // a second Source in the same bucket
        spt('2026-06-15 10:00', 2),
      ],
    }));
    expect(m.spark?.points.length).toBe(11); // 00:00 through the current 10:00 bucket
    expect(m.spark?.points[5]).toBe(1); // peak normalised to 1: 4 + 0.5 summed
    expect(m.spark?.points[1]).toBe(0); // an idle hour keeps its slot on the axis
    expect(m.spark?.bucketLabel).toBe('hourly');
    expect(m.spark?.peak).toBe('peak 05:00 · $4.50');
  });

  it('spans yesterday whole, not just the hours elapsed today', () => {
    const m = panelModel(sum(1_000, 8), sum(0, null), [], S, 'en', extras({
      period: 'yesterday',
      series: [spt('2026-06-14 03:00', 1), spt('2026-06-14 23:00', 3)],
    }));
    expect(m.spark?.points.length).toBe(24);
    expect(m.spark?.peak).toBe('peak 23:00 · $3.00');
  });

  it('buckets 30 days by day and labels the peak by date', () => {
    const m = panelModel(sum(1_000, 8), sum(0, null), [], S, 'en', extras({
      period: 'days30',
      series: [spt('2026-05-17', 2), spt('2026-06-15', 9)],
    }));
    expect(m.spark?.points.length).toBe(30); // May 17 through June 15
    expect(m.spark?.points[29]).toBe(1);
    expect(m.spark?.bucketLabel).toBe('daily');
    expect(m.spark?.peak).toBe('peak 06-15 · $9.00');
  });

  it('marks the peak Partial when that bucket holds Unpriced Models', () => {
    const m = panelModel(sum(1_000, 8), sum(0, null), [], S, 'en', extras({
      series: [spt('2026-06-15 01:00', 1), spt('2026-06-15 09:00', 3, 1_000, true)],
    }));
    expect(m.spark?.peak).toBe('peak 09:00 · ≥ $3.00');
  });

  it('hides the sparkline when the period has no Cost to draw', () => {
    const m = panelModel(sum(50, null, false, 0, 50), sum(0, null), [], S, 'en', extras({
      series: [spt('2026-06-15 01:00', 0), spt('2026-06-15 02:00', 0)],
    }));
    expect(m.spark).toBeNull(); // all-Unattributed: a flat zero line would lie
  });

  it('hides the sparkline when one bucket holds all the usage', () => {
    const m = panelModel(sum(1_000, 8), sum(0, null), [], S, 'en', extras({
      series: [spt('2026-06-15 09:00', 8)],
    }));
    expect(m.spark).toBeNull(); // nothing to draw a line between
  });

  it('picks hour buckets for a single day and day buckets for the month', () => {
    expect(seriesBucket('today')).toBe('hour');
    expect(seriesBucket('yesterday')).toBe('hour');
    expect(seriesBucket('days30')).toBe('day');
  });
});

describe('panelModel Models section', () => {
  it('orders by Cost, keeps all-Unpriced last, and colours rows by owning Source', () => {
    const m = panelModel(sum(1_000, 8), sum(0, null), [], S, 'en', extras({
      models: [
        mrow('gpt-5-codex', 'codex', 48_200_000, 47.3),
        mrow('local-llama', 'ollama', 900_000, null, true),
        mrow('claude-sonnet-4-5', 'claude', 470_100_000, 512.4),
        mrow('gemini-2.5-pro', 'gemini', 0, null), // no usage → absent
      ],
    }));
    expect(m.models.map((r) => [r.label, r.tokens, r.cost, r.color])).toEqual([
      ['claude-sonnet-4-5', '470.1M', '$512.40', '#d97757'],
      ['gpt-5-codex', '48.2M', '$47.30', '#6e50f2'],
      ['local-llama', '900K', 'unpriced', undefined], // unknown Source: no colour
    ]);
    expect(m.modelsOverflow).toBe(0);
  });

  it('caps the list at five rows and counts what it hid', () => {
    const models = Array.from({ length: 8 }, (_, i) =>
      mrow(`m-${i}`, 'claude', 1_000, 8 - i));
    const m = panelModel(sum(1_000, 8), sum(0, null), [], S, 'en', extras({ models }));
    expect(m.models.map((r) => r.label)).toEqual(['m-0', 'm-1', 'm-2', 'm-3', 'm-4']);
    expect(m.modelsOverflow).toBe(3);
  });

  it('keeps one row per Source when two Sources ran the same Model', () => {
    const m = panelModel(sum(1_000, 8), sum(0, null), [], S, 'en', extras({
      models: [
        mrow('glm-4.6', 'claude', 2_000, 2),
        mrow('glm-4.6', 'codex', 1_000, 1),
      ],
    }));
    expect(m.models.map((r) => [r.key, r.label, r.cost])).toEqual([
      ['claude/glm-4.6', 'glm-4.6', '$2.00'],
      ['codex/glm-4.6', 'glm-4.6', '$1.00'],
    ]);
  });

  it('marks a Model row Partial for its Unattributed Usage', () => {
    const m = panelModel(sum(1_000, 8), sum(0, null), [], S, 'en', extras({
      models: [{ ...mrow('claude-opus-4-1', 'claude', 1_000, 8), unattributedTokens: 20 }],
    }));
    expect(m.models[0].cost).toBe('≥ $8.00');
  });
});

describe('panelModel stats strip', () => {
  it('reads Cache Hit Rate, the top Project by Cost, and Scan freshness', () => {
    const today = { ...sum(1_000, 8), cacheHitRate: 0.9124 };
    const m = panelModel(today, sum(0, null), [], S, 'en', extras({
      projects: [
        brow('/Users/b/Project/champions-vgc', 179_200_000, 198.42),
        brow('/Users/b/Project/usage', 388_400_000, 431.9),
      ],
      scannedAt: Math.floor(NOW.getTime() / 1000) - 132,
    }));
    expect(m.stats?.cacheHit).toBe('91.2%');
    expect(m.stats?.topProject).toBe('usage · $431.90'); // basename, like the Overview
    expect(m.stats?.scanned).toBe('2 min ago');
  });

  it('says Scan freshness in seconds, minutes, or hours', () => {
    const at = (ago: number) =>
      panelModel(sum(1_000, 8), sum(0, null), [], S, 'en', extras({
        scannedAt: Math.floor(NOW.getTime() / 1000) - ago,
      })).stats?.scanned;
    expect(at(9)).toBe('just now');
    expect(at(3_600)).toBe('1h ago');
    expect(at(7_400)).toBe('2h ago');
  });

  it('admits when no Scan has run yet this launch', () => {
    const m = panelModel(sum(1_000, 8), sum(0, null), [], S, 'en', extras({ scannedAt: 0 }));
    expect(m.stats?.scanned).toBe('—'); // never a wrong "just now"
  });

  it('shows no Project when no Usage Record carries one', () => {
    const m = panelModel(sum(1_000, 8), sum(0, null), [], S, 'en', extras({ projects: [] }));
    expect(m.stats?.topProject).toBeNull();
  });

  it('never names the backend\'s "unknown" group as the top Project', () => {
    // Usage with no Project comes back grouped under "unknown" (queries.rs).
    const m = panelModel(sum(1_000, 8), sum(0, null), [], S, 'en', extras({
      projects: [brow('unknown', 900_000_000, 900), brow('/Users/b/Project/usage', 1_000, 4.4)],
    }));
    expect(m.stats?.topProject).toBe('usage · $4.40');
  });
});

describe('periodWindows', () => {
  const now = new Date(2026, 5, 15, 10, 30, 0); // June 15, 10:30 local
  const mid = (d: number) => Math.floor(new Date(2026, 5, d).getTime() / 1000);

  it('today brackets the local calendar day; comparison clamped to now − 24h', () => {
    const w = periodWindows('today', now);
    expect(w.start).toBe(mid(15));
    expect(w.end).toBe(mid(16));
    // so-far vs so-far: yesterday up to the same time.
    expect(w.prevStart).toBe(mid(14));
    expect(w.prevEnd).toBe(Math.floor(now.getTime() / 1000) - 86_400);
  });

  it('yesterday is the full previous day vs the full day before it', () => {
    const w = periodWindows('yesterday', now);
    expect(w.start).toBe(mid(14));
    expect(w.end).toBe(mid(15));
    expect(w.prevStart).toBe(mid(13));
    expect(w.prevEnd).toBe(mid(14));
  });

  it('30 days trails 30 calendar days including today vs the previous 30', () => {
    const w = periodWindows('days30', now);
    expect(w.start).toBe(Math.floor(new Date(2026, 4, 17).getTime() / 1000)); // May 17
    expect(w.end).toBe(mid(16)); // through today, end-exclusive tomorrow
    expect(w.prevStart).toBe(Math.floor(new Date(2026, 3, 17).getTime() / 1000)); // Apr 17
    expect(w.prevEnd).toBe(Math.floor(new Date(2026, 4, 17).getTime() / 1000));
  });
});
