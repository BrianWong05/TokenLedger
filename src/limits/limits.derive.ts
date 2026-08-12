// Pure derivation for the Limits tab: which Sources get a card, what state each
// card is in, and the four numbers a bar draws from. No React, no fetching and
// no `t()`, so every rule the wayfinder map settled is unit-testable against a
// SourceLimits[] and a clock.
import type { LimitWindow, SourceLimits } from '../types';
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

export interface WindowView {
  /** The vendor's own key, opaque and never parsed for structure. */
  key: string;
  /** 100 − the vendor's used figure, or 100 for an expired epoch. */
  pctLeft: number;
  /** The framed figure the numeral and the fill both show. */
  pct: number;
  tone: Tone;
  /** Minutes until this window's reset; null when none is scheduled. */
  resetsInMin: number | null;
  /** Where "now" sits on the window's axis (0–100), or null for no tick. */
  tickPct: number | null;
  /** The reading's epoch had already rolled when we read it. */
  expired: boolean;
}

export interface CardView {
  source: string;
  meta: SourceMeta;
  via: LimitsVia;
  state: CardState;
  plan: string | null;
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

  const resetsInMin = expired ? null : (w.resetsAt - nowSec) / 60;

  const timeLeftPct =
    resetsInMin !== null && durationMin && durationMin > 0
      ? Math.max(0, Math.min(100, (resetsInMin / durationMin) * 100))
      : null;

  return {
    key: w.windowKey,
    pctLeft,
    pct: framedPct(pctLeft, mode),
    tone: tone(pctLeft),
    resetsInMin,
    tickPct: timeLeftPct === null ? null : mode === 'left' ? timeLeftPct : 100 - timeLeftPct,
    expired,
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
      observedAt,
      windows: state === 'live' ? windows : [],
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
