export type SetupCompletion = "checking" | "required" | "complete";

export function setupCompletion(
  status: { activeTarget: string | null; activeLoadout: string | null } | null,
  inventory: { targets: Array<{ id: string }> } | null,
): SetupCompletion {
  if (!status || !inventory) return "checking";
  const activeTarget = status.activeTarget;
  if (
    activeTarget
    && status.activeLoadout
    && inventory.targets.some((target) => target.id === activeTarget)
  ) return "complete";
  return "required";
}
