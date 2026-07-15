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
type CounterToken =
  | { kind: 'digit'; glyph: number; target: number }
  | { kind: 'static'; value: string };

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
        <SpringCounter key={modeAnimation.id} displayValue={modeAnimation.to} />
      ) : (
        display
      )}
    </button>
  );
}
