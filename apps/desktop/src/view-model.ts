import type { ServiceStatus } from "./contracts.ts";
import type { Route } from "./navigation.ts";

export interface StatusView { heading: string; detail: string; canMutate: boolean; primaryRoute: Route; }

export function statusView(status: ServiceStatus): StatusView {
  switch (status.state) {
    case "healthy": return { heading: "Environment healthy", detail: "This target matches its loadout.", canMutate: true, primaryRoute: "status" };
    case "drifted": return { heading: "Changes available", detail: "Review a content-bound plan before applying changes.", canMutate: true, primaryRoute: "plans" };
    case "blocked": return { heading: "Action blocked", detail: status.lastDriftErrorCode ?? "A policy or readiness check needs attention.", canMutate: false, primaryRoute: "diagnostics" };
    case "applying": return { heading: "Applying plan", detail: "CommonKit is verifying each managed operation.", canMutate: false, primaryRoute: "plans" };
    case "degraded": return { heading: "Needs attention", detail: status.lastDriftErrorCode ?? "One or more capabilities are degraded.", canMutate: false, primaryRoute: "diagnostics" };
    case "offline": return { heading: "Service offline", detail: "Start CommonKit’s local service to inspect this machine.", canMutate: false, primaryRoute: "diagnostics" };
  }
}
