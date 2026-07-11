import { useCallback, useEffect, useRef, useState } from 'react';
import './overview.css';
import { fetchLimits } from '../api';
import type { LimitsSnapshot, ToolLimits } from '../types';
import { fmtAgo, fmtRemain } from '../lib/format';

const MONO = "ui-monospace,'SF Mono',Menlo,monospace";
const ACCENT = '#2a6df4';
const REFRESH_MS = 5 * 60 * 1000;

type IconKey = 'claude' | 'codex' | 'gemini' | 'grok' | 'antigravity';

const TOOL_META: Record<IconKey, { name: string; color: string }> = {
  claude: { name: 'Claude', color: '#d97757' },
  codex: { name: 'Codex', color: '#59c2a6' },
  gemini: { name: 'Gemini', color: '#e2a63b' },
  grok: { name: 'Grok Build', color: '#b8c0cc' },
  antigravity: { name: 'Antigravity', color: '#37c98b' },
};

const soft = (hex: string, a: number) => {
  const h = hex.replace('#', '');
  return `rgba(${parseInt(h.slice(0, 2), 16)},${parseInt(h.slice(2, 4), 16)},${parseInt(h.slice(4, 6), 16)},${a})`;
};

function ToolIcon({ icon }: { icon: IconKey }) {
  switch (icon) {
    case 'claude':
      return <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round"><path d="M12 3v18M3 12h18M5.6 5.6l12.8 12.8M18.4 5.6L5.6 18.4" /></svg>;
    case 'codex':
      return <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinejoin="round"><path d="M12 3l8 4.5v9L12 21l-8-4.5v-9z" /></svg>;
    case 'gemini':
      return <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><path d="M12 3l5.6 9L12 21l-5.6-9z" /></svg>;
    case 'grok':
      return <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round"><circle cx="12" cy="12" r="8" /><path d="M6.7 17.3L17.3 6.7" /></svg>;
    case 'antigravity':
      return <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><path d="M12 4l8.5 15.5h-17z" /></svg>;
  }
}

// Page-local styles; the shell (.tt-*) and tt-rise keyframes come from overview.css.
const SCOPED_CSS = `
.lim-iconbtn{width:34px;height:34px;border-radius:9px;background:rgba(255,255,255,.05);border:1px solid rgba(255,255,255,.08);color:#a9b2c4;display:inline-flex;align-items:center;justify-content:center;cursor:pointer;font-size:16px;line-height:1;transition:border-color .15s,color .15s;}
.lim-iconbtn:hover{border-color:rgba(255,255,255,.18);color:#e8ecf4;}
@keyframes tl-spin{to{transform:rotate(360deg);}}
`;

const NAV = ['Overview', 'Activity', 'Models', 'Limits', 'Settings'];

export default function Limits({ nav, onNav }: { nav: string; onNav: (n: string) => void }) {
  const [snap, setSnap] = useState<LimitsSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [mode, setMode] = useState<'used' | 'left'>('used');
  const [filter, setFilter] = useState<'all' | 'connected'>('all');
  const inflight = useRef(false);

  const load = useCallback(async (force: boolean) => {
    if (inflight.current) return;
    inflight.current = true;
    setRefreshing(true);
    try {
      setSnap(await fetchLimits(force));
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      inflight.current = false;
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    void load(false);
    const id = setInterval(() => void load(false), REFRESH_MS);
    return () => clearInterval(id);
  }, [load]);

  const tools = snap?.tools ?? [];
  const isConnected = (t: ToolLimits) => t.configured && !t.error;
  const connected = tools.filter(isConnected).length;
  const countLabel = `${tools.length} tools · ${connected} connected`;
  const visible = filter === 'connected' ? tools.filter(isConnected) : tools;

  const barColor = (t: ToolLimits, used: number) => {
    if (mode === 'used') {
      if (used >= 90) return '#f0616d';
      if (used >= 75) return '#e2a63b';
    }
    return TOOL_META[t.source as IconKey]?.color ?? '#5b9dff';
  };

  const seg = (active: boolean) => ({
    border: 'none',
    background: active ? ACCENT : 'transparent',
    color: active ? '#ffffff' : '#8891a6',
    padding: '6px 13px',
    borderRadius: '7px',
    fontSize: '12.5px',
    fontWeight: 650,
    cursor: 'pointer',
    fontFamily: 'inherit',
    transition: 'background .15s,color .15s',
  });

  const loading = snap === null && !error;

  return (
    <div className="tt">
      <div className="tt-app">
        <style>{SCOPED_CSS}</style>
        {/* ===== app top bar (same shell as Overview8b) ===== */}
        <div className="tt-top">
          <div className="tt-brand">
            <div className="tt-logo">
              <i>T</i>
              <b>tokentracker</b>
            </div>
            <div className="tt-nav">
              {NAV.map((n) => (
                <button
                  key={n}
                  className={n === nav ? 'active' : ''}
                  onClick={() => (n === 'Overview' || n === 'Limits') && onNav(n)}
                >
                  {n}
                </button>
              ))}
            </div>
          </div>
          <div className="tt-top-right">
            <span className="tt-avatar">BW</span>
          </div>
        </div>

        {/* ===== body ===== */}
        <div style={{ padding: '22px 24px 26px' }}>
          {/* heading */}
          <div style={{ marginBottom: '18px' }}>
            <div style={{ fontSize: '11px', fontWeight: 600, letterSpacing: '.14em', textTransform: 'uppercase', color: '#7f8aa0' }}>Rate limits</div>
            <div style={{ fontSize: '23px', fontWeight: 650, letterSpacing: '-.02em', color: '#f3f6fc', marginTop: '6px' }}>Usage &amp; quota across your tools</div>
            <div style={{ fontSize: '13px', color: '#8891a6', marginTop: '4px' }}>Rolling limit windows for every connected AI tool, refreshed on each scan.</div>
          </div>

          {/* toolbar */}
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: '16px', marginBottom: '16px', flexWrap: 'wrap', rowGap: '12px' }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
              <div style={{ display: 'inline-flex', background: 'rgba(255,255,255,.05)', border: '1px solid rgba(255,255,255,.08)', borderRadius: '10px', padding: '3px', gap: '2px' }}>
                <button style={seg(filter === 'all')} onClick={() => setFilter('all')}>All</button>
                <button style={seg(filter === 'connected')} onClick={() => setFilter('connected')}>Connected</button>
              </div>
              <span style={{ fontSize: '12px', color: '#7f8aa0', fontFamily: MONO }}>{countLabel}</span>
            </div>
            <div style={{ display: 'flex', alignItems: 'center', gap: '10px' }}>
              <div style={{ display: 'inline-flex', alignItems: 'center', gap: '8px' }}>
                <span style={{ fontSize: '11px', fontWeight: 600, letterSpacing: '.1em', textTransform: 'uppercase', color: '#6d7793' }}>Show</span>
                <div style={{ display: 'inline-flex', background: 'rgba(255,255,255,.05)', border: '1px solid rgba(255,255,255,.08)', borderRadius: '10px', padding: '3px', gap: '2px' }}>
                  <button style={seg(mode === 'used')} onClick={() => setMode('used')}>Used</button>
                  <button style={seg(mode === 'left')} onClick={() => setMode('left')}>Left</button>
                </div>
              </div>
              <button className="lim-iconbtn" title="Rescan" onClick={() => void load(true)}>
                <span style={{ display: 'inline-block', animation: refreshing ? 'tl-spin .7s linear infinite' : undefined }}>⟳</span>
              </button>
            </div>
          </div>

          {/* page-level error banner */}
          {error && (
            <div style={{ fontSize: '12.5px', color: '#f0616d', lineHeight: 1.5, marginBottom: '16px' }}>{error}</div>
          )}

          {/* grid of tool cards */}
          {loading ? (
            <div style={{ fontSize: '13px', color: '#6d7793' }}>Loading…</div>
          ) : (
            <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '16px', alignItems: 'start' }}>
              {visible.map((t) => {
                const meta = TOOL_META[t.source as IconKey] ?? { name: t.source, color: '#5b9dff' };
                const title = t.plan ? `${meta.name} ${t.plan}` : meta.name;
                return (
                  <div key={t.source} style={{ background: 'rgba(255,255,255,.02)', border: '1px solid rgba(255,255,255,.08)', borderRadius: '15px', padding: '16px 18px', display: 'flex', flexDirection: 'column', animation: 'tt-rise .45s cubic-bezier(.2,.7,.2,1) both' }}>
                    {/* card head */}
                    <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: '10px', marginBottom: '15px' }}>
                      <div style={{ display: 'flex', alignItems: 'center', gap: '10px', minWidth: 0 }}>
                        <span style={{ width: '28px', height: '28px', borderRadius: '8px', background: soft(meta.color, 0.14), color: meta.color, display: 'inline-flex', alignItems: 'center', justifyContent: 'center', flex: 'none' }}>
                          <ToolIcon icon={t.source as IconKey} />
                        </span>
                        <span style={{ fontSize: '14.5px', fontWeight: 650, color: '#f3f6fc', letterSpacing: '-.01em', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>{title}</span>
                      </div>
                      {t.stale && (
                        <span style={{ flex: 'none', display: 'inline-flex', alignItems: 'center', gap: '5px', fontSize: '10.5px', fontWeight: 600, color: '#37c98b', background: 'rgba(55,201,139,.12)', border: '1px solid rgba(55,201,139,.22)', borderRadius: '999px', padding: '3px 9px' }}>
                          <span style={{ width: '5px', height: '5px', borderRadius: '50%', background: '#37c98b', display: 'inline-block' }} />cached · {fmtAgo(t.cachedAtTs)}
                        </span>
                      )}
                    </div>

                    {/* body */}
                    {!t.configured ? (
                      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: '12px', padding: '2px 0' }}>
                        <span style={{ fontSize: '12.5px', color: '#6d7793' }}>Not connected</span>
                      </div>
                    ) : t.error ? (
                      <div style={{ fontSize: '12px', color: '#f0616d', lineHeight: 1.5 }}>{t.error}</div>
                    ) : t.windows.length === 0 ? (
                      <span style={{ fontSize: '12.5px', color: '#6d7793' }}>No usage data</span>
                    ) : (
                      <div style={{ display: 'flex', flexDirection: 'column', gap: '11px' }}>
                        {t.windows.map((w, i) => {
                          const dp = mode === 'used' ? w.usedPercent : 100 - w.usedPercent;
                          return (
                            <div key={i} style={{ display: 'grid', gridTemplateColumns: '48px 1fr 42px 30px', alignItems: 'center', gap: '12px' }}>
                              <span style={{ fontSize: '11.5px', color: '#7f8aa0', fontFamily: MONO }}>{w.label}</span>
                              <div style={{ position: 'relative', height: '7px', borderRadius: '4px', background: 'rgba(255,255,255,.05)' }}>
                                <div style={{ position: 'absolute', left: 0, top: 0, height: '100%', width: (dp > 0 ? Math.max(dp, 2) : 0) + '%', background: barColor(t, w.usedPercent), borderRadius: '4px', transition: 'width .7s cubic-bezier(.2,.75,.25,1)', overflow: 'hidden' }} />
                              </div>
                              <span style={{ fontSize: '12px', color: '#f3f6fc', fontWeight: 600, textAlign: 'right', fontFamily: MONO }}>{Math.round(dp)}%</span>
                              <span style={{ fontSize: '11px', color: '#6d7793', textAlign: 'right', fontFamily: MONO }}>{fmtRemain(w.resetsAtTs)}</span>
                            </div>
                          );
                        })}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          )}

          {/* footer */}
          <div style={{ display: 'flex', alignItems: 'center', gap: '9px', marginTop: '20px', fontSize: '11.5px', color: '#565e70' }}>
            <span style={{ width: '5px', height: '5px', borderRadius: '50%', background: '#37c98b', flex: 'none' }} />
            <span>Last checked {fmtAgo(snap?.fetchedAtTs ?? null)} · auto-refreshes every 5 min</span>
          </div>
        </div>
      </div>
    </div>
  );
}

