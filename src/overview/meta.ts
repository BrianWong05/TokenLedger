// Shared design meta for the Overview: the source catalog, token categories,
// heatmap themes, range presets, and month labels — plus the ToolKey/ToolMeta/
// Range8b types. Pure constants and types, no fetching or reshaping (that lives
// in data.ts), so components can pull in the meta without the reshapers.

export type ToolKey = 'claude' | 'codex' | 'gemini' | 'hermes' | 'grok' | 'antigravity' | 'pi';

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
  { key: 'pi', label: 'pi', source: 'pi', color: '#a3a3a3' },
];

// The four canonical token categories (CONTEXT.md).
export const CATEGORIES = [
  { key: 'input', label: 'Input', color: '#7c5cff' },
  { key: 'output', label: 'Output', color: '#2fbf71' },
  { key: 'cacheRead', label: 'Cache read', color: '#3aa0ff' },
  { key: 'cacheWrite', label: 'Cache write', color: '#f0a03c' },
] as const;

export type Range8b = 'day' | 'week' | 'month' | 'total' | 'custom';
export const RANGES_8B: { key: Range8b; label: string; long: string }[] = [
  { key: 'day', label: 'Day', long: 'Today' },
  { key: 'week', label: 'Week', long: 'Last 7 days' },
  { key: 'month', label: 'Month', long: 'Last 30 days' },
  { key: 'total', label: 'Total', long: 'All time' },
  { key: 'custom', label: 'Custom', long: 'Custom range' },
];

export function emptyByTool(): Record<ToolKey, number> {
  return { claude: 0, codex: 0, gemini: 0, hermes: 0, grok: 0, antigravity: 0, pi: 0 };
}
