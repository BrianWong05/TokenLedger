import { useEffect, useRef, useState, type CSSProperties } from 'react';
import { formatCompactTokenTotal, formatExactTokenTotal } from '../lib/format';

type TokenDisplayMode = 'compact' | 'exact';

const STORAGE_KEY = 'tokenledger.tokenTotalDisplayMode';
const MODE_ANIMATION_MS = 800;
const DIGIT_SETTLE_STAGGER_MS = 40;
const MIN_DIGIT_ROLL_MS = 80;

interface ModeAnimation {
  id: number;
  from: string;
  to: string;
}

type RollingDigitStyle = CSSProperties & {
  '--tt-roll-duration': string;
};

function loadDisplayMode(): TokenDisplayMode {
  return localStorage.getItem(STORAGE_KEY) === 'exact' ? 'exact' : 'compact';
}

function isDigit(character: string) {
  return character >= '0' && character <= '9';
}

function prefersReducedMotion() {
  return window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;
}

function RollingTokenTotal({ from, to }: { from: string; to: string }) {
  const previousDigits = Array.from(from).filter(isDigit);
  const previousDigitCount = previousDigits.length;
  const targetDigitCount = Array.from(to).filter(isDigit).length;
  const rollingDigitCount = Math.max(previousDigitCount, targetDigitCount);
  const settleStaggerMs =
    rollingDigitCount <= 1
      ? 0
      : Math.min(
          DIGIT_SETTLE_STAGGER_MS,
          (MODE_ANIMATION_MS - MIN_DIGIT_ROLL_MS) / (rollingDigitCount - 1),
        );
  const firstSettleMs = MODE_ANIMATION_MS - (rollingDigitCount - 1) * settleStaggerMs;
  let digitIndex = 0;
  let previousDigitIndex = 0;

  return (
    <span className="tt-token-roll" aria-hidden="true">
      <span className="tt-token-roll-old-symbols">
        {Array.from(from).map((character, index) => (
          <span className={isDigit(character) ? 'digit' : 'symbol'} key={`${character}-${index}`}>
            {character}
          </span>
        ))}
      </span>
      <span className="tt-token-roll-removed-digits">
        {Array.from(from).map((character, characterIndex) => {
          if (!isDigit(character)) {
            return (
              <span className="placeholder symbol" key={`symbol-${characterIndex}`}>
                {character}
              </span>
            );
          }

          const currentDigitIndex = previousDigitIndex++;
          if (currentDigitIndex < targetDigitCount) {
            return (
              <span className="placeholder digit" key={`digit-${characterIndex}`}>
                {character}
              </span>
            );
          }

          const duration = firstSettleMs + currentDigitIndex * settleStaggerMs;
          const style: RollingDigitStyle = { '--tt-roll-duration': `${duration}ms` };
          return (
            <span className="tt-token-roll-digit" key={`removed-${characterIndex}`}>
              <span className="tt-token-roll-track tt-token-roll-track-away" style={style}>
                <span>{character}</span>
                <span>{character}</span>
              </span>
            </span>
          );
        })}
      </span>
      <span className="tt-token-roll-new-value">
        {Array.from(to).map((character, characterIndex) => {
          if (!isDigit(character)) {
            return (
              <span className="tt-token-roll-symbol" key={`${character}-${characterIndex}`}>
                {character}
              </span>
            );
          }

          const currentDigitIndex = digitIndex++;
          const duration = firstSettleMs + currentDigitIndex * settleStaggerMs;
          const style: RollingDigitStyle = { '--tt-roll-duration': `${duration}ms` };
          return (
            <span className="tt-token-roll-digit" key={`digit-${characterIndex}`}>
              <span className="tt-token-roll-track" style={style}>
                <span>{previousDigits[currentDigitIndex] ?? '0'}</span>
                <span>{character}</span>
              </span>
            </span>
          );
        })}
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
  const responsiveFontSize = `clamp(20px, ${(155 / Math.max(display.length, 1)).toFixed(3)}cqi, 46px)`;

  useEffect(() => {
    if (!modeAnimation) return;
    const timeout = window.setTimeout(() => {
      setModeAnimation((current) => (current?.id === modeAnimation.id ? null : current));
    }, MODE_ANIMATION_MS);
    return () => window.clearTimeout(timeout);
  }, [modeAnimation]);

  const toggleMode = () => {
    const next = mode === 'compact' ? 'exact' : 'compact';
    const nextDisplay = next === 'exact' ? exact : formatCompactTokenTotal(total);
    localStorage.setItem(STORAGE_KEY, next);
    if (display !== nextDisplay && !prefersReducedMotion()) {
      animationId.current += 1;
      setModeAnimation({ id: animationId.current, from: display, to: nextDisplay });
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
      style={{
        display: 'block',
        width: 'fit-content',
        maxWidth: '100%',
        marginInline: 'auto',
        fontSize: responsiveFontSize,
        whiteSpace: 'nowrap',
      }}
    >
      {modeAnimation ? (
        <RollingTokenTotal from={modeAnimation.from} to={modeAnimation.to} />
      ) : (
        display
      )}
    </button>
  );
}
