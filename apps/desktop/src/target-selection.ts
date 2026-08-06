import type { TargetInventorySnapshot } from "./contracts.ts";

export function convergenceTarget(inventory: TargetInventorySnapshot | null): string {
  if (!inventory || inventory.selected.length === 0) throw new Error("target_selection_required");
  if (inventory.selected.length !== 1) throw new Error("single_target_required");
  const selected = inventory.selected[0];
  if (!inventory.targets.some(({ id }) => id === selected)) throw new Error("stale_target_selection");
  return selected;
}

export function assertPlanTarget(plan: unknown, selectedTarget: string): void {
  if (
    typeof plan !== "object"
    || plan === null
    || !("targetId" in plan)
    || (plan as { targetId?: unknown }).targetId !== selectedTarget
  ) {
    throw new Error("target_plan_mismatch");
  }
}
