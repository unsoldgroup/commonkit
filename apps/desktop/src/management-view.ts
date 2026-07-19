import type { Route } from "./navigation.ts";

export type ManagementState = Partial<Record<Route, unknown>>;

function escapeHtml(value: string): string {
  return value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;");
}

export function managementPanel(route: Route, state: ManagementState): string {
  const value = state[route] ?? { error: `${route}_unavailable` };
  const detail = escapeHtml(JSON.stringify(value, null, 2));
  return `<section class="panel"><p class="eyebrow">${escapeHtml(route)}</p><h1>${escapeHtml(route)}</h1><pre>${detail}</pre>${operatorActions(route)}</section>`;
}

function operatorActions(route: Route): string {
  switch (route) {
    case "plans":
      return '<div class="actions"><button id="plan-sync">Plan selected target</button><button id="verify-state">Verify selected target</button><button id="apply-plan">Apply reviewed plan</button></div>';
    case "snapshots":
      return '<div class="actions"><button id="snapshot-create">Create snapshot</button><button id="snapshot-restore">Restore snapshot</button><button id="snapshot-promote">Promote writer</button></div>';
    case "relay":
      return '<div class="actions"><button id="relay-reconcile">Reconcile relay</button><button id="relay-restart">Restart relay</button></div>';
    case "schedule":
      return '<div class="actions"><button id="schedule-enable">Enable drift checks</button><button id="schedule-disable">Disable drift checks</button></div>';
    case "credentials":
      return '<div class="actions"><button id="credential-readiness">Check readiness</button><button id="credential-apply">Apply credential</button><button id="credential-verify">Verify credential</button></div>';
    case "diagnostics":
      return '<div class="actions"><button id="diagnostics-export">Export redacted diagnostics</button></div>';
    default:
      return "";
  }
}
