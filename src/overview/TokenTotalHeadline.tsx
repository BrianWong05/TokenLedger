import { useEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import {
  motion,
  useMotionValue,
  useReducedMotion,
  useSpring,
  useTransform,
  type MotionValue,
} from 'motion/react';
import { formatCompactTokenTotal, formatExactTokenTotal } from '../lib/format';

type TokenDisplayMode = 'compact' | 'exact';
type CounterPlace = number | string;

const STORAGE_KEY = 'tokenledger.tokenTotalDisplayMode';
const MODE_ANIMATION_MS = 1_400;
const COUNTER_HEIGHT = '1.0833em';

interface ModeAnimation {
  id: number;
  to: string;
}

type HeadlineStyle = CSSProperties & {
  '--tt-counter-height': string;
  '--tt-headline-font-size': string;
};

function loadDisplayMode(): TokenDisplayMode {
  return localStorage.getItem(STORAGE_KEY) === 'exact' ? 'exact' : 'compact';
}

function prefersReducedMotion() {
  return window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;
}

function usesCompactLayout() {
  return window.matchMedia?.('(max-width: 639px)').matches ?? false;
}

function parseAnimatedCounterValue(displayValue: string) {
  const match = displayValue.replace(/,/g, '').match(/-?\d+(?:\.\d+)?/);
  if (!match) return null;
  const parsed = Number(match[0]);
  return Number.isFinite(parsed) ? parsed : null;
}

function normalizeNearInteger(value: number) {
  const nearest = Math.round(value);
  const tolerance = 1e-9 * Math.max(1, Math.abs(value));
  return Math.abs(value - nearest) < tolerance ? nearest : value;
}

function getValueRoundedToPlace(value: number, place: number) {
  return Math.floor(normalizeNearInteger(value / place));
}

function getCounterPlaces(displayValue: string): CounterPlace[] {
  const characters = Array.from(displayValue);
  const decimalIndex = characters.indexOf('.');
  let digitsBeforeDecimal = characters.filter(
    (character, index) => /\d/.test(character) && (decimalIndex === -1 || index < decimalIndex),
  ).length;
  let decimalPlaces = 0;
  let pastDecimal = false;

  return characters.map((character) => {
    if (!/\d/.test(character)) {
      if (character === '.') pastDecimal = true;
      return character;
    }

    if (!pastDecimal) {
      const place = 10 ** Math.max(digitsBeforeDecimal - 1, 0);
      digitsBeforeDecimal -= 1;
      return place;
    }

    decimalPlaces += 1;
    return 10 ** -decimalPlaces;
  });
}

function staticTokenStyle(token: string): CSSProperties {
  if (token === '.') {
    return {
      width: '0.34ch',
      justifyContent: 'center',
      marginInline: '-0.04ch',
    };
  }

  if (token === ',') {
    return {
      width: '0.88ch',
      justifyContent: 'center',
      marginInline: '-0.30ch',
    };
  }

  return {
    width: 'auto',
    justifyContent: 'center',
    paddingInline: '0.06ch',
  };
}

function RollingDigit({ value, digit }: { value: MotionValue<number>; digit: number }) {
  const y = useTransform(value, (latest) => {
    const placeValue = ((latest % 10) + 10) % 10;
    const offset = (10 + digit - placeValue) % 10;
    const wrappedOffset = offset > 5 ? offset - 10 : offset;
    return `calc(${wrappedOffset} * var(--tt-counter-height))`;
  });

  return (
    <motion.span className="tt-token-counter-rolling-digit" style={{ y }}>
      {digit}
    </motion.span>
  );
}

function StaticCounterToken({ token }: { token: string }) {
  return (
    <span
      data-counter-token="static"
      className="tt-token-counter-token is-static"
      style={staticTokenStyle(token)}
    >
      {token}
    </span>
  );
}

function AnimatedCounterToken({
  place,
  value,
}: {
  place: number;
  value: number;
}) {
  const valueRoundedToPlace = getValueRoundedToPlace(value, place);
  const motionValue = useMotionValue(0);
  const animatedValue = useSpring(motionValue, {
    stiffness: 220,
    damping: 26,
    mass: 0.8,
  });

  useEffect(() => {
    motionValue.set(valueRoundedToPlace);
  }, [motionValue, valueRoundedToPlace]);

  return (
    <span
      data-counter-token="digit"
      data-counter-place={place}
      data-counter-target={valueRoundedToPlace}
      className="tt-token-counter-token is-digit"
    >
      {Array.from({ length: 10 }, (_, digit) => (
        <RollingDigit key={digit} value={animatedValue} digit={digit} />
      ))}
    </span>
  );
}

function TokenTrackerCounter({ displayValue }: { displayValue: string }) {
  const places = useMemo(() => getCounterPlaces(displayValue), [displayValue]);
  const shouldReduceMotion = useReducedMotion() ?? false;
  const numericValue = Math.abs(parseAnimatedCounterValue(displayValue) ?? 0);

  if (shouldReduceMotion) {
    return (
      <span data-counter-root="true" className="tt-token-counter-reduced">
        {displayValue}
      </span>
    );
  }

  return (
    <span data-counter-root="true" className="tt-token-counter-root" aria-hidden="true">
      <span className="tt-token-counter-row">
        {places.map((place, index) =>
          typeof place === 'number' ? (
            <AnimatedCounterToken
              key={`${String(place)}-${index}`}
              place={place}
              value={numericValue}
            />
          ) : (
            <StaticCounterToken key={`${place}-${index}`} token={place} />
          ),
        )}
      </span>
    </span>
  );
}

export default function TokenTotalHeadline({ total }: { total: number }) {
  const [mode, setMode] = useState<TokenDisplayMode>(loadDisplayMode);
  const [modeAnimation, setModeAnimation] = useState<ModeAnimation | null>(null);
  const animationId = useRef(0);
  const exact = formatExactTokenTotal(total);
  const display = mode === 'exact' ? exact : formatCompactTokenTotal(total);
  const action = mode === 'exact' ? 'Show compact token count' : 'Show exact token count';
  const layoutLength = modeAnimation ? modeAnimation.to.length : display.length;
  const responsiveFontSize = `clamp(20px, ${(155 / Math.max(layoutLength, 1)).toFixed(3)}cqi, 46px)`;
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

  const toggleMode = () => {
    if (modeAnimation) return;
    const next = mode === 'compact' ? 'exact' : 'compact';
    const nextDisplay = next === 'exact' ? exact : formatCompactTokenTotal(total);
    localStorage.setItem(STORAGE_KEY, next);
    if (
      display !== nextDisplay &&
      !prefersReducedMotion() &&
      !usesCompactLayout()
    ) {
      animationId.current += 1;
      setModeAnimation({ id: animationId.current, to: nextDisplay });
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
      aria-label={`${exact} total tokens. ${action}`}
      aria-busy={modeAnimation ? true : undefined}
      style={headlineStyle}
    >
      {modeAnimation ? (
        <TokenTrackerCounter key={modeAnimation.id} displayValue={modeAnimation.to} />
      ) : (
        display
      )}
    </button>
  );
}
