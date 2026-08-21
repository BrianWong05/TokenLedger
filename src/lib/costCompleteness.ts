export interface CostCompleteness {
  cost: number | null;
  hasUnpriced: boolean;
  unattributedTokens: number;
}

// The Menu Bar Extra's bar title applies the same rule natively
// (src-tauri/src/readout.rs is_partial_cost); readout-cases.json's
// partialCosts rows pin the two.
export function isPartialCost(value: CostCompleteness): boolean {
  return value.cost !== null && (value.hasUnpriced || value.unattributedTokens > 0);
}

export function isAllUnattributedCost(value: CostCompleteness): boolean {
  return value.cost === null && !value.hasUnpriced && value.unattributedTokens > 0;
}
