// One window's label as text, for the two surfaces that need a string rather
// than the Limits page's styled parts: the panel's meters and the Settings row
// that chooses them. Both call this, so a reader picks a window by the same
// words the meter will print — they diverged once (`Gemini Session` in Settings
// beside `Gemini · Session` on the panel), which is the whole reason this is one
// function and not two.
//
// It takes its translator rather than reaching for one: the panel is
// English-only and reads the dictionary directly, while Settings renders through
// `t()`. The parts themselves come from `windowLabel`, which stays in the
// language-free derivation.
import { limits as limitStrings } from '../lib/strings/limits';
import { fill } from '../lib/format';
import type { windowLabel } from './limits.derive';

/** The Limits dictionary's own keys — the only ones this asks for. */
export type LimitT = (key: keyof (typeof limitStrings)['en']) => string;

export type WindowLabelParts = ReturnType<typeof windowLabel>;

/**
 * Pools ride the label the way the Limits page prefixes them; a per-model window
 * carries "· Weekly" as a `sub` so long model names stay scannable, and a
 * surface with one line of room prints `text` alone.
 */
export function windowText(t: LimitT, label: WindowLabelParts): { text: string; sub?: string } {
  const pools: Record<string, string> = {
    gemini: t('limits.pool.gemini'),
    '3p': t('limits.pool.other'),
  };
  const pool = label.pool ? `${pools[label.pool] ?? label.pool} · ` : '';
  if (label.kind === 'model') {
    return { text: `${pool}${label.model}`, sub: `· ${t('limits.win.weeklySub')}` };
  }
  if (label.kind === 'other') {
    return { text: pool + fill(t('limits.win.other'), { n: label.minutes }) };
  }
  const names = {
    session: t('limits.win.session'),
    weekly: t('limits.win.weekly'),
    weeklyCredits: t('limits.win.weeklyCredits'),
    monthlyCredits: t('limits.win.monthlyCredits'),
  } as const;
  return { text: pool + names[label.kind] };
}
