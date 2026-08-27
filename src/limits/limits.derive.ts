// Pure derivation for the Limits tab and the panel's cards: which Sources get a
// card, what state each card is in, the four numbers a bar draws from, which
// of a Source's bars the panel shows collapsed, and in which order the cards
// themselves stack — including reading those last two back out of the reader's
// own storage, since a stored preference is an input to the derivation like the
// clock is. No React, no fetching and no `t()`, so every rule the wayfinder map
// settled is unit-testable against a SourceLimits[], a clock and a stored string.
import type { LimitWindow, SourceLimits } from '../types';
import type { LimitEstimateEvaluation } from '../bindings/LimitEstimateEvaluation';
import type { LimitEstimateState } from '../bindings/LimitEstimateState';
import { SOURCES, type SourceMeta } from '../overview/meta';

// How a Source's Limits are acquired. `logs` flows from ordinary scans; `live`
// is fetched by a Companion (ADR-0019) and so sits behind the opt-in.
export type LimitsVia = 'logs' | 'live';

/** Which way every figure on the page is framed. */
export type Mode = 'left' | 'used';

/**
 * Four resolved card states. `nothing-recorded` is the `logs` analogue of
 * `signed-out`: a Source we can read but have never seen a request from. A card
 * always exists for a catalogued Source — a missing card would read as
 * "unsupported", which is a lie.
 */
export type CardState = 'live' | 'signed-out' | 'error' | 'nothing-recorded';

/** Scarcity tone. Follows % LEFT alone, whatever the framing. */
export type Tone = 'ok' | 'low' | 'dry';

/**
 * What the evidence line draws — the wire's union resolved into the row's own
 * figures, so the renderer never converts or counts anything itself.
 *
 * Every figure is canonical tokens, unformatted: locale is the renderer's
 * business, and this file has no language.
 */
export type EstimateView =
  | {
      state: 'ready';
      /**
       * `perPct` × the displayed percentage; null wherever that percentage is
       * not a fact about now (see `estimateView`).
       */
      selected: number | null;
      perPct: number;
      /**
       * `perPct` × 100 — the info control's local equivalent, never a figure the
       * vendor reported.
       */
      total: number;
      /** Stable-core membership, not every candidate epoch. */
      epochs: number;
    }
  | {
      state: Exclude<LimitEstimateState, 'ready'>;
      /** Completed epochs that qualified; only Gathering's copy reports it. */
      qualifying: number;
    };

export interface WindowView {
  /** The vendor's own key, opaque and never parsed for structure. */
  key: string;
  /** 100 − the vendor's used figure, or 100 for an expired epoch. */
  pctLeft: number;
  /** The framed unrounded figure the fill's geometry uses. */
  pct: number;
  /**
   * The rounded percentage the row DISPLAYS under this framing. The spec fixes
   * the order: the USED figure rounds, and Left is 100 − that — so the two
   * framings always total 100. The numeral, its aria text, and every estimate
   * conversion read this; `pct` keeps the fraction for the fill.
   */
  pctShown: number;
  /** 100 − the rounded used figure — what "used up" is judged by. */
  pctLeftShown: number;
  tone: Tone;
  /** Minutes until this window's reset; null when none is scheduled. */
  resetsInMin: number | null;
  /** Where "now" sits on the window's axis (0–100), or null for no tick. */
  tickPct: number | null;
  /** The reading's epoch had already rolled when we read it. */
  expired: boolean;
  /** The evidence line's content, or null when the wire broke its own rule. */
  estimate: EstimateView | null;
}

export interface CardView {
  source: string;
  meta: SourceMeta;
  via: LimitsVia;
  state: CardState;
  plan: string | null;
  /** Current source-level count; null means the vendor did not report it. */
  usageResetsAvailable: number | null;
  /** Epoch seconds of the newest observation behind these windows, or null. */
  observedAt: number | null;
  windows: WindowView[];
  /** The Companion's own failure line, shown verbatim on an error card. */
  detail?: string;
}

/** A Source's acquisition mode, or null when it has no vendor window to show. */
function limitsVia(meta: SourceMeta): LimitsVia | null {
  const via = meta.capabilities.limits;
  return via === 'logs' || via === 'live' ? via : null;
}

/**
 * Catalogued Sources with a `limits` capability, in catalog order. The enum
 * drives card existence: a Source without it gets no card at all.
 */
export function limitsSources(): { meta: SourceMeta; via: LimitsVia }[] {
  return SOURCES.flatMap((meta) => {
    const via = limitsVia(meta);
    return via ? [{ meta, via }] : [];
  });
}

// >50 comfortable, 21–50 getting thin, ≤20 nearly gone. Read off % left even
// under Used framing, so the colors never flip when the toggle does.
export function tone(pctLeft: number): Tone {
  if (pctLeft <= 20) return 'dry';
  if (pctLeft <= 50) return 'low';
  return 'ok';
}

/** The percentage this window shows under the current framing. */
export function framedPct(pctLeft: number, mode: Mode): number {
  return mode === 'left' ? pctLeft : 100 - pctLeft;
}

/**
 * The estimate's own clock (spec: "Evaluation timing"). Each evaluation names
 * the earliest future second at which time ALONE changes its answer — an epoch
 * resetting, or a candidate ageing out of recency — and the soonest of those is
 * the only moment the page has anything to re-ask. A window whose answer no
 * clock can change reports `null` and contributes nothing; a stamp the clock
 * has already passed is the backend's answer ageing, not a reason to spin.
 */
export function nextDueAt(stored: SourceLimits[], nowSec: number): number | null {
  const times = stored
    .flatMap((s) => s.windows)
    .map((w) => w.estimate.nextEvaluationAt)
    .filter((at): at is number => at !== null && at > nowSec);
  return times.length ? Math.min(...times) : null;
}

/**
 * One stored Reading → the row's drawable state.
 *
 * The tick is time, not a forecast (#107): under Left framing it sits at
 * `resetsIn ÷ duration`, under Used at the elapsed fraction. It needs the
 * window's length, so a window whose duration the vendor never reported draws
 * no tick — there is no axis to place "now" on.
 *
 * An expired epoch renders full/unused rather than stale: nothing could have
 * been used since without a request being logged. It gets no tick — #107 allows
 * one derived from the next reset only "when the window is periodic", and
 * nothing in a Reading says whether it is. A Claude session window is anchored
 * to when its session started, so projecting `resets_at + n·duration` for it
 * would draw exactly the forecast that ticket removed. Drawing none is the
 * honest branch; the card's freshness line is what explains the age.
 */
export function windowView(w: LimitWindow, mode: Mode, nowSec: number): WindowView {
  const durationMin = w.windowMinutes ?? null;
  const expired = w.resetsAt <= nowSec;
  const pctLeft = expired ? 100 : Math.max(0, Math.min(100, 100 - w.usedPct));
  // The spec's displayed percentage: the USED figure rounds, and Left is
  // 100 − that — never a rounding of the raw remainder, which lands one point
  // off at the .5 boundary (59.5% used must read 60% used / 40% left, not 41%).
  const usedShown = expired ? 0 : Math.round(Math.max(0, Math.min(100, w.usedPct)));
  const pctLeftShown = 100 - usedShown;

  const resetsInMin = expired ? null : (w.resetsAt - nowSec) / 60;

  const timeLeftPct =
    resetsInMin !== null && durationMin && durationMin > 0
      ? Math.max(0, Math.min(100, (resetsInMin / durationMin) * 100))
      : null;

  const bar: Omit<WindowView, 'estimate'> = {
    key: w.windowKey,
    pctLeft,
    pct: framedPct(pctLeft, mode),
    pctShown: mode === 'left' ? pctLeftShown : usedShown,
    pctLeftShown,
    tone: tone(pctLeft),
    resetsInMin,
    tickPct: timeLeftPct === null ? null : mode === 'left' ? timeLeftPct : 100 - timeLeftPct,
    expired,
  };

  return { ...bar, estimate: estimateView(w.estimate, bar) };
}

/**
 * One tagged evaluation, plus the bar it sits under → the evidence line's
 * figures.
 *
 * Ready is the only state that carries any, and only when the wire kept its own
 * promise (see `EstimateView`). A `ready` evaluation without a finite positive
 * ratio is a broken contract rather than a state worth describing — the row draws
 * no line at all, which is quieter than an "≈NaN" and louder than pretending the
 * estimate was merely withheld.
 *
 * It takes the whole bar rather than loose numbers because every figure here is
 * about what the row DISPLAYS, and its two questions — which percentage to
 * convert, and whether that percentage is a fact about now — have to be answered
 * off the same rounding. Asking them separately is what let the line read "≈0
 * tokens left" beside a bar that was not yet used up.
 */
export function estimateView(
  evaluation: LimitEstimateEvaluation,
  bar: Omit<WindowView, 'estimate'>,
): EstimateView | null {
  if (evaluation.state !== 'ready') {
    return { state: evaluation.state, qualifying: evaluation.explanation.qualifyingEpochs };
  }
  const perPct = evaluation.tokensPerPct;
  if (!Number.isFinite(perPct) || perPct <= 0) return null;

  // The selected figure is hidden wherever the displayed percentage is not a fact
  // about now: a window the row shows as used up — which would read "≈0 left"
  // under one framing and as a whole-window figure under the other, both of them
  // forbidden — and an expired epoch, whose 100% the bar SYNTHESISES. Converting
  // that second one would invent a full window's worth of tokens out of a Reading
  // that proves nothing about the current window. Judged off `pctShown`'s own
  // rounding, so the line and the numeral can never disagree about which of
  // those a window is.
  //
  // Everything the evidence itself knows survives either way: tokens per 1%, the
  // epoch count, and the info control. A used-up Reading may still carry a Ready
  // historical ratio.
  const spent = bar.expired || bar.pctLeftShown <= 0;

  return {
    state: 'ready',
    selected: spent ? null : perPct * bar.pctShown,
    perPct,
    total: perPct * 100,
    epochs: evaluation.explanation.candidates.filter((c) => c.inCore).length,
  };
}


/**
 * Window key → the label parts to render. The known Claude keys and Codex's
 * duration classes are named; a per-model window is DISCOVERED from the key's
 * own tail, so a `seven_day_zephyr` nobody has seen renders as "Zephyr" rather
 * than disappearing.
 *
 * A key may carry a **pool** ahead of a colon — `gemini:w300`. Antigravity is
 * the first Source where the pool is a genuine second axis rather than a slot:
 * two pools share both durations, so the duration alone does not address a bar.
 * An unrecognised pool renders raw, mirroring the `seven_day_zephyr` rule.
 *
 * `source` is passed because one bar's *quantity* can be a fact about the Source
 * rather than the window: Grok's weekly bar meters a credit pool, not a
 * rate-limit window, and the same geometry at 80% means two different things
 * (#126). It says "credits" so the two are not read as the same thing.
 */
export function windowLabel(
  key: string,
  source?: string,
):
  | { kind: 'session' | 'weekly' | 'weeklyCredits' | 'monthlyCredits'; pool?: string; model?: undefined }
  | { kind: 'model'; model: string; pool?: string }
  | { kind: 'other'; minutes: string; pool?: string } {
  const split = key.indexOf(':');
  if (split > 0) {
    return { ...windowLabel(key.slice(split + 1), source), pool: key.slice(0, split) };
  }
  if (source === 'grok') {
    if (key === 'w10080') return { kind: 'weeklyCredits' };
    if (key === 'w43200') return { kind: 'monthlyCredits' };
  }
  if (key === 'five_hour' || key === 'w300') return { kind: 'session' };
  if (key === 'seven_day' || key === 'w10080') return { kind: 'weekly' };
  const model = key.startsWith('seven_day_') ? key.slice('seven_day_'.length) : null;
  if (model) return { kind: 'model', model: capitalize(model) };
  // A Codex duration outside the canonical set keeps its raw minutes rather than
  // being guessed into a lane it may not belong to.
  const minutes = /^w(\d+)$/.exec(key);
  if (minutes) return { kind: 'other', minutes: minutes[1] };
  return { kind: 'model', model: capitalize(key) };
}

function capitalize(s: string): string {
  const spaced = s.replace(/[_-]+/g, ' ');
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

/**
 * The plan pill's text. Vendors report an internal identifier here —
 * `default_claude_max_5x`, `plus` — and printing one verbatim reads as a bug
 * rather than as honesty, so the wrapper words come off and the rest is
 * title-cased. The stored Reading keeps the vendor's own string untouched; this
 * is a label, and only a label.
 */
export function planLabel(plan: string | null): string | null {
  if (!plan) return null;
  const words = plan
    .replace(/[_-]+/g, ' ')
    .trim()
    .split(/\s+/)
    .filter((w) => w !== 'default' && w !== 'claude');
  if (!words.length) return plan;
  return words.map((w) => w.charAt(0).toUpperCase() + w.slice(1)).join(' ');
}

/**
 * The stored Readings plus any live failure → one card per catalogued Source
 * with a `limits` capability, in catalog order.
 *
 * `failures` carries a `live` Source's own trouble, keyed by Source: 'signed-out'
 * for a credential that is absent or refused, or an error detail line for
 * anything else. The two must never collapse into one another — telling someone
 * they are not signed in when the keystore was merely unreadable sends them to
 * re-authenticate a login they already have.
 */
export function cards(
  stored: SourceLimits[],
  nowSec: number,
  mode: Mode,
  failures: Record<string, 'signed-out' | { detail: string }> = {},
  // Injectable so the `logs` rules stay testable while the shipped catalog
  // happens to hold only `live` Sources.
  sources: { meta: SourceMeta; via: LimitsVia }[] = limitsSources(),
): CardView[] {
  const bySource = new Map(stored.map((s) => [s.source, s]));

  return sources.map(({ meta, via }) => {
    const held = bySource.get(meta.key);
    const windows = (held?.windows ?? []).map((w) => windowView(w, mode, nowSec));
    const observedAt = held?.windows.length
      ? Math.max(...held.windows.map((w) => w.observedAt))
      : null;
    const failure = failures[meta.key];

    const state: CardState =
      failure === 'signed-out'
        ? 'signed-out'
        : failure
          ? 'error'
          : windows.length
            ? 'live'
            : via === 'live'
              ? 'signed-out'
              : 'nothing-recorded';

    return {
      source: meta.key,
      meta,
      via,
      state,
      plan: state === 'live' ? (held?.plan ?? null) : null,
      usageResetsAvailable: state === 'live' ? (held?.usageResetsAvailable ?? null) : null,
      observedAt,
      // An error card keeps its held windows: the failure line says why the
      // figures could not refresh, and the dated rows say what is still known —
      // a bare error card made a refused check look like having nothing at all.
      // The other trouble states stay bare: signed-out figures could belong to
      // a login that is gone (the same rule that keeps the Companion's cache
      // from answering a 401), and nothing-recorded has nothing to show.
      windows: state === 'live' || state === 'error' ? windows : [],
      ...(failure && failure !== 'signed-out' ? { detail: failure.detail } : {}),
    };
  });
}

/**
 * "1d 6h" — the largest two units. Minutes in, so the same helper serves a
 * reset four days out and a live check twelve seconds old.
 */
export function durationParts(minutes: number): { unit: 'd' | 'h' | 'm'; n: number }[] {
  const total = Math.max(0, minutes);
  const d = Math.floor(total / 1440);
  const h = Math.floor((total % 1440) / 60);
  const m = Math.round(total % 60);
  const parts: { unit: 'd' | 'h' | 'm'; n: number }[] = [];
  if (d > 0) parts.push({ unit: 'd', n: d });
  if (h > 0) parts.push({ unit: 'h', n: h });
  // Minutes are noise beside a day; below that they are the whole answer.
  if (d === 0 && m > 0) parts.push({ unit: 'm', n: m });
  if (parts.length === 0) parts.push({ unit: 'm', n: 0 });
  return parts.slice(0, 2);
}

/**
 * Per-card freshness. A `live` card reports the age of its fetch; a `logs` card
 * reports the age of the last request it read, turning amber past a day because
 * that is how old the figures themselves are. Staleness of a *bar* is judged
 * separately, per window, against that window's own reset.
 */
export function freshness(
  card: CardView,
  nowSec: number,
): { key: 'checkedNow' | 'checkedAgo' | 'observedAgo' | 'observedOld'; ageMin: number } | null {
  if (card.observedAt === null) return null;
  const ageMin = Math.max(0, (nowSec - card.observedAt) / 60);
  if (card.via === 'live') {
    return { key: ageMin < 0.5 ? 'checkedNow' : 'checkedAgo', ageMin };
  }
  return { key: ageMin > 1440 ? 'observedOld' : 'observedAgo', ageMin };
}

/**
 * A window beside its parsed label, so the panel's pick and its render read the
 * same parts.
 */
export type ParsedWindow = { w: WindowView; label: ReturnType<typeof windowLabel> };

// The two windows a Source's card collapses to on the panel: its Session and
// its Weekly lane. Restored after the one-line design was reverted (ADR-0007's
// third amendment) — but not restored to picking each with a bare `find`,
// because a rung can hold more than one window and first-match let the alphabet
// choose. Antigravity records a Session per pool (`gemini:w300` and `3p:w300`)
// and the query orders by key, so the meter showed whichever pool sorted first.
// Each meter now shows the window nearest its wall, read off `pctLeft`: neither
// figure a meter can print is usable for this, since both round to whole
// percent and two pools a fraction apart tie on the numeral, and a tie hands
// the pick back to stored order. `pctLeft` is unrounded and
// framing-independent, so the Left/Used toggle cannot move it either. Two ties
// it still cannot break — windows whose `pctLeft` is exactly equal, and a rung
// whose windows have all expired, since `windowView` reads an expired epoch as
// 100 left. That second one is also what keeps a stale window out of a meter
// while a live sibling exists.
export function primaryWindows(parsed: ParsedWindow[]): ParsedWindow[] {
  const nearestWall = (ps: ParsedWindow[]) =>
    ps.length > 0 ? ps.reduce((a, b) => (b.w.pctLeft < a.w.pctLeft ? b : a)) : undefined;
  const session = nearestWall(parsed.filter((p) => p.label.kind === 'session'));
  const weekly = nearestWall(
    parsed.filter((p) => p.label.kind === 'weekly' || p.label.kind === 'weeklyCredits'),
  );
  return [session, weekly].filter((p): p is ParsedWindow => p !== undefined);
}

/**
 * The windows the panel shows for a Source before its card is expanded, in the
 * order it shows them. A Source the reader has picked windows for shows exactly
 * that pick, in the pick's OWN order — the pick is a running order, not a
 * filter, which is what lets Settings put the weekly bar above the session one.
 * An empty pick shows none, which is a choice and not a fault; a Source they
 * never touched keeps the Session + Weekly default, so the pick is opt-in per
 * Source rather than a switch that blanks every card at once.
 *
 * A key the Source no longer reports contributes nothing, so a window a vendor
 * retires leaves no hole. If that empties a pick that DID name something, the
 * default pair comes back rather than the card going blank: a pick naming only
 * windows this Source has stopped reporting is a choice about a lineup that no
 * longer exists, and a blank card is indistinguishable from the deliberate empty
 * pick above — with nothing on it to say why or how to get back.
 */
export function panelWindows(parsed: ParsedWindow[], pick?: string[]): ParsedWindow[] {
  if (!pick) return primaryWindows(parsed);
  const held = pick.flatMap((key) => parsed.filter((p) => p.w.key === key));
  return held.length === 0 && pick.length > 0 ? primaryWindows(parsed) : held;
}

/**
 * The stored panel picks, Source → window keys. This is a trust boundary: the
 * value is whatever web storage holds, so unreadable JSON — or any entry that
 * is not a list of strings — reads as "no pick for that Source" rather than
 * throwing away the panel that asked.
 */
export function parsePanelPicks(raw: string | null): Record<string, string[]> {
  let parsed: unknown;
  try {
    parsed = raw ? JSON.parse(raw) : null;
  } catch {
    return {};
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return {};
  return Object.fromEntries(
    Object.entries(parsed as Record<string, unknown>).flatMap(([source, keys]) =>
      Array.isArray(keys) && keys.every((k) => typeof k === 'string')
        ? [[source, keys as string[]]]
        : [],
    ),
  );
}

/** The stored panel-card pick: every known Source in stack order, plus which are off. */
export interface PanelCardPick {
  order: string[];
  off: string[];
}

export interface PanelCardRow {
  source: string;
  on: boolean;
}

/**
 * The Sources Settings lists, in the order the panel stacks the ones that are
 * on. A reader who has picked an order sees exactly that sequence, including
 * Sources they switched off — those keep their place so turning one back on
 * does not send it to the end. A Source they have not named yet (a newly live
 * one) appends in catalog order among the unnamed, on, so a new card never
 * jumps the queue and never starts hidden.
 *
 * No pick keeps catalog order, every card on. Switching every card off is a
 * real choice (the panel draws none); a pick that names nothing still on
 * offer is not — that lineup no longer exists, so catalog order comes back.
 */
export function panelCardRows(sources: string[], pick: PanelCardPick | null): PanelCardRow[] {
  if (!pick) return sources.map((source) => ({ source, on: true }));
  const known = new Set(sources);
  const ordered = pick.order.filter((k) => known.has(k));
  const rest = sources.filter((k) => !pick.order.includes(k));
  return [...ordered, ...rest].map((source) => ({
    source,
    on: !pick.off.includes(source),
  }));
}

/**
 * The cards the panel draws: `panelCardRows` with the switched-off Sources
 * taken out, which is the only list the panel itself has to walk.
 */
export function panelCardOrder(sources: string[], pick: PanelCardPick | null): string[] {
  return panelCardRows(sources, pick)
    .filter((r) => r.on)
    .map((r) => r.source);
}

function isStringList(v: unknown): v is string[] {
  return Array.isArray(v) && v.every((k) => typeof k === 'string');
}

/**
 * The stored panel-card pick. This is a trust boundary: the value is whatever
 * web storage holds, so unreadable JSON — or anything that is not an object of
 * `{order, off}` string lists — reads as "no pick" rather than throwing away
 * the panel that asked.
 */
export function parsePanelCardPick(raw: string | null): PanelCardPick | null {
  let parsed: unknown;
  try {
    parsed = raw ? JSON.parse(raw) : null;
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return null;
  const rec = parsed as Record<string, unknown>;
  if (!isStringList(rec.order)) return null;
  const off = isStringList(rec.off) ? rec.off : [];
  return rec.order.length === 0 && off.length === 0 ? null : { order: rec.order, off };
}
