import type { SetupCompletion } from "./setup-gate.ts";

export interface DesktopState<S = unknown, M = unknown, T = unknown> {
  snapshot: S;
  management: M;
  targets: T;
}

export interface RefreshApi<S = unknown, M = unknown, T = unknown> {
  snapshot(): Promise<S>;
  managementSnapshot(): Promise<M>;
  targets(): Promise<T>;
}

export function shouldRunLiveRefresh(
  setupUnlocked: boolean,
  completion: SetupCompletion,
): boolean {
  return setupUnlocked || completion !== "required";
}

export async function refreshDesktopState<S, M, T>(
  api: RefreshApi<S, M, T>,
  previous?: Partial<DesktopState<S, M, T>>,
): Promise<DesktopState<S, M, T>> {
  const [snapshot, management, targets] = await Promise.allSettled([
    api.snapshot(), api.managementSnapshot(), api.targets(),
  ]);
  const retain = <V>(result: PromiseSettledResult<V>, prior: V | undefined): V => {
    if (result.status === "fulfilled") return result.value;
    if (prior !== undefined) return prior;
    throw result.reason;
  };
  return {
    snapshot: retain(snapshot, previous?.snapshot),
    management: retain(management, previous?.management),
    targets: retain(targets, previous?.targets),
  };
}
