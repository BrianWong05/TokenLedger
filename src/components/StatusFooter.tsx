import type { ScanStatus } from '../types';

export interface StatusFooterProps {
  scanStatus: ScanStatus | null;
  scanning: boolean;
  unpricedModels: string[];
  onSetPrice: (model: string) => void;
}

function relativeTime(epochSec: number, nowMs: number): string {
  const diff = Math.max(0, Math.floor(nowMs / 1000) - epochSec);
  if (diff < 60) return `${diff}s ago`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  return `${Math.floor(diff / 86400)}d ago`;
}

export default function StatusFooter({
  scanStatus,
  scanning,
  unpricedModels,
  onSetPrice,
}: StatusFooterProps) {
  const scanText = scanning
    ? 'scanning…'
    : scanStatus
    ? `last scan ${relativeTime(scanStatus.scannedAt, Date.now())}`
    : 'not scanned yet';

  return (
    <div className="status-footer">
      <div className="footer-scan">{scanText}</div>

      <div className="footer-sources">
        {scanStatus?.sources.map((s) => (
          <span
            key={s.source}
            className={s.error ? 'source-stat error' : 'source-stat'}
            title={s.error ?? undefined}
          >
            {s.source}: {s.eventsInserted} in / {s.linesSkipped} skipped
            {s.error ? ' · error' : ''}
          </span>
        ))}
      </div>

      {unpricedModels.length > 0 && (
        <div className="footer-unpriced">
          <span className="footer-unpriced-label">Unpriced models:</span>
          {unpricedModels.map((m) => (
            <span key={m} className="unpriced-model">
              {m}
              <button
                type="button"
                className="set-price"
                onClick={() => onSetPrice(m)}
              >
                set price…
              </button>
            </span>
          ))}
        </div>
      )}
    </div>
  );
}
