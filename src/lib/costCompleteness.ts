export interface CostCompleteness {
  cost: number | null;
  hasUnpriced: boolean;
  unattributedTokens: number;
}

export function isPartialCost(value: CostCompleteness): boolean {
  return value.cost !== null && (value.hasUnpriced || value.unattributedTokens > 0);
}

export function isAllUnattributedCost(value: CostCompleteness): boolean {
  return value.cost === null && !value.hasUnpriced && value.unattributedTokens > 0;
}
