import type { Summary } from '../types';
import { formatTokens } from '../lib/format';

export interface StatCardsProps {
  summary: Summary | null;
}

function StatCard({ label, value }: { label: string; value: string }) {
  return (
    <div className="stat-card">
      <div className="stat-label">{label}</div>
      <div className="stat-value">{value}</div>
    </div>
  );
}

export default function StatCards({ summary }: StatCardsProps) {
  const s = summary;
  const pct = s ? Math.round(s.cacheHitRate * 100) : 0;

  return (
    <div className="stat-cards">
      <StatCard label="Input" value={s ? formatTokens(s.inputTokens) : '—'} />
      <StatCard label="Output" value={s ? formatTokens(s.outputTokens) : '—'} />
      <StatCard label="Cache write" value={s ? formatTokens(s.cacheWriteTokens) : '—'} />
      <StatCard label="Cache read" value={s ? formatTokens(s.cacheReadTokens) : '—'} />
      <div className="stat-card">
        <div className="stat-label">Cache hit rate</div>
        <div className="stat-value">{s ? `${pct}%` : '—'}</div>
        <div className="progress">
          <div className="progress-fill" style={{ width: `${pct}%` }} />
        </div>
      </div>
    </div>
  );
}
