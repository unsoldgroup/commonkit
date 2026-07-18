import type { Route } from "./navigation.ts";

export type ManagementState = Partial<Record<Route, unknown>>;

function escapeHtml(value: string): string {
  return value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;");
}

export function managementPanel(route: Route, state: ManagementState): string {
  const value = state[route] ?? { error: `${route}_unavailable` };
  const detail = escapeHtml(JSON.stringify(value, null, 2));
  return `<section class="panel"><p class="eyebrow">${escapeHtml(route)}</p><h1>${escapeHtml(route)}</h1><pre>${detail}</pre></section>`;
}
