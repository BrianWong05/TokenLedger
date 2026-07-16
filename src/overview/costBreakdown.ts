import type { BreakdownRow, Settings, Summary } from '../types';
import type { Lang } from '../lib/i18n';
import { formatDisplayCost, overviewT, countLabel, USD_IDENTITY } from './localize';
import { TOOLS } from './meta';

type CostSettings = Pick<Settings, 'currency' | 'usdRate'>;

interface CostBreakdownModelData {
  name: string;
  cost: number | null;
  cacheEstimated: boolean;
}

interface CostBreakdownGroupData {
  sourceKey: string;
  sourceName: string;
  models: CostBreakdownModelData[];
  cost: number | null;
  unpricedCount: number;
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
      cacheEstimated: boolean;
    }>;
  }>;
}

function getSourceName(sourceKey: string): string {
  return TOOLS.find((tool) => tool.key === sourceKey)?.source ?? sourceKey;
}

function buildCostBreakdownGroups(rows: BreakdownRow[]): CostBreakdownGroupData[] {
  const groups = new Map<string, CostBreakdownGroupData>();

  for (const row of rows) {
    const sourceKey = row.source ?? 'Unknown Source';
    const group = groups.get(sourceKey) ?? {
      sourceKey,
      sourceName: getSourceName(sourceKey),
      models: [],
      cost: null,
      unpricedCount: 0,
    };

    group.models.push({
      name: row.key,
      cost: row.cost,
      cacheEstimated: row.cacheEstimated,
    });

    if (row.cost === null) {
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
  settings: CostSettings = USD_IDENTITY,
  lang: Lang = 'en',
): string {
  if (cost === null) return overviewT(lang, 'overview.unpricedLabel');
  if (unpricedCount === 0) return formatBreakdownCost(cost, false, settings, lang);
  return `${formatBreakdownCost(cost, true, settings, lang)} · ${unpricedCount} ${overviewT(lang, 'overview.unpricedMarker')}`;
}

export function buildCostBreakdownView(
  summary: Summary,
  rows: BreakdownRow[],
  settings: CostSettings = USD_IDENTITY,
  lang: Lang = 'en',
): CostBreakdownView {
  const groups = buildCostBreakdownGroups(rows).map((group) => ({
    sourceKey: group.sourceKey,
    sourceName: group.sourceName,
    costLabel: formatSourceCost(group.cost, group.unpricedCount, settings, lang),
    models: group.models.map((model) => ({
      name: model.name,
      costLabel: formatBreakdownCost(model.cost, false, settings, lang),
      unpriced: model.cost === null,
      cacheEstimated: model.cacheEstimated,
    })),
  }));

  return {
    totalCostLabel: formatBreakdownCost(
      summary.cost,
      summary.hasUnpriced && summary.cost !== null,
      settings,
      lang,
    ),
    note:
      summary.hasUnpriced && summary.cost !== null
        ? `${overviewT(lang, 'overview.partialCost')} · ${countLabel(summary.unpricedModels.length, 'overview.unpricedModelOne', 'overview.unpricedModelMany', lang)}`
        : null,
    groups,
  };
}
