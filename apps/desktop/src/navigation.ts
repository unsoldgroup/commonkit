export const navigation = [
  { group: "Overview", route: "status", label: "Status" },
  { group: "Operate", route: "plans", label: "Changes" },
  { group: "Operate", route: "credentials", label: "Credentials" },
  { group: "Operate", route: "snapshots", label: "Data" },
  { group: "Operate", route: "aboutMe", label: "About Me" },
  { group: "Operate", route: "relay", label: "MCP connections" },
  { group: "Operate", route: "schedule", label: "Drift checks" },
  { group: "System", route: "diagnostics", label: "Diagnostics" },
  { group: "System", route: "settings", label: "Settings" },
] as const;

const onboardingNavigation = [{ group: "Setup", route: "onboarding", label: "Get started" }] as const;

export type Route = "onboarding" | typeof navigation[number]["route"];
export type NavigationItem = { readonly group: string; readonly route: Route; readonly label: string };
const routes = new Set<string>(["onboarding", ...navigation.map(({ route }) => route)]);

export function navigationForSetup(complete: boolean): readonly NavigationItem[] {
  return complete ? navigation : onboardingNavigation;
}

export function routeForSetup(route: Route, complete: boolean): Route {
  if (!complete) return "onboarding";
  return route === "onboarding" ? "settings" : route;
}

export function routeFromHash(hash: string): Route {
  const candidate = hash.replace(/^#\/?/, "");
  return routes.has(candidate) ? candidate as Route : "status";
}
