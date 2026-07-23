import type { BreakdownRow, Settings, Summary } from '../types';
import type { Lang } from '../lib/i18n';
import { formatDisplayCost, overviewT, countLabel, USD_IDENTITY } from './localize';
import { TOOLS } from './meta';
import { isAllUnattributedCost, isPartialCost } from '../lib/costCompleteness';

type CostSettings = Pick<Settings, 'currency' | 'usdRate'>;

interface CostBreakdownModelData {
  name: string;
  cost: number | null;
  cacheEstimated: boolean;
  unattributed: boolean;
}

interface CostBreakdownGroupData {
  sourceKey: string;
  sourceName: string;
  models: CostBreakdownModelData[];
  cost: number | null;
  unpricedCount: number;
  unattributedTokens: number;
}

export interface CostBreakdownView {
  totalCostLabel: string;
  note: string | null;
  groups: Array<{
    sourceKey: string;
    sourceName: string;
    costLabel: string;
    models: Array<{
      name: string;
      costLabel: string;
      unpriced: boolean;
      unattributed: boolean;
      cacheEstimated: boolean;
    }>;
  }>;
}

function getSourceName(sourceKey: string): string {
  return TOOLS.find((tool) => tool.key === sourceKey)?.source ?? sourceKey;
}

function buildCostBreakdownGroups(rows: BreakdownRow[], lang: Lang): CostBreakdownGroupData[] {
  const groups = new Map<string, CostBreakdownGroupData>();

  for (const row of rows) {
    const sourceKey = row.source ?? 'Unknown Source';
    const group = groups.get(sourceKey) ?? {
      sourceKey,
      sourceName: getSourceName(sourceKey),
      models: [],
      cost: null,
      unpricedCount: 0,
      unattributedTokens: 0,
    };

    const unattributed = row.key === null;
    group.models.push({
      name: row.key ?? overviewT(lang, 'overview.unattributedUsage'),
      cost: row.cost,
      cacheEstimated: row.cacheEstimated,
      unattributed,
    });

    if (unattributed) {
      group.unattributedTokens += row.unattributedTokens;
    } else if (row.cost === null) {
      group.unpricedCount += 1;
    } else {
      group.cost = (group.cost ?? 0) + row.cost;
    }

    groups.set(sourceKey, group);
  }

  for (const group of groups.values()) {
    group.models.sort((a, b) => {
      if (a.cost === null && b.cost === null) return a.name.localeCompare(b.name);
      if (a.cost === null) return 1;
      if (b.cost === null) return -1;
      return b.cost - a.cost || a.name.localeCompare(b.name);
    });
  }

  return [...groups.values()].sort((a, b) => {
    if (a.cost === null && b.cost === null) return a.sourceName.localeCompare(b.sourceName);
    if (a.cost === null) return 1;
    if (b.cost === null) return -1;
    return b.cost - a.cost || a.sourceName.localeCompare(b.sourceName);
  });
}

export function formatBreakdownCost(
  cost: number | null,
  partial = false,
  settings: CostSettings = USD_IDENTITY,
  lang: Lang = 'en',
): string {
  return formatDisplayCost(cost, partial, settings, lang, {
    adaptivePrecision: true,
    unpricedLabel: overviewT(lang, 'overview.unpricedLabel'),
  });
}

export function formatSourceCost(
  cost: number | null,
  unpricedCount: number,
  unattributedTokens = 0,
  settings: CostSettings = USD_IDENTITY,
  lang: Lang = 'en',
): string {
  const unattributed = unattributedTokens > 0;
  if (cost === null) {
    const base = unpricedCount > 0
      ? overviewT(lang, 'overview.unpricedLabel')
      : unattributed
        ? overviewT(lang, 'overview.unavailableCost')
        : overviewT(lang, 'overview.unpricedLabel');
    return unattributed ? `${base} · ${overviewT(lang, 'overview.unattributedUsage')}` : base;
  }
  const bits: string[] = [];
  if (unpricedCount > 0) bits.push(`${unpricedCount} ${overviewT(lang, 'overview.unpricedMarker')}`);
  if (unattributed) bits.push(overviewT(lang, 'overview.unattributedUsage'));
  const base = formatBreakdownCost(cost, bits.length > 0, settings, lang);
  return bits.length ? `${base} · ${bits.join(' · ')}` : base;
}

export function buildCostBreakdownView(
  summary: Summary,
  rows: BreakdownRow[],
  settings: CostSettings = USD_IDENTITY,
  lang: Lang = 'en',
): CostBreakdownView {
  const groups = buildCostBreakdownGroups(rows, lang).map((group) => ({
    sourceKey: group.sourceKey,
    sourceName: group.sourceName,
    costLabel: formatSourceCost(
      group.cost, group.unpricedCount, group.unattributedTokens, settings, lang,
    ),
    models: group.models.map((model) => ({
      name: model.name,
      costLabel: model.unattributed
        ? overviewT(lang, 'overview.unavailableCost')
        : formatBreakdownCost(model.cost, false, settings, lang),
      unpriced: !model.unattributed && model.cost === null,
      unattributed: model.unattributed,
      cacheEstimated: model.cacheEstimated,
    })),
  }));

  const reasons: string[] = [];
  if (summary.hasUnpriced && summary.cost !== null) {
    reasons.push(countLabel(
      summary.unpricedModels.length,
      'overview.unpricedModelOne',
      'overview.unpricedModelMany',
      lang,
    ));
  }
  if (summary.unattributedTokens > 0) reasons.push(overviewT(lang, 'overview.unattributedUsage'));
  const note = reasons.length
    ? `${summary.cost !== null ? `${overviewT(lang, 'overview.partialCost')} · ` : ''}${reasons.join(' · ')}`
    : null;

  return {
    totalCostLabel: isAllUnattributedCost(summary)
      ? overviewT(lang, 'overview.unavailableCost')
      : formatBreakdownCost(summary.cost, isPartialCost(summary), settings, lang),
    note,
    groups,
  };
}
