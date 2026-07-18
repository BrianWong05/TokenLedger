import { useMemo, useRef, useState, type RefObject } from 'react';
import type { Summary } from '../types';
import { heatStats, type Day } from './data';
import { TOOLS } from './meta';
import { fmtPct, fmtTok } from '../lib/format';
import { fmtDateL, fmtWeekdayDateL, formatDisplayCost, useOverviewT } from './localize';
import { useChartColors, CHART_LIGHT } from '../lib/chartColors';
import { useSettings } from '../settings/SettingsContext';
import Landscape3D, { INITIAL_YAW } from './Landscape3D';
import { HEAT_DARK, HEAT_LIGHT } from './Heatmap';
import { useDialogChrome } from './useDialogChrome';

// Design 3a — the Activity card's full-screen enlarge. A centered dialog at 80%
// of the viewport: a top command band (title + scene controls), a stats band of
// the year's headline figures, and the 3D activity landscape given the full
// canvas below. Numbers come from the same heatStats + Summary the card shows.
export default function HeatmapModal({
  days,
  summary,
  returnFocusRef,
  onClose,
}: {
  days: Day[];
  summary: Summary | null; // year-window Summary; null while it loads
  returnFocusRef: RefObject<HTMLElement | null>;
  onClose: () => void;
}) {
  const { t, lang } = useOverviewT();
  const { settings } = useSettings();
  const colors = useChartColors();
  const ramp = colors === CHART_LIGHT ? HEAT_LIGHT : HEAT_DARK;
  const accent = ramp[3];

  const [yaw, setYaw] = useState(INITIAL_YAW);
  const [hover, setHover] = useState<Day | null>(null);
  const [pos, setPos] = useState<{ x: number; y: number; flip: boolean }>({ x: 0, y: 0, flip: false });

  const modalRef = useRef<HTMLElement>(null);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  useDialogChrome({ modalRef, initialFocusRef: closeButtonRef, returnFocusRef, onClose });
  // A drag that starts on the landscape and releases over the dimmed margin
  // makes the backdrop the click target (click fires on the common ancestor of
  // mousedown/mouseup) — only close when the press also began on the backdrop.
  const backdropArmed = useRef(false);

  const stats = useMemo(() => heatStats(days), [days]);
  const activeRate = fmtPct(stats.activeDays / Math.max(1, days.length));
  const activeTools = useMemo(() => {
    const totals = new Map<string, number>();
    for (const d of days) for (const tl of TOOLS) totals.set(tl.key, (totals.get(tl.key) ?? 0) + d.byTool[tl.key]);
    return TOOLS.filter((tl) => (totals.get(tl.key) ?? 0) > 0);
  }, [days]);
  const costLabel =
    summary === null ? '…' : formatDisplayCost(summary.cost, summary.hasUnpriced, settings, lang);

  // tooltip per-tool rows for the hovered day (mirrors the card)
  const tipRows = hover
    ? TOOLS.map((tl) => ({ ...tl, val: hover.byTool[tl.key] }))
        .filter((r) => r.val > 0)
        .sort((a, b) => b.val - a.val)
        .slice(0, 3)
    : [];
  const tipMax = Math.max(1, ...tipRows.map((r) => r.val));

  function onMove(e: React.MouseEvent<HTMLDivElement>) {
    const w = e.currentTarget.clientWidth;
    setPos({ x: e.nativeEvent.offsetX, y: e.nativeEvent.offsetY, flip: e.nativeEvent.offsetX > w * 0.58 });
  }

  return (
    <div
      className="tt-heat-modal-backdrop"
      onMouseDown={(event) => {
        backdropArmed.current = event.target === event.currentTarget;
      }}
      onClick={(event) => {
        if (event.target === event.currentTarget && backdropArmed.current) onClose();
        backdropArmed.current = false;
      }}
    >
      <section
        ref={modalRef}
        className="tt-heat-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="tt-heat-modal-title"
        tabIndex={-1}
      >
        <header className="tt-heat-modal-head">
          <div>
            <div className="tt-heat-modal-eyebrow">
              <span className="dot" />
              {t('overview.insight3d')}
            </div>
            <div className="tt-heat-modal-title" id="tt-heat-modal-title">
              {t('overview.token3dPerspective')}
            </div>
          </div>
          <span className="tt-heat-modal-spacer" />
          <div className="tt-heat-modal-controls">
            {activeTools.map((tl) => (
              <span key={tl.key} className="tool" style={{ background: tl.color }} title={tl.label} />
            ))}
            <span className="div" />
            <button
              type="button"
              className="reset"
              onClick={() => setYaw(INITIAL_YAW)}
              title={t('overview.resetView')}
              aria-label={t('overview.resetView')}
            >
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
                <path d="M3 3v5h5" />
              </svg>
            </button>
          </div>
          <button
            ref={closeButtonRef}
            type="button"
            className="tt-heat-modal-close"
            onClick={onClose}
            aria-label={t('overview.close')}
          >
            <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M18 6 6 18" />
              <path d="m6 6 12 12" />
            </svg>
          </button>
        </header>

        <div className="tt-heat-modal-stats">
          <div className="stat">
            <span className="lbl">{t('overview.totalTokens')}</span>
            <span className="val">{fmtTok(stats.totalTokens)}</span>
          </div>
          <span className="sep" />
          <div className="stat" title={t('overview.notBilled')}>
            <span className="lbl">{t('overview.estCost')}</span>
            <span className="val cost">
              {costLabel}
              {summary?.hasUnpriced && (
                <span className="muted" title={summary.unpricedModels.join(', ')}>
                  {' '}· {summary.unpricedModels.length} {t('overview.unpricedMarker')}
                </span>
              )}
            </span>
          </div>
          <span className="sep" />
          <div className="stat">
            <span className="lbl">{t('overview.activeDays')}</span>
            <span className="val">
              {stats.activeDays} <span className="muted">/ {activeRate}</span>
            </span>
          </div>
          <span className="sep" />
          <div className="stat">
            <span className="lbl">{t('overview.longestStreak')}</span>
            <span className="val streak">
              {stats.streak} <span className="unit">{t('overview.daysUnit')}</span>
            </span>
          </div>
          <span className="sep" />
          <div className="stat">
            <span className="lbl">{t('overview.peakDay')}</span>
            <span className="val">
              {fmtTok(stats.bestDay.tokens)} <span className="muted">{fmtDateL(stats.bestDay.date, lang)}</span>
            </span>
          </div>
        </div>

        <div className="tt-heat-modal-canvas" onMouseMove={onMove} onMouseLeave={() => setHover(null)}>
          <Landscape3D days={days} ramp={ramp} yaw={yaw} onYaw={setYaw} onHoverDay={setHover} />

          {hover && (
            <div
              className="tt-tip"
              style={{
                left: pos.x,
                top: pos.y,
                transform: `translate(${pos.flip ? 'calc(-100% - 14px)' : '14px'}, -50%)`,
              }}
            >
              <div className="tt-tip-head">
                <b>{fmtWeekdayDateL(hover.date, lang)}</b>
                <span className="tt-tip-badge">Lv {hover.level}</span>
              </div>
              <div className="tt-tip-tok">
                <b>{fmtTok(hover.tokens)}</b>
                <span>{t('overview.tokens')}</span>
              </div>
              {tipRows.map((r) => (
                <div className="tt-tip-row" key={r.key}>
                  <div className="lab">
                    <span>{r.label}</span>
                    <span>{fmtTok(r.val)}</span>
                  </div>
                  <div className="track">
                    <div className="fill" style={{ width: (r.val / tipMax) * 100 + '%', background: accent }} />
                  </div>
                </div>
              ))}
              {tipRows.length === 0 && <div className="tt-ctx-meta">{t('overview.noActivity')}</div>}
            </div>
          )}

          <div className="tt-heat-modal-legend">
            <span className="cap">{t('overview.activity')}</span>
            <span>{t('overview.heatLess')}</span>
            {ramp.map((c, i) => (
              <span key={i} className="cell" style={{ background: c }} />
            ))}
            <span>{t('overview.heatMore')}</span>
          </div>
          <div className="tt-heat-modal-hint">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke={accent} strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <circle cx="12" cy="12" r="10" />
              <path d="M12 16v-4" />
              <path d="M12 8h.01" />
            </svg>
            {t('overview.dragRotate')}
          </div>
        </div>
      </section>
    </div>
  );
}
