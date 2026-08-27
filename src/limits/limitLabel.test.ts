import { describe, expect, it } from 'vitest';
import { windowText, type LimitT } from './limitLabel';
import { windowLabel } from './limits.derive';
import { limits as limitStrings } from '../lib/strings/limits';

const en: LimitT = (key) => limitStrings.en[key];
const text = (key: string, source?: string) => windowText(en, windowLabel(key, source));

describe('a window label as text', () => {
  it('names the canonical lanes, and a per-model window with its sub', () => {
    expect(text('five_hour')).toEqual({ text: 'Session' });
    expect(text('seven_day')).toEqual({ text: 'Weekly' });
    expect(text('w10080', 'grok')).toEqual({ text: 'Weekly credits' });
    expect(text('w43200', 'grok')).toEqual({ text: 'Monthly credits' });
    expect(text('seven_day_fable')).toEqual({ text: 'Fable', sub: '· Weekly' });
    expect(text('w120', 'codex')).toEqual({ text: '120 window' });
  });

  // The separator both surfaces print. It is pinned because they once assembled
  // it separately and disagreed: Settings read "Gemini Session" while the panel
  // read "Gemini · Session", so a reader picked a window by one name and was
  // shown another.
  it('prefixes a pool with the same separator everywhere', () => {
    expect(text('gemini:w300')).toEqual({ text: 'Gemini · Session' });
    expect(text('3p:w10080')).toEqual({ text: 'Other models · Weekly' });
    // A pool nobody has named renders raw rather than disappearing.
    expect(text('zeta:w300')).toEqual({ text: 'zeta · Session' });
  });
});
