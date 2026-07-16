// Shared design meta for the Overview: the source catalog, token categories,
// heatmap themes, range presets, and month labels — plus the ToolKey/ToolMeta/
// Range8b types. Pure constants and types, no fetching or reshaping (that lives
// in data.ts), so components can pull in the meta without the reshapers.

export type ToolKey = 'claude' | 'codex' | 'gemini' | 'hermes' | 'grok' | 'antigravity';

export interface ToolMeta {
  key: ToolKey;
  label: string;
  source: string; // full source name, e.g. "Claude Code"
  color: string;
}

// color = each source's brand accent, matching its icon (src/overview/icons/).
// Hermes has no brand mark, so it keeps a distinct pink.
export const TOOLS: ToolMeta[] = [
  { key: 'claude', label: 'Claude', source: 'Claude Code', color: '#d97757' },
  { key: 'codex', label: 'Codex', source: 'Codex', color: '#6e50f2' },
  { key: 'gemini', label: 'Gemini', source: 'Gemini CLI', color: '#3186ff' },
  { key: 'hermes', label: 'Hermes', source: 'Hermes', color: '#f472b6' },
  { key: 'grok', label: 'Grok', source: 'Grok Build', color: '#c3c8d2' },
  { key: 'antigravity', label: 'Antigravity', source: 'Google Antigravity', color: '#22d3ee' },
];

// The four canonical token categories (CONTEXT.md).
export const CATEGORIES = [
  { key: 'input', label: 'Input', color: '#7c5cff' },
  { key: 'output', label: 'Output', color: '#2fbf71' },
  { key: 'cacheRead', label: 'Cache read', color: '#3aa0ff' },
  { key: 'cacheWrite', label: 'Cache write', color: '#f0a03c' },
] as const;

// Heatmap ramps: index 0 = empty cell, 1..4 = ascending intensity.
export const THEMES: Record<string, string[]> = {
  ocean: ['#12161f', '#173a63', '#1f5aa6', '#2f80ed', '#63a4ff'],
  emerald: ['#12161f', '#14503a', '#1a7d55', '#25a56f', '#4ad991'],
  neon: ['#12161f', '#312a63', '#4b3aa6', '#6d4fed', '#9a7cff'],
  amber: ['#12161f', '#5a4114', '#8a6417', '#c98f25', '#f0b84a'],
};
export const THEME_OPTIONS = [
  { value: 'ocean', label: 'Blue' },
  { value: 'emerald', label: 'Green' },
  { value: 'neon', label: 'Violet' },
  { value: 'amber', label: 'Amber' },
];

export type Range8b = 'day' | 'week' | 'month' | 'total' | 'custom';
export const RANGES_8B: { key: Range8b; label: string; long: string }[] = [
  { key: 'day', label: 'Day', long: 'Today' },
  { key: 'week', label: 'Week', long: 'Last 7 days' },
  { key: 'month', label: 'Month', long: 'Last 30 days' },
  { key: 'total', label: 'Total', long: 'All time' },
  { key: 'custom', label: 'Custom', long: 'Custom range' },
];

export function emptyByTool(): Record<ToolKey, number> {
  return { claude: 0, codex: 0, gemini: 0, hermes: 0, grok: 0, antigravity: 0 };
}
