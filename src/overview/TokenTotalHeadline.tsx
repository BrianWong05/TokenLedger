import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import {
  motion,
  useMotionValue,
  useReducedMotion,
  useSpring,
  useTransform,
  type MotionValue,
} from 'motion/react';
import { formatCompactTokenTotal, formatExactTokenTotal } from '../lib/format';
import { useOverviewT } from './localize';

type TokenDisplayMode = 'compact' | 'exact';
type CounterToken =
  | { kind: 'digit'; glyph: number; target: number }
  | { kind: 'static'; value: string };

const STORAGE_KEY = 'tokenledger.tokenTotalDisplayMode';
const ENTRANCE_PLAYED_KEY = 'tokenledger.tokenTotalEntrancePlayed';
const MODE_ANIMATION_MS = 1_400;
const COUNTER_HEIGHT = '1.0833em';
const HEADLINE_TRACKING_EM = 0.03;

interface ModeAnimation {
  id: number;
  to: string;
  // A click's reel locks the button until it settles (#12 story 20); a reel the
  // data started must not, or every period switch would deaden the headline.
  fromClick: boolean;
}

interface TokenTotalHeadlineProps {
  total: number;
  summaryReady: boolean;
  // Identifies the window the total describes (range + bounds). A change here is
  // what earns a roll: the same window reporting a new figure is a background
  // scan landing, which #12 story 9 keeps still.
  windowKey: string;
  // Non-null makes the total a floor (ADR-0017): rendered as a ≥ prefix whose
  // hover text is this string — the per-Source unreadable-session reasons.
  incomplete?: string | null;
}

type HeadlineStyle = CSSProperties & {
  '--tt-counter-height': string;
  '--tt-headline-font-size': string;
};

function loadDisplayMode(): TokenDisplayMode {
  return localStorage.getItem(STORAGE_KEY) === 'exact' ? 'exact' : 'compact';
}

function zeroShaped(displayValue: string) {
  return displayValue.replace(/\d/g, '0');
}

function prefersReducedMotion() {
  return window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;
}

function usesCompactLayout() {
  return window.matchMedia?.('(max-width: 639px)').matches ?? false;
}

function getCounterTokens(displayValue: string): CounterToken[] {
  let digitPrefix = 0;

  return Array.from(displayValue, (character): CounterToken => {
    if (character >= '0' && character <= '9') {
      const glyph = Number(character);
      digitPrefix = digitPrefix * 10 + glyph;
      return { kind: 'digit', glyph, target: digitPrefix };
    }

    return { kind: 'static', value: character };
  });
}

function staticTokenClass(token: string) {
  if (token === '.') return 'is-decimal';
  if (token === ',') return 'is-comma';
  return 'is-unit';
}

function WheelGlyph({ position, glyph }: { position: MotionValue<number>; glyph: number }) {
  const y = useTransform(position, (current) => {
    const phase = current - Math.floor(current / 10) * 10;
    let rowOffset = glyph - phase;
    if (rowOffset > 5) rowOffset -= 10;
    if (rowOffset <= -5) rowOffset += 10;
    return `calc(${rowOffset} * var(--tt-counter-height))`;
  });

  return (
    <motion.span className="tt-token-counter-rolling-digit" style={{ y }}>
      {glyph}
    </motion.span>
  );
}

function StaticCounterToken({ token }: { token: string }) {
  return (
    <span
      data-counter-token="static"
      className={`tt-token-counter-token is-static ${staticTokenClass(token)}`}
    >
      {token}
    </span>
  );
}

function AnimatedCounterToken({
  glyph,
  target,
}: {
  glyph: number;
  target: number;
}) {
  const destination = useMotionValue(0);
  const position = useSpring(destination, {
    stiffness: 220,
    damping: 26,
    mass: 0.8,
  });

  useEffect(() => {
    destination.set(target);
  }, [destination, target]);

  return (
    <span
      data-counter-token="digit"
      data-counter-glyph={glyph}
      data-counter-target={target}
      className="tt-token-counter-token is-digit"
    >
      {Array.from({ length: 10 }, (_, wheelGlyph) => (
        <WheelGlyph key={wheelGlyph} position={position} glyph={wheelGlyph} />
      ))}
    </span>
  );
}

function SpringCounter({ displayValue }: { displayValue: string }) {
  const tokens = useMemo(() => getCounterTokens(displayValue), [displayValue]);
  const shouldReduceMotion = useReducedMotion() ?? false;
  const counterWidth = `calc(${tokens.length}ch - ${(tokens.length * HEADLINE_TRACKING_EM).toFixed(2)}em)`;

  if (shouldReduceMotion) {
    return (
      <span data-counter-root="true" className="tt-token-counter-reduced">
        {displayValue}
      </span>
    );
  }

  return (
    <span data-counter-root="true" className="tt-token-counter-root" aria-hidden="true">
      <span className="tt-token-counter-row" style={{ width: counterWidth }}>
        {tokens.map((token, index) =>
          token.kind === 'digit' ? (
            <AnimatedCounterToken
              key={`${token.target}-${index}`}
              glyph={token.glyph}
              target={token.target}
            />
          ) : (
            <StaticCounterToken key={`${token.value}-${index}`} token={token.value} />
          ),
        )}
      </span>
    </span>
  );
}

export default function TokenTotalHeadline({
  total,
  summaryReady,
  windowKey,
  incomplete = null,
}: TokenTotalHeadlineProps) {
  const { t } = useOverviewT();
  const [mode, setMode] = useState<TokenDisplayMode>(loadDisplayMode);
  const [modeAnimation, setModeAnimation] = useState<ModeAnimation | null>(null);
  const [awaitingInitialLoad, setAwaitingInitialLoad] = useState(
    () => sessionStorage.getItem(ENTRANCE_PLAYED_KEY) !== 'true',
  );
  const animationId = useRef(0);
  const startAnimation = useCallback((to: string, fromClick = false) => {
    animationId.current += 1;
    setModeAnimation({ id: animationId.current, to, fromClick });
  }, []);
  const exact = formatExactTokenTotal(total);
  // Three decimals here and on the source cards; the tray panel keeps two.
  const compact = formatCompactTokenTotal(total, 3);
  const display = mode === 'exact' ? exact : compact;
  const revealImmediately = summaryReady && prefersReducedMotion();
  const restingDisplay =
    awaitingInitialLoad && !revealImmediately ? zeroShaped(display) : display;
  const action = mode === 'exact' ? t('overview.showCompact') : t('overview.showExact');
  // The ≥ marker sits outside the counter but inside the width budget.
  const layoutLength =
    (modeAnimation ? modeAnimation.to.length : restingDisplay.length) + (incomplete ? 2 : 0);
  const responsiveFontSize = `clamp(20px, ${(155 / Math.max(layoutLength, 1)).toFixed(3)}cqi, 48px)`;
  const headlineStyle: HeadlineStyle = {
    display: 'block',
    width: 'fit-content',
    maxWidth: '100%',
    marginInline: 'auto',
    height: COUNTER_HEIGHT,
    fontSize: 'var(--tt-headline-font-size)',
    whiteSpace: 'nowrap',
    '--tt-counter-height': COUNTER_HEIGHT,
    '--tt-headline-font-size': responsiveFontSize,
  };

  useEffect(() => {
    if (!modeAnimation) return;
    const timeout = window.setTimeout(() => {
      setModeAnimation((current) => (current?.id === modeAnimation.id ? null : current));
    }, MODE_ANIMATION_MS);
    return () => window.clearTimeout(timeout);
  }, [modeAnimation]);

  useEffect(() => {
    if (!awaitingInitialLoad || !summaryReady || total <= 0) return;

    sessionStorage.setItem(ENTRANCE_PLAYED_KEY, 'true');
    setAwaitingInitialLoad(false);
    if (!prefersReducedMotion()) {
      startAnimation(display);
    }
  }, [awaitingInitialLoad, display, startAnimation, summaryReady, total]);

  // A period switch rolls the wheels in place to the new window's figure — the
  // same motion as a mode change, never the zero-shaped entrance. The roll is
  // owed to the WINDOW moving, not merely to the figure changing: the switch's
  // Summary lands a beat after the click, so what marks it is that the figure
  // settled under a different windowKey than the one now selected. A background
  // scan reports a new figure for the SAME window and stays still (#12 story 9),
  // as does a window whose total is unchanged — there is nothing to roll.
  // ponytail: a switch between two windows with identical totals leaves the
  // recorded key stale, so a later scan on the new window rolls once. Needs the
  // store to say "this Summary is for that window" to close; not worth it.
  const settled = useRef<{ display: string; windowKey: string } | null>(null);
  useEffect(() => {
    if (awaitingInitialLoad || !summaryReady) return;
    const previous = settled.current;
    if (previous?.display === display) return;
    settled.current = { display, windowKey };
    if (!previous) return; // the entrance (or its reduced-motion reveal) showed this value
    if (previous.windowKey === windowKey) return; // same window: a scan, not a switch
    if (modeAnimation?.to === display) return;
    // A zero window reads out immediately: rolling to it would imply usage
    // settling into place where there is none.
    if (total <= 0) return;
    if (!prefersReducedMotion() && !usesCompactLayout()) startAnimation(display);
  }, [awaitingInitialLoad, display, modeAnimation, startAnimation, summaryReady, total, windowKey]);

  const toggleMode = () => {
    // Only a click's own reel swallows further clicks; a data-driven roll must
    // stay interruptible, or a period switch would deaden the button for 1.4s.
    if (modeAnimation?.fromClick) return;
    const next = mode === 'compact' ? 'exact' : 'compact';
    const nextDisplay = next === 'exact' ? exact : compact;
    localStorage.setItem(STORAGE_KEY, next);
    if (
      display !== nextDisplay &&
      !prefersReducedMotion() &&
      !usesCompactLayout()
    ) {
      startAnimation(nextDisplay, true);
    } else {
      setModeAnimation(null);
    }
    setMode(next);
  };

  return (
    <button
      type="button"
      className="tt-b8-total"
      onClick={toggleMode}
      title={action}
      aria-label={`${incomplete ? `${t('overview.atLeast')} ` : ''}${exact} ${t('overview.totalTokensAria')} ${incomplete ? `${incomplete}. ` : ''}${action}`}
      aria-busy={modeAnimation ? true : undefined}
      style={headlineStyle}
    >
      {incomplete && (
        <span className="tt-b8-total-mark" title={incomplete}>
          {'≥ '}
        </span>
      )}
      {modeAnimation ? (
        <SpringCounter key={modeAnimation.id} displayValue={modeAnimation.to} />
      ) : (
        restingDisplay
      )}
    </button>
  );
}
