// Shared design meta for the Overview: the source catalog, token categories,
// heatmap themes, range presets, and month labels — plus the ToolKey/ToolMeta/
// Range8b types. Pure constants and types, no fetching or reshaping (that lives
// in data.ts), so components can pull in the meta without the reshapers.

import catalog from '../source-catalog.json';

export type ToolKey = string;

export interface ToolMeta {
  key: ToolKey;
  label: string;
  source: string; // full source name, e.g. "Claude Code"
  color: string;
  icon: string;
  aliases: string[];
  capabilities: Record<string, boolean>;
}

// The frontend reads the same declarative facts as Rust. This is the one
// ordered display list for catalogued Sources; Ledger history may contain keys
// introduced by a newer catalog, which sourceMeta keeps visible below.
export const TOOLS: ToolMeta[] = catalog.sources.map((source) => ({
  key: source.key,
  label: source.label,
  source: source.source,
  color: source.color,
  icon: source.icon,
  aliases: source.aliases,
  capabilities: source.capabilities,
}));

const KNOWN_TOOLS = new Map(TOOLS.map((tool) => [tool.key, tool]));
const FALLBACK_COLOR = '#5f6880';

export function sourceMeta(key: string): ToolMeta {
  return KNOWN_TOOLS.get(key) ?? {
    key,
    label: key,
    source: key,
    color: FALLBACK_COLOR,
    icon: 'generic',
    aliases: [],
    capabilities: {},
  };
}

// Known Sources always use catalog order. Historical or newer keys retain
// their first-seen order after the known entries instead of being discarded.
export function orderedSourceKeys(keys: Iterable<string>): ToolKey[] {
  const seen = new Set(keys);
  return [
    ...TOOLS.filter((tool) => seen.delete(tool.key)).map((tool) => tool.key),
    ...seen,
  ];
}

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

export function emptyByTool(): Record<string, number> {
  return Object.fromEntries(TOOLS.map((tool) => [tool.key, 0]));
}
