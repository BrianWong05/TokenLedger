// PROTOTYPE-ONLY: the floating variant bar for throwaway UI prototypes. It
// renders nothing outside dev builds, so a stray merge cannot ship it.
import { useEffect, type CSSProperties } from 'react';

export interface PrototypeVariant {
  key: string;
  name: string;
}

const bar: CSSProperties = {
  position: 'fixed',
  bottom: 16,
  left: '50%',
  transform: 'translateX(-50%)',
  display: 'flex',
  alignItems: 'center',
  gap: 10,
  padding: '6px 10px',
  borderRadius: 999,
  background: '#fff',
  color: '#111',
  boxShadow: '0 4px 18px rgba(0, 0, 0, 0.35)',
  font: '600 12px system-ui, sans-serif',
  zIndex: 9999,
  whiteSpace: 'nowrap',
};
const arrow: CSSProperties = {
  border: 0,
  borderRadius: 999,
  width: 26,
  height: 26,
  background: '#111',
  color: '#fff',
  font: 'inherit',
  cursor: 'pointer',
};

export function PrototypeSwitcher({
  variants,
  current,
  onChange,
  status,
}: {
  variants: PrototypeVariant[];
  current: string;
  onChange: (key: string) => void;
  status?: string;
}) {
  const i = Math.max(0, variants.findIndex((v) => v.key === current));
  const go = (d: number) => onChange(variants[(i + d + variants.length) % variants.length].key);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target;
      if (t instanceof HTMLElement && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) {
        return;
      }
      // The in-app browser can send the legacy key names.
      if (e.key === 'ArrowLeft' || e.key === 'Left') go(-1);
      else if (e.key === 'ArrowRight' || e.key === 'Right') go(1);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  if (!import.meta.env.DEV) return null;
  const v = variants[i];
  return (
    <div style={bar}>
      <button type="button" style={arrow} aria-label="Previous variant" onClick={() => go(-1)}>
        ←
      </button>
      <span>
        {v.key} — {v.name}
        {status ? <span style={{ fontWeight: 400, color: '#555' }}> · {status}</span> : null}
      </span>
      <button type="button" style={arrow} aria-label="Next variant" onClick={() => go(1)}>
        →
      </button>
    </div>
  );
}
