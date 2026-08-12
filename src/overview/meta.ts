// Shared design meta for the Overview: the source catalog, token categories,
// heatmap themes, range presets, and month labels — plus the SourceKey/SourceMeta/
// Range8b types. Pure constants and types, no fetching or reshaping (that lives
// in data.ts), so components can pull in the meta without the reshapers.

import catalog from '../source-catalog.json';

export type SourceKey = string;

export interface SourceArtifact {
  id: string;
  kind: string;
  path: string | null;
  environment: string | null;
  suffix: string | null;
  platforms: string[];
  prerequisite: string | null;
}

export interface SourceMeta {
  key: SourceKey;
  label: string;
  source: string; // full source name, e.g. "Claude Code"
  color: string;
  icon: string;
  aliases: string[];
  // Mostly booleans, but `limits` is the acquisition enum ("logs" | "live"),
  // absent on a Source with no vendor window to show.
  capabilities: Record<string, boolean | string | undefined>;
  artifacts: SourceArtifact[];
  platforms: string[];
  prerequisite: string | null;
}

// The frontend reads the same declarative facts as Rust. This is the one
// ordered display list for catalogued Sources; Ledger history may contain keys
// introduced by a newer catalog, which sourceMeta keeps visible below.
export const SOURCES: SourceMeta[] = catalog.sources.map((source) => ({
  key: source.key,
  label: source.label,
  source: source.source,
  color: source.color,
  icon: source.icon,
  aliases: source.aliases,
  capabilities: source.capabilities,
  artifacts: source.artifacts,
  platforms: source.platforms,
  prerequisite: source.prerequisite,
}));

const KNOWN_SOURCES = new Map(SOURCES.map((source) => [source.key, source]));
const FALLBACK_COLOR = '#5f6880';

export function sourceMeta(key: string): SourceMeta {
  return KNOWN_SOURCES.get(key) ?? {
    key,
    label: key,
    source: key,
    color: FALLBACK_COLOR,
    icon: 'generic',
    aliases: [],
    capabilities: {},
    artifacts: [],
    platforms: [],
    prerequisite: null,
  };
}

// Known Sources always use catalog order. Historical or newer keys retain
// their first-seen order after the known entries instead of being discarded.
export function orderedSourceKeys(keys: Iterable<string>): SourceKey[] {
  const seen = new Set(keys);
  return [
    ...SOURCES.filter((source) => seen.delete(source.key)).map((source) => source.key),
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

export function emptyBySource(): Record<string, number> {
  return Object.fromEntries(SOURCES.map((source) => [source.key, 0]));
}
