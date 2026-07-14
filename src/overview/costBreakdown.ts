import type { BreakdownRow } from '../types';
import { TOOLS } from './data';

export interface CostBreakdownModel {
  name: string;
  cost: number | null;
  cacheEstimated: boolean;
}

export interface CostBreakdownGroup {
  sourceKey: string;
  sourceName: string;
  models: CostBreakdownModel[];
  cost: number | null;
  unpricedCount: number;
}

function getSourceName(sourceKey: string): string {
  return TOOLS.find((tool) => tool.key === sourceKey)?.source ?? sourceKey;
}

export function buildCostBreakdown(rows: BreakdownRow[]): CostBreakdownGroup[] {
  const groups = new Map<string, CostBreakdownGroup>();

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

function formatAmount(cost: number): string {
  const absoluteCost = Math.abs(cost);
  const fractionDigits =
    cost === 0 || absoluteCost >= 0.01
      ? 2
      : Math.min(8, Math.max(4, Math.ceil(-Math.log10(absoluteCost)) + 1));
  const rounded = cost.toFixed(fractionDigits);

  if (cost !== 0 && Number(rounded) === 0) return `$${cost.toExponential(2)}`;

  return new Intl.NumberFormat('en-US', {
    style: 'currency',
    currency: 'USD',
    minimumFractionDigits: fractionDigits,
    maximumFractionDigits: fractionDigits,
  }).format(cost);
}

export function formatBreakdownCost(cost: number | null, partial = false): string {
  if (cost === null) return 'Unpriced';
  return `${partial ? '≥ ' : ''}${formatAmount(cost)}`;
}

export function formatSourceCost(cost: number | null, unpricedCount: number): string {
  if (cost === null) return 'Unpriced';
  if (unpricedCount === 0) return formatBreakdownCost(cost);
  return `${formatBreakdownCost(cost, true)} · ${unpricedCount} unpriced`;
}
