import { useEffect, useRef, useState } from 'react';

// Design — "App · Limits". Rolling rate-limit windows for every connected AI
// tool. Static design mockup: all figures are hard-coded here (no backend);
// mode/filter/refresh/connect are live client-side interactions, faithfully
// ported from the Claude Design canvas file (Limits.dc.html).

const MONO = "ui-monospace,'SF Mono',Menlo,monospace";

// Design-time knobs (DC props in the source). Defaults from the canvas.
const ACCENT = '#2a6df4';
const COMPACT_ROWS = false;
const SHOW_TICKS = true;

type IconKey =
  | 'claude' | 'codex' | 'cursor' | 'gemini' | 'kimi'
  | 'kiro' | 'grok' | 'copilot' | 'antigravity' | 'zcode';

type BarDef = { label: string; pct: number; remain: string; tick?: number; warn?: boolean };

type ToolDef = {
  key: string;
  icon: IconKey;
  color: string;
  name: string;
  bars?: BarDef[];
  status?: 'nc' | 'err';
  errorMsg?: string;
  defBars?: BarDef[];
  badge?: boolean;
};

const RAW: ToolDef[] = [
  { key: 'claude', icon: 'claude', color: '#d97757', name: 'Claude Pro', bars: [
    { label: '5h', pct: 11, remain: '2h', tick: 62 },
    { label: '7d', pct: 28, remain: '1d', tick: 88 },
    { label: 'Fable', pct: 43, remain: '1d', tick: 88 },
  ] },
  { key: 'codex', icon: 'codex', color: '#59c2a6', name: 'Codex Plus', bars: [
    { label: '5h', pct: 1, remain: '4h' },
    { label: '7d', pct: 16, remain: '6d', warn: true },
  ] },
  { key: 'cursor', icon: 'cursor', color: '#9aa3b2', name: 'Cursor', status: 'nc',
    defBars: [{ label: '5h', pct: 0, remain: '5h' }, { label: '7d', pct: 0, remain: '7d' }] },
  { key: 'gemini', icon: 'gemini', color: '#e2a63b', name: 'Gemini Paid', bars: [
    { label: 'Pro', pct: 0, remain: '23h' },
    { label: 'Flash', pct: 0, remain: '23h' },
    { label: 'Lite', pct: 0, remain: '23h' },
  ] },
  { key: 'kimi', icon: 'kimi', color: '#8f7be8', name: 'Kimi', status: 'nc',
    defBars: [{ label: '5h', pct: 0, remain: '5h' }, { label: '7d', pct: 0, remain: '7d' }] },
  { key: 'kiro', icon: 'kiro', color: '#7c93ff', name: 'Kiro', status: 'nc',
    defBars: [{ label: '5h', pct: 0, remain: '5h' }, { label: '7d', pct: 0, remain: '7d' }] },
  { key: 'grok', icon: 'grok', color: '#b8c0cc', name: 'Grok Build', status: 'err',
    errorMsg: 'Error: Not logged in to Grok Build. Run `grok login` in Terminal to authenticate.',
    defBars: [{ label: '5h', pct: 0, remain: '5h' }, { label: '7d', pct: 0, remain: '7d' }] },
  { key: 'copilot', icon: 'copilot', color: '#a0a8b6', name: 'GitHub Copilot', status: 'nc',
    defBars: [{ label: 'Chat', pct: 0, remain: '—' }, { label: 'Compl.', pct: 0, remain: '—' }] },
  { key: 'antigravity', icon: 'antigravity', color: '#37c98b', name: 'Antigravity', badge: true, bars: [
    { label: 'Cl 7d', pct: 0, remain: '6d' },
    { label: 'Cl 5h', pct: 0, remain: '2h' },
    { label: 'Gm 7d', pct: 24, remain: '18h' },
    { label: 'Gm 5h', pct: 0, remain: '2h' },
  ] },
  { key: 'zcode', icon: 'zcode', color: '#5b9dff', name: 'ZCode', bars: [
    { label: '5h', pct: 8, remain: '3h' },
    { label: '7d', pct: 19, remain: '5d' },
  ] },
];

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
    case 'cursor':
      return <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinejoin="round"><rect x="4" y="4" width="16" height="16" rx="3.5" /></svg>;
    case 'gemini':
      return <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><path d="M12 3l5.6 9L12 21l-5.6-9z" /></svg>;
    case 'kiro':
      return <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><circle cx="12" cy="12" r="8" /></svg>;
    case 'grok':
      return <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round"><circle cx="12" cy="12" r="8" /><path d="M6.7 17.3L17.3 6.7" /></svg>;
    case 'copilot':
      return <svg width="17" height="17" viewBox="0 0 24 24" fill="currentColor"><rect x="3" y="8" width="18" height="9.5" rx="4.75" /><circle cx="9" cy="12.75" r="1.5" fill="#0b0d13" /><circle cx="15" cy="12.75" r="1.5" fill="#0b0d13" /></svg>;
    case 'antigravity':
      return <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><path d="M12 4l8.5 15.5h-17z" /></svg>;
    case 'kimi':
    case 'zcode':
      return <span style={{ fontSize: '13px', fontWeight: 800, fontFamily: MONO }}>{icon === 'kimi' ? 'K' : 'Z'}</span>;
  }
}

const SCOPED_CSS = `
.lim a{color:#5b9dff;text-decoration:none;}
.lim a:hover{color:#84b6ff;text-decoration:underline;}
.lim-nav{padding:6px 12px;border-radius:8px;font-size:13px;font-weight:500;color:#8891a6;cursor:pointer;transition:background .15s,color .15s;}
.lim-nav:hover{background:rgba(255,255,255,.04);color:#cfd6e6;}
.lim-iconbtn{width:34px;height:34px;border-radius:9px;background:rgba(255,255,255,.05);border:1px solid rgba(255,255,255,.08);color:#a9b2c4;display:inline-flex;align-items:center;justify-content:center;cursor:pointer;font-size:16px;line-height:1;transition:border-color .15s,color .15s;}
.lim-iconbtn:hover{border-color:rgba(255,255,255,.18);color:#e8ecf4;}
.lim-ghost{background:rgba(255,255,255,.05);border:1px solid rgba(255,255,255,.1);border-radius:8px;color:#cfd6e6;padding:6px 13px;font-size:12px;font-weight:650;cursor:pointer;font-family:inherit;transition:border-color .15s,color .15s;}
.lim-ghost:hover{border-color:rgba(255,255,255,.2);color:#f3f6fc;}
@keyframes tt-rise{from{opacity:0;transform:translateY(14px);}to{opacity:1;transform:none;}}
@keyframes tl-spin{to{transform:rotate(360deg);}}
`;

const NAV = ['Overview', 'Activity', 'Models', 'Limits', 'Settings'];

export default function Limits() {
  const [mounted, setMounted] = useState(false);
  const [mode, setMode] = useState<'used' | 'left'>('used');
  const [filter, setFilter] = useState<'all' | 'connected'>('all');
  const [refreshing, setRefreshing] = useState(false);
  const [cachedAge, setCachedAge] = useState('2h');
  const [connected, setConnected] = useState<Record<string, boolean>>({});
  const timer = useRef<ReturnType<typeof setTimeout>>();

  useEffect(() => {
    const t = setTimeout(() => setMounted(true), 90);
    return () => {
      clearTimeout(t);
      clearTimeout(timer.current);
    };
  }, []);

  const refresh = () => {
    if (refreshing) return;
    setMounted(false);
    setRefreshing(true);
    clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      setMounted(true);
      setRefreshing(false);
      setCachedAge('now');
    }, 500);
  };
  const connect = (key: string) => setConnected((c) => ({ ...c, [key]: true }));

  let total = 0;
  let conn = 0;
  let tools = RAW.map((t) => {
    let st: 'bars' | 'nc' | 'err' = t.status ?? 'bars';
    let barsData = t.bars ?? [];
    if ((st === 'nc' || st === 'err') && connected[t.key]) {
      st = 'bars';
      barsData = t.defBars ?? [];
    }
    total++;
    if (st === 'bars') conn++;
    const bars = barsData.map((b) => {
      const dp = mode === 'used' ? b.pct : 100 - b.pct;
      let fc = t.color;
      if (mode === 'used') {
        if (b.pct >= 90) fc = '#f0616d';
        else if (b.pct >= 75) fc = '#e2a63b';
      }
      return {
        label: b.label,
        pctText: dp + '%',
        remain: b.remain,
        fillW: mounted ? (dp > 0 ? Math.max(dp, 2) : 0) + '%' : '0%',
        fillColor: fc,
        hasTick: SHOW_TICKS && mode === 'used' && b.tick != null,
        tickLeft: (b.tick ?? 0) + '%',
        hasWarn: mode === 'used' && !!b.warn && dp > 2,
      };
    });
    return {
      key: t.key,
      icon: t.icon,
      color: t.color,
      soft: soft(t.color, 0.14),
      name: t.name,
      hasBadge: !!t.badge,
      badgeText: 'cached · ' + cachedAge,
      status: st,
      errorMsg: t.errorMsg ?? '',
      bars,
      rowGap: COMPACT_ROWS ? '8px' : '11px',
      headMb: COMPACT_ROWS ? '12px' : '15px',
    };
  });
  const countLabel = total + ' tools · ' + conn + ' connected';
  if (filter === 'connected') tools = tools.filter((t) => t.status === 'bars');

  const lastChecked = cachedAge === 'now' ? 'just now' : cachedAge + ' ago';

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

  return (
    <div
      className="lim"
      style={{
        minHeight: '100vh',
        background: '#131519',
        backgroundImage: 'radial-gradient(1200px 620px at 30% -14%, #1b1e26 0%, transparent 60%)',
        padding: '44px',
        fontFamily: "-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,'Helvetica Neue',Arial,sans-serif",
        fontVariantNumeric: 'tabular-nums',
        WebkitFontSmoothing: 'antialiased',
      }}
    >
      <style>{SCOPED_CSS}</style>
      <div
        style={{
          width: '100%',
          maxWidth: '1180px',
          margin: '0 auto',
          background: '#0b0d13',
          border: '1px solid rgba(255,255,255,.09)',
          borderRadius: '18px',
          overflow: 'hidden',
          color: '#e8ecf4',
          boxShadow: '0 40px 90px -46px rgba(0,0,0,.92)',
          animation: 'tt-rise .5s cubic-bezier(.2,.7,.2,1) both',
        }}
      >
        {/* ===== app top bar ===== */}
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: '20px', padding: '15px 22px', borderBottom: '1px solid rgba(255,255,255,.07)', background: 'linear-gradient(180deg, rgba(255,255,255,.03), transparent)' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: '26px' }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: '9px' }}>
              <span style={{ width: '22px', height: '22px', borderRadius: '6px', background: 'linear-gradient(135deg,#3b82f6,#1d4ed8)', display: 'inline-flex', alignItems: 'center', justifyContent: 'center', fontSize: '12px', fontWeight: 800, color: '#fff', fontFamily: "ui-monospace,'SF Mono',Menlo,monospace" }}>T</span>
              <span style={{ fontSize: '14.5px', fontWeight: 700, letterSpacing: '-.01em', color: '#f3f6fc' }}>tokentracker</span>
            </div>
            <div style={{ display: 'flex', alignItems: 'center', gap: '4px' }}>
              {NAV.map((n) =>
                n === 'Limits' ? (
                  <span key={n} style={{ padding: '6px 12px', borderRadius: '8px', fontSize: '13px', fontWeight: 600, color: '#f3f6fc', background: 'rgba(255,255,255,.07)', cursor: 'pointer' }}>{n}</span>
                ) : (
                  <span key={n} className="lim-nav">{n}</span>
                ),
              )}
            </div>
          </div>
          <div style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
            <span style={{ width: '28px', height: '28px', borderRadius: '50%', background: 'linear-gradient(135deg,#37c98b,#1f8a5b)', display: 'inline-flex', alignItems: 'center', justifyContent: 'center', fontSize: '11px', fontWeight: 700, color: '#06130d' }}>MK</span>
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
              <button className="lim-iconbtn" title="Rescan" onClick={refresh}>
                <span style={{ display: 'inline-block', animation: refreshing ? 'tl-spin .7s linear infinite' : undefined }}>⟳</span>
              </button>
            </div>
          </div>

          {/* grid of tool cards */}
          <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '16px', alignItems: 'start' }}>
            {tools.map((t) => (
              <div key={t.key} style={{ background: 'rgba(255,255,255,.02)', border: '1px solid rgba(255,255,255,.08)', borderRadius: '15px', padding: '16px 18px', display: 'flex', flexDirection: 'column', animation: 'tt-rise .45s cubic-bezier(.2,.7,.2,1) both' }}>
                {/* card head */}
                <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: '10px', marginBottom: t.headMb }}>
                  <div style={{ display: 'flex', alignItems: 'center', gap: '10px', minWidth: 0 }}>
                    <span style={{ width: '28px', height: '28px', borderRadius: '8px', background: t.soft, color: t.color, display: 'inline-flex', alignItems: 'center', justifyContent: 'center', flex: 'none' }}>
                      <ToolIcon icon={t.icon} />
                    </span>
                    <span style={{ fontSize: '14.5px', fontWeight: 650, color: '#f3f6fc', letterSpacing: '-.01em', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>{t.name}</span>
                  </div>
                  {t.hasBadge && (
                    <span style={{ flex: 'none', display: 'inline-flex', alignItems: 'center', gap: '5px', fontSize: '10.5px', fontWeight: 600, color: '#37c98b', background: 'rgba(55,201,139,.12)', border: '1px solid rgba(55,201,139,.22)', borderRadius: '999px', padding: '3px 9px' }}>
                      <span style={{ width: '5px', height: '5px', borderRadius: '50%', background: '#37c98b', display: 'inline-block' }} />{t.badgeText}
                    </span>
                  )}
                </div>

                {/* bars */}
                {t.status === 'bars' && (
                  <div style={{ display: 'flex', flexDirection: 'column', gap: t.rowGap }}>
                    {t.bars.map((b, i) => (
                      <div key={i} style={{ display: 'grid', gridTemplateColumns: '48px 1fr 42px 30px', alignItems: 'center', gap: '12px' }}>
                        <span style={{ fontSize: '11.5px', color: '#7f8aa0', fontFamily: MONO }}>{b.label}</span>
                        <div style={{ position: 'relative', height: '7px', borderRadius: '4px', background: 'rgba(255,255,255,.05)' }}>
                          <div style={{ position: 'absolute', left: 0, top: 0, height: '100%', width: b.fillW, background: b.fillColor, borderRadius: '4px', transition: 'width .7s cubic-bezier(.2,.75,.25,1)', overflow: 'hidden' }}>
                            {b.hasWarn && <span style={{ position: 'absolute', right: 0, top: 0, height: '100%', width: '3px', background: '#f0616d' }} />}
                          </div>
                          {b.hasTick && <span style={{ position: 'absolute', top: '-1.5px', height: '10px', width: '2px', left: b.tickLeft, background: 'rgba(255,255,255,.32)', borderRadius: '1px' }} />}
                        </div>
                        <span style={{ fontSize: '12px', color: '#f3f6fc', fontWeight: 600, textAlign: 'right', fontFamily: MONO }}>{b.pctText}</span>
                        <span style={{ fontSize: '11px', color: '#6d7793', textAlign: 'right', fontFamily: MONO }}>{b.remain}</span>
                      </div>
                    ))}
                  </div>
                )}

                {/* not connected */}
                {t.status === 'nc' && (
                  <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: '12px', padding: '2px 0' }}>
                    <span style={{ fontSize: '12.5px', color: '#6d7793' }}>Not connected</span>
                    <button className="lim-ghost" onClick={() => connect(t.key)}>Connect</button>
                  </div>
                )}

                {/* error */}
                {t.status === 'err' && (
                  <div>
                    <div style={{ fontSize: '12px', color: '#f0616d', lineHeight: 1.5 }}>{t.errorMsg}</div>
                    <button className="lim-ghost" style={{ marginTop: '11px' }} onClick={() => connect(t.key)}>Authenticate</button>
                  </div>
                )}
              </div>
            ))}
          </div>

          {/* footer */}
          <div style={{ display: 'flex', alignItems: 'center', gap: '9px', marginTop: '20px', fontSize: '11.5px', color: '#565e70' }}>
            <span style={{ width: '5px', height: '5px', borderRadius: '50%', background: '#37c98b', flex: 'none' }} />
            <span>Last checked {lastChecked} · auto-refreshes every 5 min</span>
          </div>
        </div>
      </div>
    </div>
  );
}
