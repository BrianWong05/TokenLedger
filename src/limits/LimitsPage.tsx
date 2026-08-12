// Limits tab: one card per Source with a vendor Limit — how much of each rolling
// window is used, when it resets, and whether the pace is comfortable. It ignores
// the Overview's date window and Source selection entirely: this page is *now*,
// not a range.
//
// Every rule it draws by lives in limits.derive.ts; this file is layout, strings,
// and the two things that touch the outside world — reading the stored series and
// asking a Companion for a live one. Fetch policy is page-open plus manual
// Refresh only, never on the scan timer and never in the background (a floor of
// LIVE_FLOOR_MS between live checks per Source enforces it against a fast tab
// flipper).
import { useCallback, useEffect, useMemo, useState } from 'react';
import './limits.css';
import { useT } from '../lib/i18n';
import { fill } from '../lib/format';
import { sourceIcon } from '../overview/icons';
import type { SourceLimits } from '../types';
import {
  cards, durationParts, freshness, limitsSources, planLabel, windowLabel,
  type CardView, type Mode, type WindowView,
} from './limits.derive';
import { tauriLimits, lastCheckKey, LIVE_ENABLED_KEY, MODE_KEY, type LimitsPort } from './limits';

type T = ReturnType<typeof useT>['t'];

// Decision 5's floor: at most one live check per Source per minute, however often
// the page is opened. The last-check stamp is *stored* rather than held in a ref
// because the shell unmounts this page on every tab switch — a ref would reset
// with it, and flipping away and back would fetch again immediately.
const LIVE_FLOOR_MS = 60_000;

export default function LimitsPage({
  ports,
  now = () => Date.now(),
}: {
  ports?: { limits?: LimitsPort };
  now?: () => number;
} = {}) {
  const port = ports?.limits ?? tauriLimits;
  const { t } = useT();

  const [stored, setStored] = useState<SourceLimits[]>([]);
  const [mode, setMode] = useState<Mode>(() => (port.read(MODE_KEY) === 'used' ? 'used' : 'left'));
  const [liveEnabled, setLiveEnabled] = useState(() => port.read(LIVE_ENABLED_KEY) === 'true');
  const [failures, setFailures] = useState<Record<string, 'signed-out' | { detail: string }>>({});
  // In-flight checks, counted rather than boolean: a Refresh runs a scan and a
  // live check concurrently, and whichever finishes first must not re-enable the
  // button under the other.
  const [checking, setChecking] = useState(0);
  // The clock the ticks and the freshness line read. Re-read on every render pass
  // the page triggers; nothing here polls.
  const [nowSec, setNowSec] = useState(() => Math.floor(now() / 1000));

  const reload = useCallback(() => {
    setNowSec(Math.floor(now() / 1000));
    // Promise.resolve guards a port whose IPC throws synchronously when no Tauri
    // runtime is present.
    try {
      Promise.resolve(port.list()).then(setStored).catch(() => {});
    } catch { /* no runtime */ }
  }, [port, now]);

  // One live check per `live` Source, subject to the floor. Failures are recorded
  // per Source and kept distinct: exit 44 or a refused credential is "not signed
  // in", anything else is "couldn't check" with the Companion's own line.
  const checkLive = useCallback(
    () => {
      // The floor has no override, not even a button: it is a promise to the
      // vendor's endpoint (whose budget is shared with the Source's own CLI), so
      // "a person asked" is what permits a call at all, never what exempts it.
      // Refresh still always runs a scan, which is how a `logs` Source updates,
      // so the button is never inert.
      const due = limitsSources()
        .filter(({ via }) => via === 'live')
        .map(({ meta }) => meta.key)
        .filter((key) => now() - Number(port.read(lastCheckKey(key)) ?? 0) >= LIVE_FLOOR_MS);
      if (!due.length) return;

      setChecking((n) => n + 1);
      Promise.all(
        due.map((key) => {
          port.write(lastCheckKey(key), String(now()));
          return Promise.resolve(port.checkLive(key)).then(
            () => setFailures((f) => {
              const { [key]: _gone, ...rest } = f;
              return rest;
            }),
            (err: unknown) => {
              const detail = String((err as { message?: string })?.message ?? err ?? '');
              setFailures((f) => ({
                ...f,
                [key]: signedOut(detail) ? 'signed-out' : { detail },
              }));
            },
          );
        }),
      )
        .catch(() => {})
        .finally(() => {
          setChecking((n) => n - 1);
          reload();
        });
    },
    [port, now, reload],
  );

  // Page open: read the stored series always, and check live only once the
  // disclosure has been accepted. No credential is read before that.
  useEffect(() => {
    reload();
    if (liveEnabled) checkLive();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- page-open only, by design
  }, []);

  const refresh = () => {
    // For a `logs` Source, Refresh is just an ordinary scan — and it shows as
    // one. Without this bracket the button gave no feedback at all inside the
    // live floor, and an unchanged Codex card made Refresh look broken rather
    // than honest (its figures can never be fresher than the last request the
    // logs hold).
    setChecking((n) => n + 1);
    Promise.resolve(port.scan())
      .catch(() => {})
      .finally(() => {
        setChecking((n) => n - 1);
        reload();
      });
    if (liveEnabled) checkLive();
  };

  const enableLive = () => {
    port.write(LIVE_ENABLED_KEY, 'true');
    setLiveEnabled(true);
    checkLive();
  };

  const pick = (next: Mode) => {
    setMode(next);
    port.write(MODE_KEY, next);
  };

  const views = useMemo(() => cards(stored, nowSec, mode, failures), [stored, nowSec, mode, failures]);

  return (
    <div className="tl-page tl-page-limits">
      {/* the toolbar's empty stretch is a window-drag handle (frameless window) */}
      <div className="tl-lim-toolbar" data-tauri-drag-region>
        <h1>{t('limits.title')}</h1>
        <span className="tl-lim-note">{t('limits.note')}</span>
        <span className="tl-lim-spacer" data-tauri-drag-region />
        <div className="tl-lim-mode" role="group" aria-label={t('limits.title')}>
          {(['left', 'used'] as Mode[]).map((m) => (
            <button
              key={m}
              type="button"
              className={mode === m ? 'active' : ''}
              aria-pressed={mode === m}
              onClick={() => pick(m)}
            >
              {t(m === 'left' ? 'limits.mode.left' : 'limits.mode.used')}
            </button>
          ))}
        </div>
        <button type="button" className="tl-lim-refresh" onClick={refresh} disabled={checking > 0}>
          {t(checking > 0 ? 'limits.refreshing' : 'limits.refresh')}
        </button>
      </div>

      {liveEnabled ? (
        <div className="tl-lim-grid">
          {views.map((card) => (
            <Card key={card.source} card={card} mode={mode} nowSec={nowSec} t={t} onRetry={() => checkLive()} />
          ))}
        </div>
      ) : (
        <OptIn t={t} onEnable={enableLive} />
      )}
    </div>
  );
}

// A missing or refused credential is the one failure that reads as "sign in
// again"; every other failure is a failure and must not wear that face. The
// Companion is what can tell them apart — it knows a 401 on the token from a 403
// the same token earns on one method while another succeeds — so it marks the
// signed-out case with this prefix and the page trusts that, rather than
// re-deriving it by grepping for a status code that also appears in the text of
// a genuine error (e.g. "the vendor answered 403 … PERMISSION_DENIED").
function signedOut(detail: string): boolean {
  return /\bnot signed in\b/i.test(detail);
}

function Card({
  card, mode, nowSec, t, onRetry,
}: { card: CardView; mode: Mode; nowSec: number; t: T; onRetry: () => void }) {
  const icon = sourceIcon(card.meta.icon);
  const fresh = card.state === 'live' ? freshness(card, nowSec) : null;

  return (
    <section className={'tl-lim-card' + (card.state === 'live' ? '' : ' off')}>
      <div className="tl-lim-head">
        {icon ? <img src={icon} alt="" width={18} height={18} /> : <span className="tl-lim-nomark" />}
        <span className="tl-lim-name">{card.meta.label}</span>
        {card.plan && <span className="tl-lim-plan">{planLabel(card.plan)}</span>}
        {fresh && (
          <span className={'tl-lim-fresh' + (fresh.key === 'observedOld' ? ' old' : '')}>
            {fresh.key === 'checkedNow'
              ? t('limits.checkedNow')
              : fill(t(`limits.${fresh.key}` as 'limits.checkedAgo'), { t: fmtDuration(t, fresh.ageMin) })}
          </span>
        )}
      </div>

      {card.state === 'live' ? (
        card.windows.map((w) => <Row key={w.key} w={w} mode={mode} t={t} />)
      ) : (
        <Trouble card={card} t={t} onRetry={onRetry} />
      )}
    </section>
  );
}

function Row({ w, mode, t }: { w: WindowView; mode: Mode; t: T }) {
  const label = windowLabel(w.key);
  const resets = w.resetsInMin === null ? null : fmtDuration(t, w.resetsInMin);
  const spent = w.pctLeft <= 0;

  return (
    <div className="tl-lim-row">
      {/* A used-up window replaces its figures rather than reading "0% left" —
          the column keeps its width so the bars below stay aligned. */}
      {spent ? (
        <span className="tl-lim-num" aria-hidden="true" />
      ) : (
        <span className={'tl-lim-num ' + w.tone}>
          {Math.round(w.pct)}
          <span className="pct">%</span>
        </span>
      )}
      <div className="tl-lim-body">
        <div className="tl-lim-labels">
          <span className="tl-lim-label">
            {poolPrefix(t, label.pool)}
            {label.kind === 'session' && t('limits.win.session')}
            {label.kind === 'weekly' && t('limits.win.weekly')}
            {label.kind === 'other' && fill(t('limits.win.other'), { n: label.minutes })}
            {label.kind === 'model' && (
              <>
                {label.model}
                <span className="sub">{t('limits.win.weeklySub')}</span>
              </>
            )}
          </span>
          <span className={'tl-lim-resets' + (spent ? ' spent' : '')}>
            {resets && fill(t(spent ? 'limits.spent' : 'limits.resetsIn'), { t: resets })}
          </span>
        </div>
        <div
          className={'tl-lim-bar ' + w.tone}
          role="img"
          aria-label={fill(t(mode === 'left' ? 'limits.pctLeft' : 'limits.pctUsed'), {
            pct: Math.round(w.pct),
          })}
        >
          <div className="fill" style={{ width: `${w.pct}%` }} />
          {/* the tick is TIME (where "now" sits in the window), so it stays
              neutral — never tinted by the scarcity tones around it */}
          {w.tickPct !== null && resets && (
            <div
              className="tick"
              style={{ left: `${w.tickPct}%` }}
              title={fill(t('limits.tickTitle'), { t: resets })}
            />
          )}
        </div>
      </div>
    </div>
  );
}

// Signed-out, nothing-recorded, and error bodies. The first two are the same
// muted shape (a Source we can see but have no figures for); the third is
// danger-tinted and carries the Companion's raw line, because conflating a
// failure with an absence would send someone to fix the wrong thing.
function Trouble({ card, t, onRetry }: { card: CardView; t: T; onRetry: () => void }) {
  if (card.state === 'error') {
    return (
      <div className="tl-lim-trouble error">
        <span className="title">{t('limits.error')}</span>
        <span className="hint mono">{card.detail}</span>
        <button type="button" className="tl-lim-ghost" onClick={onRetry}>
          {t('limits.retry')}
        </button>
      </div>
    );
  }
  if (card.state === 'nothing-recorded') {
    return (
      <div className="tl-lim-trouble">
        <span className="title">{fill(t('limits.nothingRecorded'), { label: card.meta.label })}</span>
        <span className="hint">{t('limits.nothingRecordedHint')}</span>
      </div>
    );
  }
  return (
    <div className="tl-lim-trouble">
      <span className="title">{t('limits.signedOut')}</span>
      <span className="hint">{fill(t('limits.signedOutHint'), { cli: card.source })}</span>
      <button type="button" className="tl-lim-ghost" onClick={onRetry}>
        {t('limits.checkAgain')}
      </button>
    </div>
  );
}

// The disclosure. It replaces the whole card area on first visit, states exactly
// what enabling does and what it will not do, and gates only `live` acquisition —
// `logs` Readings have been flowing from ordinary scans all along, so a Source
// like Codex may already have history the moment this is dismissed.
function OptIn({ t, onEnable }: { t: T; onEnable: () => void }) {
  return (
    <div className="tl-lim-optin">
      <h2>{t('limits.optinTitle')}</h2>
      <p>{t('limits.optinBody')}</p>
      <p className="bounds">{t('limits.optinBounds')}</p>
      <button type="button" className="tl-lim-primary" onClick={onEnable}>
        {t('limits.optinButton')}
      </button>
    </div>
  );
}

// A window key's pool, where it carries one. `3p` is named "Other models"
// rather than "Claude" because what it really means is *non-Gemini* — the set
// behind it changes, and a label naming today's members would rot. A pool
// nobody has named renders raw, the way an unseen per-model window does.
function poolPrefix(t: T, pool: string | undefined): string {
  if (!pool) return '';
  const named: Record<string, string> = {
    gemini: t('limits.pool.gemini'),
    '3p': t('limits.pool.other'),
  };
  return `${named[pool] ?? pool} · `;
}

// "1d 6h" — largest two units, in the reader's own language.
function fmtDuration(t: T, minutes: number): string {
  return durationParts(minutes)
    .map((p) => fill(t(`limits.t.${p.unit}` as 'limits.t.d'), { n: p.n }))
    .join(' ');
}
