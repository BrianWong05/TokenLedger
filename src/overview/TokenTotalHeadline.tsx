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
import { NO_FLOOR, useOverviewT, type TokenFloor } from './localize';

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
  // Whether `total` descends from a settled scan. False through the launch's
  // provisional paint, which shows real figures the reconcile may still
  // correct: the entrance is spent once (#14) and a same-window correction
  // holds still (#12 story 9), so rolling that figure would leave the settled
  // one to arrive with no motion at all. The zero-shaped placeholder therefore
  // stands until this turns true — which is why the store reads it off the
  // post-scan SERIES and not the window() reload behind it.
  authoritative: boolean;
  // Identifies the window the total describes (range + bounds). A change here is
  // what earns a roll: the same window reporting a new figure is a background
  // scan landing, which #12 story 9 keeps still.
  windowKey: string;
  // Whether the Overview is the tab on screen. The Overview stays mounted while
  // another tab shows, so this is the only way the headline can tell it has just
  // come back into view — which rolls it (#94). Defaults true for the surfaces
  // that mount the headline with nothing hiding it.
  visible?: boolean;
  // The window's ≥ floor (ADR-0017), as localize.tokenFloor built it: marked
  // renders the ≥ prefix, reason is its hover text.
  floor?: TokenFloor;
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
  authoritative,
  windowKey,
  visible = true,
  floor = NO_FLOOR,
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
  const revealImmediately = authoritative && prefersReducedMotion();
  const restingDisplay =
    awaitingInitialLoad && !revealImmediately ? zeroShaped(display) : display;
  const action = mode === 'exact' ? t('overview.showCompact') : t('overview.showExact');
  // The ≥ marker sits outside the counter but inside the width budget.
  const layoutLength =
    (modeAnimation ? modeAnimation.to.length : restingDisplay.length) + (floor.marked ? 2 : 0);
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
    if (!awaitingInitialLoad || !authoritative || total <= 0) return;

    sessionStorage.setItem(ENTRANCE_PLAYED_KEY, 'true');
    setAwaitingInitialLoad(false);
    if (!prefersReducedMotion()) {
      startAnimation(display);
    }
  }, [awaitingInitialLoad, display, startAnimation, authoritative, total]);

  // What every data-driven roll requires, wherever it is triggered from: an
  // authoritative figure, usage to show (a zero window reads out immediately —
  // rolling to it would imply usage settling into place where there is none),
  // and an environment that wants motion at all. A click's roll answers to
  // toggleMode's own conditions instead, because it may cross modes when the
  // figure itself has not moved.
  const rollAllowed = () =>
    authoritative && total > 0 && !prefersReducedMotion() && !usesCompactLayout();

  // A period switch rolls the wheels in place as soon as its windowKey changes,
  // using the series-derived figure while the window Summary is still loading.
  // It is the WINDOW moving, not merely the figure changing, that earns a roll;
  // a background scan reports a new figure for the SAME window and stays still
  // (#12 story 9).
  const settled = useRef<{ display: string; windowKey: string } | null>(null);
  useEffect(() => {
    if (awaitingInitialLoad || !authoritative) return;
    const previous = settled.current;
    const windowChanged = previous?.windowKey !== windowKey;
    if (previous?.display === display && !windowChanged) return;
    settled.current = { display, windowKey };
    if (!previous) return; // the entrance (or its reduced-motion reveal) showed this value
    if (!windowChanged) return; // same window: a scan, not a switch
    if (modeAnimation?.to === display) return;
    if (rollAllowed()) startAnimation(display);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [awaitingInitialLoad, display, modeAnimation, startAnimation, authoritative, total, windowKey]);

  // Coming back to the Overview from another tab rolls the figure that is
  // already on screen. The Overview stays mounted while Pricing or Settings
  // shows, so nothing about the total changed — the return itself is the
  // occasion (#94). A still-owed entrance wins: it has its own zero-shaped
  // motion and has not played yet.
  const wasVisible = useRef(visible);
  useEffect(() => {
    const returning = visible && !wasVisible.current;
    wasVisible.current = visible;
    if (!returning || awaitingInitialLoad) return;
    if (rollAllowed()) startAnimation(display);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visible, awaitingInitialLoad, display, startAnimation, authoritative, total]);

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
      aria-label={`${floor.marked ? `${t('overview.atLeast')} ` : ''}${exact} ${t('overview.totalTokensAria')} ${floor.marked ? `${floor.reason}. ` : ''}${action}`}
      aria-busy={modeAnimation ? true : undefined}
      style={headlineStyle}
    >
      {/* The one ≥ markedTokenFigure does not render: the counter animates
          in its own element, so the marker needs a span of its own to carry
          the hover reason. */}
      {floor.marked && (
        <span className="tt-b8-total-mark" title={floor.reason}>
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
