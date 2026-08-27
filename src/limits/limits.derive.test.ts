import { describe, expect, it } from 'vitest';
import type { LimitWindow, SourceLimits } from '../types';
import type { LimitEstimateEvaluation } from '../bindings/LimitEstimateEvaluation';
import { makeFakeEstimate, makeReadyEstimate } from './limits.fake';
import {
  cards, durationParts, framedPct, freshness, limitsSources, nextDueAt, panelWindows,
  parsePanelPicks, planLabel, primaryWindows, tone, windowLabel, windowView,
} from './limits.derive';
import type { ParsedWindow } from './limits.derive';

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
    estimate: makeFakeEstimate(),
    ...over,
  };
}

function held(
  source: string,
  windows: LimitWindow[],
  plan: string | null = 'Plus',
  usageResetsAvailable: number | null = null,
): SourceLimits {
  return { source, plan, usageResetsAvailable, windows };
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

  it('gets no tick, even knowing the window length', () => {
    // #107 allows a projected tick only "when the window is periodic", and a
    // Reading says nothing about which kind it is: a Claude session window is
    // anchored to when its session started, so `resets_at + n·duration` would be
    // exactly the forecast that ticket removed. Drawing none is the honest branch.
    const w = windowView(
      win({ windowKey: 'five_hour', windowMinutes: 300, resetsAt: NOW - HOUR }),
      'left',
      NOW,
    );
    expect(w.resetsInMin).toBeNull();
    expect(w.tickPct).toBeNull();
  });

  it('has no tick when no period is known either', () => {
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

  it('splits a pool off the front of the key, and never guesses at an unknown one', () => {
    // Antigravity's two pools share both durations, so the pool is part of the
    // key rather than a second card.
    expect(windowLabel('gemini:w300')).toEqual({ kind: 'session', pool: 'gemini' });
    expect(windowLabel('3p:w10080')).toEqual({ kind: 'weekly', pool: '3p' });
    // A pool nobody has named still renders, the way an unseen per-model
    // window does — it is carried through raw rather than dropped.
    expect(windowLabel('zephyr:w300')).toEqual({ kind: 'session', pool: 'zephyr' });
  });

  it('says "credits" where the bar meters a pool rather than a rate-limit window', () => {
    // Same key, same geometry, different quantity: 80% of Grok's weekly credit
    // pool is not 80% of the way to a rate limit (#126).
    expect(windowLabel('w10080', 'grok')).toEqual({ kind: 'weeklyCredits' });
    expect(windowLabel('w43200', 'grok')).toEqual({ kind: 'monthlyCredits' });
    expect(windowLabel('w10080', 'codex')).toEqual({ kind: 'weekly' });
  });
});

describe('the plan pill', () => {
  it('reads the vendor identifier as a label', () => {
    // What this machine's own credential actually carries.
    expect(planLabel('default_claude_max_5x')).toBe('Max 5x');
    expect(planLabel('plus')).toBe('Plus');
    expect(planLabel('Team 5x')).toBe('Team 5x');
    expect(planLabel('self_serve_business_prolite')).toBe('Self Serve Business Prolite');
    expect(planLabel(null)).toBeNull();
    // A tier that is nothing but wrapper words keeps what the vendor said.
    expect(planLabel('default')).toBe('default');
  });
});

describe('framing', () => {
  it('flips the figure and nothing else', () => {
    expect(framedPct(41, 'left')).toBe(41);
    expect(framedPct(41, 'used')).toBe(59);
  });
});

// A fabricated `logs` Source. No shipped Source is `logs`-only now (Grok gained
// a live Companion), so the logs rules are pinned here independently of the
// catalog rather than against a real card.
const LOGS_SOURCE = {
  meta: {
    key: 'faketool', label: 'FakeTool', source: 'FakeTool', color: '#000000',
    icon: 'generic', aliases: [], capabilities: { limits: 'logs' },
    artifacts: [], platforms: ['all'], prerequisite: null,
  },
  via: 'logs' as const,
};

describe('catalog gating', () => {
  it('yields a card only for a Source declaring a limits capability', () => {
    const keys = limitsSources().map((s) => s.meta.key);
    // All five shipped Sources are `live` now. Copilot's Companion is paired
    // with its native CLI usage database.
    expect(keys).toEqual(['claude', 'codex', 'copilot', 'grok', 'antigravity']);
    expect(limitsSources().map((s) => s.via)).toEqual(['live', 'live', 'live', 'live', 'live']);
  });

  it('gives a Source without the capability no card, even holding Readings', () => {
    // Nothing writes Readings for an uncatalogued Source, but if history ever
    // held some, the enum still decides what the page shows.
    const views = cards([held('gemini', [win()])], NOW, 'left');
    expect(views.map((v) => v.source)).toEqual([
      'claude', 'codex', 'copilot', 'grok', 'antigravity',
    ]);
  });
});

describe('card states', () => {
  it('is live when the Source holds windows', () => {
    const [, codex] = cards([held('codex', [win({ windowKey: 'w10080' })], 'Plus', 1)], NOW, 'left');
    expect(codex.state).toBe('live');
    expect(codex.plan).toBe('Plus');
    expect(codex.usageResetsAvailable).toBe(1);
    expect(codex.windows).toHaveLength(1);
  });

  it('reads a logs Source with no Readings as nothing-recorded, not signed-out', () => {
    const [fake] = cards([], NOW, 'left', {}, [LOGS_SOURCE]);
    expect(fake.state).toBe('nothing-recorded');
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

  it('keeps the held windows on an error card but hides its plan pill', () => {
    const [claude] = cards([held('claude', [win()], 'Team 5x')], NOW, 'left', {
      claude: { detail: 'network unreachable' },
    });
    expect(claude.state).toBe('error');
    expect(claude.plan).toBeNull();
    expect(claude.usageResetsAvailable).toBeNull();
    // The failure says why nothing refreshed; the dated rows say what is still
    // known. A bare error card made a refused check look like having nothing.
    expect(claude.windows).toHaveLength(1);
  });

  it('shows no windows on a signed-out card — those figures may belong to a login that is gone', () => {
    const [claude] = cards([held('claude', [win()], 'Team 5x')], NOW, 'left', {
      claude: 'signed-out',
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
    const logsCard = (observedAt: number) =>
      cards([held('faketool', [win({ observedAt })])], NOW, 'left', {}, [LOGS_SOURCE])[0];
    expect(freshness(logsCard(NOW - 3 * HOUR), NOW)?.key).toBe('observedAgo');
    expect(freshness(logsCard(NOW - 3 * DAY), NOW)?.key).toBe('observedOld');
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

describe('the estimate clock', () => {
  const at = (nextEvaluationAt: number | null) =>
    win({ estimate: makeFakeEstimate({ nextEvaluationAt }) });

  it('picks the earliest future stamp across every window', () => {
    const stored = [
      held('claude', [at(null), at(NOW + 50)]),
      held('codex', [at(NOW + 30)]),
    ];
    expect(nextDueAt(stored, NOW)).toBe(NOW + 30);
  });

  it('ignores stamps the clock has already passed, and reports null on none', () => {
    // A past stamp is the backend's answer ageing, not a timer to spin on.
    expect(nextDueAt([held('claude', [at(NOW - 10)])], NOW)).toBeNull();
    expect(nextDueAt([held('claude', [at(null)])], NOW)).toBeNull();
    expect(nextDueAt([], NOW)).toBeNull();
  });
});

describe('the estimate view', () => {
  // 59% used → 41% left. Both framings convert the figure the row is SHOWING,
  // so whoever multiplies 41 × 350000 gets exactly what the line reports.
  const ready = (perPct: number, core = 4, spare = 0) =>
    win({ usedPct: 59, estimate: makeReadyEstimate(perPct, core, spare) });

  it('converts the displayed percentage, and counts only the stable core', () => {
    const v = windowView(ready(350_000, 4, 3), 'left', NOW);

    expect(v.estimate).toEqual({
      state: 'ready',
      selected: 41 * 350_000,
      perPct: 350_000,
      total: 35_000_000,
      epochs: 4,
    });
  });

  it('rounds the way the numeral does, so the two figures multiply out', () => {
    // 40.6% left, shown as "41%".
    const v = windowView(win({ usedPct: 59.4, estimate: makeReadyEstimate(350_000) }), 'left', NOW);
    expect(v.estimate).toMatchObject({ selected: 41 * 350_000 });
  });

  it('derives Left from the rounded Used figure, so the framings total 100', () => {
    // 59.5% used rounds to 60% used, so Left must read 40% — never
    // round(40.5) = 41, which would put 101 points on screen across the two
    // framings and convert a percentage the numeral does not show.
    const at = (mode: 'left' | 'used') =>
      windowView(win({ usedPct: 59.5, estimate: makeReadyEstimate(350_000) }), mode, NOW);

    expect([at('left').pctShown, at('used').pctShown]).toEqual([40, 60]);
    expect(at('left').estimate).toMatchObject({ selected: 40 * 350_000 });
    expect(at('used').estimate).toMatchObject({ selected: 60 * 350_000 });
  });

  it('changes only the selected figure when the framing flips', () => {
    const left = windowView(ready(350_000), 'left', NOW).estimate;
    const used = windowView(ready(350_000), 'used', NOW).estimate;

    expect(left).toMatchObject({ selected: 41 * 350_000 });
    expect(used).toMatchObject({ selected: 59 * 350_000 });
    // Everything the estimate itself knows is framing-blind.
    expect({ ...left, selected: null }).toEqual({ ...used, selected: null });
  });

  // The rate, the evidence count and (with them) the info control survive every
  // case that hides the selected figure — they are facts about the evidence, not
  // about the current Reading.
  const withoutSelected = {
    state: 'ready',
    selected: null,
    perPct: 350_000,
    total: 35_000_000,
    epochs: 4,
  };

  it('hides the selected figure on a used-up window under either framing', () => {
    const spent = win({ usedPct: 100, estimate: makeReadyEstimate(350_000) });

    for (const mode of ['left', 'used'] as const) {
      expect(windowView(spent, mode, NOW).estimate).toEqual(withoutSelected);
    }
  });

  it('hides it on a window the row only DISPLAYS as used up', () => {
    // 99.6% used is not `pctLeft <= 0`, but the numeral rounds to "0%" left and
    // "100%" used. Guarding on the raw figure let the line read "≈0 tokens left"
    // beside that 0%, and a whole-window "≈35M tokens used" beside that 100% —
    // both of them the renderings the spec names as forbidden.
    const nearly = win({ usedPct: 99.6, estimate: makeReadyEstimate(350_000) });

    for (const mode of ['left', 'used'] as const) {
      expect(windowView(nearly, mode, NOW).estimate).toEqual(withoutSelected);
    }
    // One displayed point of headroom is still a figure worth showing.
    expect(windowView(win({ usedPct: 99, estimate: makeReadyEstimate(350_000) }), 'left', NOW)
      .estimate).toMatchObject({ selected: 350_000 });
  });

  it('hides it on an expired epoch, whose percentage the bar invents', () => {
    // An expired window renders full and unused (#107) off a `pctLeft` nothing
    // observed. The backend should already be Blocked here — "an expired
    // displayed Reading does not prove a new current Partition" — but its
    // `evaluatedAt` and this page's clock are two different clocks, so the row
    // must not multiply a ratio by a percentage it made up on its own.
    const expired = win({
      usedPct: 0,
      resetsAt: NOW - HOUR,
      estimate: makeReadyEstimate(350_000),
    });

    expect(windowView(expired, 'left', NOW)).toMatchObject({ expired: true, pctLeft: 100 });
    for (const mode of ['left', 'used'] as const) {
      expect(windowView(expired, mode, NOW).estimate).toEqual(withoutSelected);
    }
  });

  it('carries a withheld state through with the count its copy needs', () => {
    for (const state of ['gathering', 'unstable', 'stale', 'blocked'] as const) {
      const base = makeFakeEstimate();
      const estimate = {
        ...base,
        state,
        explanation: { ...base.explanation, qualifyingEpochs: 2 },
      };
      expect(windowView(win({ estimate }), 'left', NOW).estimate).toEqual({ state, qualifying: 2 });
    }
  });

  it('draws nothing at all when a ready evaluation brings no usable ratio', () => {
    // A broken wire promise (see `EstimateView`) is not a withheld estimate, and
    // must never reach a person as one.
    for (const tokensPerPct of [undefined, 0, -1, NaN, Infinity]) {
      // The union has no room for these, so the broken wire is forced past the
      // compiler on purpose: the runtime guard is what this test pins.
      const estimate = {
        ...makeReadyEstimate(350_000),
        tokensPerPct,
      } as unknown as LimitEstimateEvaluation;
      expect(windowView(win({ estimate }), 'left', NOW).estimate).toBeNull();
    }
  });
});


describe('the panel\'s collapsed windows', () => {
  const parse = (ws: LimitWindow[], source = 'claude'): ParsedWindow[] =>
    ws.map((w) => ({ w: windowView(w, 'left', NOW), label: windowLabel(w.windowKey, source) }));

  const claude = () =>
    parse([
      win({ windowKey: 'five_hour', windowMinutes: 300, usedPct: 5, resetsAt: NOW + 2 * HOUR }),
      win({ windowKey: 'seven_day', usedPct: 66 }),
      win({ windowKey: 'seven_day_fable', usedPct: 46 }),
    ]);

  const keys = (ps: ParsedWindow[]) => ps.map((p) => p.w.key);

  it('keeps the Session + Weekly default for a Source with no pick', () => {
    expect(keys(panelWindows(claude()))).toEqual(['five_hour', 'seven_day']);
    // The default is the shared rule, not a copy of it: whatever primaryWindows
    // answers is what an untouched Source shows.
    expect(keys(panelWindows(claude()))).toEqual(keys(primaryWindows(claude())));
  });

  it("shows exactly the picked windows, in the pick's own order", () => {
    // The pick is a running order, not a filter: reversing it reverses the
    // meters, which is the only way Settings can put Weekly above Session.
    expect(keys(panelWindows(claude(), ['seven_day_fable', 'seven_day']))).toEqual([
      'seven_day_fable',
      'seven_day',
    ]);
    expect(keys(panelWindows(claude(), ['seven_day', 'seven_day_fable']))).toEqual([
      'seven_day',
      'seven_day_fable',
    ]);
    // A per-model window alone is a legitimate pick, and it must not drag the
    // default pair back in beside it.
    expect(keys(panelWindows(claude(), ['seven_day_fable']))).toEqual(['seven_day_fable']);
  });

  it('shows none for an empty pick, and ignores a key the Source no longer has', () => {
    expect(panelWindows(claude(), [])).toEqual([]);
    expect(keys(panelWindows(claude(), ['seven_day_zephyr', 'seven_day']))).toEqual(['seven_day']);
  });

  it('falls back to the default pair when the pick names nothing still reported', () => {
    // A pick whose every window the vendor has retired is a choice about a
    // lineup that no longer exists. The default pair is recoverable; a blank
    // card is indistinguishable from the deliberate empty pick above.
    expect(keys(panelWindows(claude(), ['seven_day_zephyr']))).toEqual(['five_hour', 'seven_day']);
    // Only when NOTHING matches: one surviving key is still a pick, and the
    // default must not come back beside it.
    expect(keys(panelWindows(claude(), ['seven_day_zephyr', 'seven_day_fable']))).toEqual([
      'seven_day_fable',
    ]);
    // And an emptied pick stays empty — it names no retired window either.
    expect(panelWindows(claude(), [])).toEqual([]);
  });

  it('reads stored picks, and treats anything else as no pick at all', () => {
    expect(parsePanelPicks('{"claude":["seven_day","seven_day_fable"],"codex":[]}')).toEqual({
      claude: ['seven_day', 'seven_day_fable'],
      codex: [],
    });
    // Web storage holds whatever anyone put there. None of these may throw, and
    // none may reach the panel as a pick — an unreadable entry has to fall back
    // to the default pair, never to a blank card.
    for (const raw of [null, '', 'not json', '[]', '"claude"', '7', '{"claude":"seven_day"}',
      '{"claude":[1,2]}', '{"claude":null}']) {
      expect(parsePanelPicks(raw), raw ?? 'null').toEqual({});
    }
    // One bad entry loses only itself.
    expect(parsePanelPicks('{"claude":["seven_day"],"codex":3}')).toEqual({
      claude: ['seven_day'],
    });
  });
});
