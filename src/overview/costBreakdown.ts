import type { BreakdownRow, Summary } from '../types';
import { formatCost } from '../lib/format';
import { TOOLS } from './meta';

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

export function formatBreakdownCost(cost: number | null, partial = false): string {
  return formatCost(cost, partial, { adaptivePrecision: true, unpricedLabel: 'Unpriced' });
}

export function formatSourceCost(cost: number | null, unpricedCount: number): string {
  if (cost === null) return 'Unpriced';
  if (unpricedCount === 0) return formatBreakdownCost(cost);
  return `${formatBreakdownCost(cost, true)} · ${unpricedCount} unpriced`;
}

export function buildCostBreakdownView(summary: Summary, rows: BreakdownRow[]): CostBreakdownView {
  const groups = buildCostBreakdownGroups(rows).map((group) => ({
    sourceKey: group.sourceKey,
    sourceName: group.sourceName,
    costLabel: formatSourceCost(group.cost, group.unpricedCount),
    models: group.models.map((model) => ({
      name: model.name,
      costLabel: formatBreakdownCost(model.cost),
      unpriced: model.cost === null,
      cacheEstimated: model.cacheEstimated,
    })),
  }));

  return {
    totalCostLabel: formatBreakdownCost(
      summary.cost,
      summary.hasUnpriced && summary.cost !== null,
    ),
    note:
      summary.hasUnpriced && summary.cost !== null
        ? `Partial Cost · ${summary.unpricedModels.length} Unpriced Model${summary.unpricedModels.length === 1 ? '' : 's'}`
        : null,
    groups,
  };
}
