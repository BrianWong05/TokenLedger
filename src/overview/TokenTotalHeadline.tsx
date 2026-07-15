import { useState } from 'react';
import { formatCompactTokenTotal, formatExactTokenTotal } from '../lib/format';

type TokenDisplayMode = 'compact' | 'exact';

const STORAGE_KEY = 'tokenledger.tokenTotalDisplayMode';

function loadDisplayMode(): TokenDisplayMode {
  return localStorage.getItem(STORAGE_KEY) === 'exact' ? 'exact' : 'compact';
}

export default function TokenTotalHeadline({ total }: { total: number }) {
  const [mode, setMode] = useState<TokenDisplayMode>(loadDisplayMode);
  const exact = formatExactTokenTotal(total);
  const display = mode === 'exact' ? exact : formatCompactTokenTotal(total);
  const action = mode === 'exact' ? 'Show compact token count' : 'Show exact token count';
  const responsiveFontSize = `clamp(20px, ${(155 / Math.max(display.length, 1)).toFixed(3)}cqi, 46px)`;

  const toggleMode = () => {
    const next = mode === 'compact' ? 'exact' : 'compact';
    localStorage.setItem(STORAGE_KEY, next);
    setMode(next);
  };

  return (
    <button
      type="button"
      className="tt-b8-total"
      onClick={toggleMode}
      title={action}
      aria-label={`${exact} total tokens. ${action}`}
      style={{
        display: 'block',
        width: 'fit-content',
        maxWidth: '100%',
        marginInline: 'auto',
        fontSize: responsiveFontSize,
        whiteSpace: 'nowrap',
      }}
    >
      {display}
    </button>
  );
}
