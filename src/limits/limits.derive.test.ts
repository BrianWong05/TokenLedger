import { describe, expect, it } from 'vitest';
import type { LimitWindow, SourceLimits } from '../types';
import {
  cards, durationParts, framedPct, freshness, limitsSources, tone, windowLabel, windowView,
} from './limits.derive';

// 2026-08-12T00:00:00Z
const NOW = 1_786_492_800;
const HOUR = 3600;
const DAY = 24 * HOUR;

function win(over: Partial<LimitWindow> = {}): LimitWindow {
  return {
    windowKey: 'seven_day',
    windowMinutes: 10080,
    usedPct: 59,
    resetsAt: NOW + 4 * DAY,
    observedAt: NOW - 60,
    ...over,
  };
}

function held(source: string, windows: LimitWindow[], plan: string | null = 'Plus'): SourceLimits {
  return { source, plan, windows };
}

describe('scarcity tone', () => {
  // The boundaries the spec fixes: >50 comfortable, 21-50 thin, <=20 nearly gone.
  it('follows % left alone, at every boundary', () => {
    expect(tone(51)).toBe('ok');
    expect(tone(50)).toBe('low');
    expect(tone(21)).toBe('low');
    expect(tone(20)).toBe('dry');
    expect(tone(0)).toBe('dry');
  });

  it('never flips with the framing', () => {
    // 21% left is amber; the same window framed as 79% used is still amber, and
    // the figure is what moves.
    const left = windowView(win({ usedPct: 79 }), 'left', NOW);
    const used = windowView(win({ usedPct: 79 }), 'used', NOW);
    expect([left.tone, used.tone]).toEqual(['low', 'low']);
    expect([Math.round(left.pct), Math.round(used.pct)]).toEqual([21, 79]);
  });
});

describe('the time tick', () => {
  it('sits at time-left over duration under Left framing', () => {
    // 4 days left of a 7-day window -> 4/7 across.
    const w = windowView(win({ resetsAt: NOW + 4 * DAY }), 'left', NOW);
    expect(w.tickPct).toBeCloseTo((4 / 7) * 100, 6);
    expect(w.resetsInMin).toBeCloseTo(4 * 1440, 6);
  });

  it('flips to elapsed under Used framing', () => {
    const w = windowView(win({ resetsAt: NOW + 4 * DAY }), 'used', NOW);
    expect(w.tickPct).toBeCloseTo(100 - (4 / 7) * 100, 6);
  });

  it('is absent when the vendor never reported the window length', () => {
    // No duration means no axis to place "now" on — the fill still draws.
    const w = windowView(win({ windowMinutes: null }), 'left', NOW);
    expect(w.tickPct).toBeNull();
    expect(w.pct).toBe(41);
  });

  it('reads the same for a 5-hour session window', () => {
    // 1 hour left of a 5-hour session -> 1/5 across.
    const w = windowView(
      win({ windowKey: 'five_hour', windowMinutes: 300, resetsAt: NOW + HOUR }),
      'left',
      NOW,
    );
    expect(w.tickPct).toBeCloseTo(20, 6);
  });
});

describe('an expired epoch', () => {
  // Nothing could have been used since without a request being logged, so the
  // window reads full rather than stale.
  it('renders full and unused, whatever the reading said', () => {
    const w = windowView(win({ usedPct: 100, resetsAt: NOW - HOUR }), 'left', NOW);
    expect(w.expired).toBe(true);
    expect(w.pctLeft).toBe(100);
    expect(w.pct).toBe(100);
    expect(w.tone).toBe('ok');

    const used = windowView(win({ usedPct: 100, resetsAt: NOW - HOUR }), 'used', NOW);
    expect(used.pct).toBe(0);
  });

  it('takes its tick from the next reset the window period implies', () => {
    // A 5-hour window that rolled 1 hour ago rolls again in 4.
    const w = windowView(
      win({ windowKey: 'five_hour', windowMinutes: 300, resetsAt: NOW - HOUR }),
      'left',
      NOW,
    );
    expect(w.resetsInMin).toBeCloseTo(4 * 60, 6);
    expect(w.tickPct).toBeCloseTo(80, 6);
  });

  it('has no tick when no period is known', () => {
    const w = windowView(win({ windowMinutes: null, resetsAt: NOW - HOUR }), 'left', NOW);
    expect(w.resetsInMin).toBeNull();
    expect(w.tickPct).toBeNull();
  });
});

describe('window labels', () => {
  it('names the known keys and discovers per-model windows from the key tail', () => {
    expect(windowLabel('five_hour')).toEqual({ kind: 'session' });
    expect(windowLabel('w300')).toEqual({ kind: 'session' });
    expect(windowLabel('seven_day')).toEqual({ kind: 'weekly' });
    expect(windowLabel('w10080')).toEqual({ kind: 'weekly' });
    expect(windowLabel('seven_day_opus')).toEqual({ kind: 'model', model: 'Opus' });
    // Never a fixed list: a window nobody has seen before still renders.
    expect(windowLabel('seven_day_zephyr')).toEqual({ kind: 'model', model: 'Zephyr' });
  });

  it('keeps an unrecognised Codex duration as its raw minutes', () => {
    expect(windowLabel('w4321')).toEqual({ kind: 'other', minutes: '4321' });
  });
});

describe('framing', () => {
  it('flips the figure and nothing else', () => {
    expect(framedPct(41, 'left')).toBe(41);
    expect(framedPct(41, 'used')).toBe(59);
  });
});

describe('catalog gating', () => {
  it('yields a card only for a Source declaring a limits capability', () => {
    const keys = limitsSources().map((s) => s.meta.key);
    expect(keys).toEqual(['claude', 'codex']);
    expect(limitsSources().map((s) => s.via)).toEqual(['live', 'logs']);
  });

  it('gives a Source without the capability no card, even holding Readings', () => {
    // Nothing writes Readings for an uncatalogued Source, but if history ever
    // held some, the enum still decides what the page shows.
    const views = cards([held('gemini', [win()])], NOW, 'left');
    expect(views.map((v) => v.source)).toEqual(['claude', 'codex']);
  });
});

describe('card states', () => {
  it('is live when the Source holds windows', () => {
    const [, codex] = cards([held('codex', [win({ windowKey: 'w10080' })])], NOW, 'left');
    expect(codex.state).toBe('live');
    expect(codex.plan).toBe('Plus');
    expect(codex.windows).toHaveLength(1);
  });

  it('reads a logs Source with no Readings as nothing-recorded, not signed-out', () => {
    const [, codex] = cards([], NOW, 'left');
    expect(codex.state).toBe('nothing-recorded');
  });

  it('reads a live Source with no Readings as signed-out', () => {
    const [claude] = cards([], NOW, 'left');
    expect(claude.state).toBe('signed-out');
  });

  it('keeps a failure distinct from an absence, and carries its detail', () => {
    const [claude] = cards([], NOW, 'left', {
      claude: { detail: 'security find-generic-password exited 51' },
    });
    expect(claude.state).toBe('error');
    expect(claude.detail).toBe('security find-generic-password exited 51');

    const [out] = cards([], NOW, 'left', { claude: 'signed-out' });
    expect(out.state).toBe('signed-out');
    expect(out.detail).toBeUndefined();
  });

  it('shows no plan pill and no windows on a card that is not live', () => {
    const [claude] = cards([held('claude', [win()], 'Team 5x')], NOW, 'left', {
      claude: { detail: 'network unreachable' },
    });
    expect(claude.plan).toBeNull();
    expect(claude.windows).toEqual([]);
  });
});

describe('freshness', () => {
  it('reports the age of the fetch for a live Source', () => {
    // Under half a minute reads as "just now" rather than rounding to "0m ago".
    const [justChecked] = cards([held('claude', [win({ observedAt: NOW - 12 })])], NOW, 'left');
    expect(freshness(justChecked, NOW)?.key).toBe('checkedNow');

    const [claude] = cards([held('claude', [win({ observedAt: NOW - 4 * 60 })])], NOW, 'left');
    expect(freshness(claude, NOW)).toEqual({ key: 'checkedAgo', ageMin: 4 });
  });

  it('reports the age of the last request for a logs Source, amber past a day', () => {
    const [, fresh] = cards([held('codex', [win({ observedAt: NOW - 3 * HOUR })])], NOW, 'left');
    expect(freshness(fresh, NOW)?.key).toBe('observedAgo');

    const [, old] = cards([held('codex', [win({ observedAt: NOW - 3 * DAY })])], NOW, 'left');
    expect(freshness(old, NOW)?.key).toBe('observedOld');
  });

  it('has nothing to say without a reading', () => {
    const [claude] = cards([], NOW, 'left');
    expect(freshness(claude, NOW)).toBeNull();
  });
});

describe('duration formatting', () => {
  it('keeps the largest two units', () => {
    expect(durationParts(1 * 1440 + 6 * 60)).toEqual([{ unit: 'd', n: 1 }, { unit: 'h', n: 6 }]);
    expect(durationParts(185)).toEqual([{ unit: 'h', n: 3 }, { unit: 'm', n: 5 }]);
    expect(durationParts(42)).toEqual([{ unit: 'm', n: 42 }]);
    // Minutes are noise beside a day.
    expect(durationParts(4 * 1440 + 7)).toEqual([{ unit: 'd', n: 4 }]);
    expect(durationParts(0)).toEqual([{ unit: 'm', n: 0 }]);
  });
});
