// 'YYYY-MM-DD' -> local-midnight Date. The one parser for ISO date strings —
// new Date('YYYY-MM-DD') would parse as UTC and drift a day in some timezones.
export function parseLocalDate(s: string): Date {
  const [y, m, d] = s.split('-').map(Number);
  return new Date(y, m - 1, d);
}
