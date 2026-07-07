import type { DateRange, CustomRange } from '../types';

function midnight(d: Date): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate());
}

function secs(d: Date): number {
  return Math.floor(d.getTime() / 1000);
}

function parseLocalDate(s: string): Date {
  const [y, m, d] = s.split('-').map(Number);
  return new Date(y, m - 1, d);
}

// 'today' = local midnight..now (end open); '7d'/'30d' = midnight (today-6/-29)..open;
// 'all' = both open; custom = midnight(start)..midnight(end + 1 day), end exclusive.
export function rangeToBounds(
  range: DateRange,
  now: Date = new Date(),
): { startTs?: number; endTs?: number } {
  if (typeof range === 'string') {
    switch (range) {
      case 'today':
        return { startTs: secs(midnight(now)) };
      case '7d': {
        const start = midnight(now);
        start.setDate(start.getDate() - 6);
        return { startTs: secs(start) };
      }
      case '30d': {
        const start = midnight(now);
        start.setDate(start.getDate() - 29);
        return { startTs: secs(start) };
      }
      case 'all':
        return {};
    }
  }
  const r = range as CustomRange;
  const start = parseLocalDate(r.start);
  const end = parseLocalDate(r.end);
  end.setDate(end.getDate() + 1); // inclusive end date -> exclusive next-day midnight
  return { startTs: secs(start), endTs: secs(end) };
}
