import { useEffect, useRef, useState, type CSSProperties } from 'react';
import { formatCompactTokenTotal, formatExactTokenTotal } from '../lib/format';

type TokenDisplayMode = 'compact' | 'exact';

const STORAGE_KEY = 'tokenledger.tokenTotalDisplayMode';
const MODE_ANIMATION_MS = 1_400;
const TARGET_STAGE_MS = 160;
const DIGIT_SETTLE_STAGGER_MS = 55;
const MIN_DIGIT_ROLL_MS = 320;

interface ModeAnimation {
  id: number;
  from: string;
  to: string;
}

type OdometerViewportStyle = CSSProperties & {
  '--tt-from-width': string;
  '--tt-reel-window': string;
  '--tt-stage-duration': string;
  '--tt-to-width': string;
};

type OdometerReelStyle = CSSProperties & {
  '--tt-reel-distance': string;
  '--tt-roll-duration': string;
};

type OdometerGridStyle = CSSProperties & {
  '--tt-from-offset': string;
  '--tt-to-offset': string;
};

type HeadlineStyle = CSSProperties & {
  '--tt-headline-font-size': string;
};

function loadDisplayMode(): TokenDisplayMode {
  return localStorage.getItem(STORAGE_KEY) === 'exact' ? 'exact' : 'compact';
}

function isDigit(character: string) {
  return character >= '0' && character <= '9';
}

function centeredCharacters(value: string, length: number) {
  const characters = Array.from(value);
  const leftPadding = Math.floor((length - characters.length) / 2);
  return Array.from({ length }, (_, index) => characters[index - leftPadding] ?? null);
}

function upwardDigitReel(from: string | null, to: string, extraTurns: number) {
  const start = from !== null && isDigit(from) ? Number(from) : 0;
  const target = Number(to);
  const steps = 10 * (1 + extraTurns) + ((target - start + 10) % 10);
  return Array.from({ length: steps + 1 }, (_, index) => String((start + index) % 10));
}

function exitingDigitReel(from: string, extraTurns: number) {
  const start = Number(from);
  const steps = 10 * (1 + extraTurns);
  return Array.from({ length: steps + 1 }, (_, index) => String((start + index) % 10));
}

function prefersReducedMotion() {
  return window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;
}

function RollingTokenTotal({ from, to }: { from: string; to: string }) {
  const slotCount = Math.max(Array.from(from).length, Array.from(to).length);
  const fromSlots = centeredCharacters(from, slotCount);
  const toSlots = centeredCharacters(to, slotCount);
  const rollingSlotIndexes = fromSlots.flatMap((fromCharacter, index) =>
    (fromCharacter !== null && isDigit(fromCharacter)) ||
    (toSlots[index] !== null && isDigit(toSlots[index]!))
      ? [index]
      : [],
  );
  const settleStaggerMs =
    rollingSlotIndexes.length <= 1
      ? 0
      : Math.min(
          DIGIT_SETTLE_STAGGER_MS,
          (MODE_ANIMATION_MS - MIN_DIGIT_ROLL_MS) / (rollingSlotIndexes.length - 1),
        );
  const firstSettleMs =
    MODE_ANIMATION_MS - (rollingSlotIndexes.length - 1) * settleStaggerMs;
  const viewportStyle: OdometerViewportStyle = {
    overflow: 'hidden',
    '--tt-from-width': `${Array.from(from).length}ch`,
    '--tt-reel-window': '1.32em',
    '--tt-stage-duration': `${TARGET_STAGE_MS}ms`,
    '--tt-to-width': `${Array.from(to).length}ch`,
  };
  const gridStyle: OdometerGridStyle = {
    gridTemplateColumns: `repeat(${slotCount}, 1ch)`,
    width: `${slotCount}ch`,
    '--tt-from-offset':
      (slotCount - Array.from(from).length) % 2 === 0 ? '0ch' : '0.5ch',
    '--tt-to-offset': (slotCount - Array.from(to).length) % 2 === 0 ? '0ch' : '0.5ch',
  };

  return (
    <span className="tt-token-odometer-viewport" aria-hidden="true" style={viewportStyle}>
      <span className="tt-token-odometer-grid" style={gridStyle}>
        {fromSlots.map((fromCharacter, index) => {
          const toCharacter = toSlots[index];
          const fromIsDigit = fromCharacter !== null && isDigit(fromCharacter);
          const toIsDigit = toCharacter !== null && isDigit(toCharacter);
          const rollingIndex = rollingSlotIndexes.indexOf(index);
          const duration = firstSettleMs + rollingIndex * settleStaggerMs;
          const extraTurns = rollingIndex % 2;

          let reel: string[] | null = null;
          if (toIsDigit) {
            reel = upwardDigitReel(fromCharacter, toCharacter!, extraTurns);
          } else if (fromIsDigit) {
            reel = exitingDigitReel(fromCharacter!, extraTurns);
          }

          const reelStyle: OdometerReelStyle | undefined = reel
            ? {
                '--tt-reel-distance': `${-(reel.length - 1)}em`,
                '--tt-roll-duration': `${duration}ms`,
              }
            : undefined;
          return (
            <span className="tt-token-odometer-slot" key={index}>
              {reel ? (
                <span
                  className={`tt-token-odometer-reel${toIsDigit ? '' : ' is-exiting'}`}
                  style={reelStyle}
                >
                  {reel.map((digit, reelIndex) => (
                    <span key={reelIndex}>{digit}</span>
                  ))}
                </span>
              ) : null}
              {toCharacter !== null && !toIsDigit ? (
                <span className="tt-token-odometer-symbol is-static">{toCharacter}</span>
              ) : fromCharacter !== null && !fromIsDigit ? (
                <span className="tt-token-odometer-symbol is-old">{fromCharacter}</span>
              ) : null}
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
  const layoutLength = modeAnimation ? modeAnimation.to.length : display.length;
  const responsiveFontSize = `clamp(20px, ${(155 / Math.max(layoutLength, 1)).toFixed(3)}cqi, 46px)`;
  const headlineStyle: HeadlineStyle = {
    display: 'block',
    width: 'fit-content',
    maxWidth: '100%',
    marginInline: 'auto',
    height: '1.32em',
    fontSize: 'var(--tt-headline-font-size)',
    whiteSpace: 'nowrap',
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
      style={headlineStyle}
    >
      {modeAnimation ? (
        <RollingTokenTotal from={modeAnimation.from} to={modeAnimation.to} />
      ) : (
        display
      )}
    </button>
  );
}
