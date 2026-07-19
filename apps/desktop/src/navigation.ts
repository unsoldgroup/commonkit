export const navigation = [
  { route: "onboarding", label: "Get started" }, { route: "status", label: "Status" },
  { route: "plans", label: "Plan review" }, { route: "credentials", label: "Credentials" },
  { route: "snapshots", label: "Snapshots" }, { route: "relay", label: "Relay" },
  { route: "schedule", label: "Schedule" }, { route: "diagnostics", label: "Diagnostics" },
  { route: "settings", label: "Settings" },
] as const;

export type Route = typeof navigation[number]["route"];
const routes = new Set<string>(navigation.map(({ route }) => route));

export function navigationForSetup(complete: boolean): typeof navigation | readonly [typeof navigation[0]] {
  return complete ? navigation : [navigation[0]];
}

export function routeForSetup(route: Route, complete: boolean): Route {
  return complete || route === "onboarding" ? route : "onboarding";
}

export function routeFromHash(hash: string): Route {
  const candidate = hash.replace(/^#\/?/, "");
  return routes.has(candidate) ? candidate as Route : "status";
}
