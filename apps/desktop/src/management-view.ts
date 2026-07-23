import type { Route } from "./navigation.ts";
import { escapeHtml } from "./html.ts";

export type ManagementState = Partial<Record<Route, unknown>>;
type RecordValue = Record<string, unknown>;

function record(value: unknown): RecordValue { return value && typeof value === "object" && !Array.isArray(value) ? value as RecordValue : {}; }
function list(value: unknown): unknown[] { return Array.isArray(value) ? value : []; }
function label(value: unknown): string { return String(value ?? "unknown").replaceAll("_", " "); }
function empty(message: string): string { return `<p class="empty">${escapeHtml(message)}</p>`; }
function error(value: RecordValue): string | null { return typeof value.error === "string" ? `<p class="domain-error" role="alert">${escapeHtml(value.error)}</p>` : null; }

const riskRank = new Map([
  ["read_only", 0],
  ["low", 1],
  ["medium", 2],
  ["high", 3],
  ["destructive", 4],
]);

function planRisk(plan: RecordValue, operations: unknown[]): string {
  if (typeof plan.risk === "string") return plan.risk;
  return operations.reduce<string>((highest, item) => {
    const candidate = String(record(item).risk ?? "read_only");
    return (riskRank.get(candidate) ?? 0) > (riskRank.get(highest) ?? 0) ? candidate : highest;
  }, "read_only");
}

function operationResource(operation: RecordValue): string {
  if (typeof operation.path === "string") return operation.path;
  if (typeof operation.resource === "string") return operation.resource;
  const resource = record(operation.resource);
  return String(
    resource.managedPath
      ?? resource.resourceId
      ?? operation.id
      ?? "Managed resource",
  );
}

function operationSource(operation: RecordValue): string {
  const provenance = record(operation.provenance);
  return String(
    provenance.layer
      ?? provenance.source
      ?? operation.source
      ?? operation.adapterId
      ?? "composed loadout",
  );
}

function plansPanel(value: RecordValue): string {
  const plan = record(value.plan ?? value.currentPlan ?? value);
  const operations = list(plan.operations);
  const risk = planRisk(plan, operations);
  const operation = record(value.operation ?? value.applyOperation);
  const progress = operation.totalOperations
    ? `<p class="progress"><strong>${escapeHtml(operation.state ?? "Applying")}</strong> · ${escapeHtml(operation.completedOperations ?? 0)} of ${escapeHtml(operation.totalOperations)} operations</p>` : "";
  const rows = operations.map((item) => {
    const op = record(item);
    const summary = typeof op.summary === "string" ? op.summary : label(op.kind);
    return `<li><strong>${escapeHtml(summary)}</strong><span>${escapeHtml(operationResource(op))}</span><small>${escapeHtml(label(op.risk))} risk · From ${escapeHtml(operationSource(op))}</small></li>`;
  }).join("");
  return `${progress}<div class="summary-card"><span class="risk risk-${escapeHtml(risk)}">${escapeHtml(label(risk))} risk</span><h2>${operations.length} planned operation${operations.length === 1 ? "" : "s"}</h2><p>Plan ${escapeHtml(plan.id ?? "not yet created")}</p></div>${rows ? `<ol class="inventory">${rows}</ol>` : empty("Create a plan to review provenance and risk.")}<label>Reviewed plan<select name="plan-id">${plan.id ? `<option value="${escapeHtml(plan.id)}">${escapeHtml(plan.id)}</option>` : ""}</select></label>`;
}

function snapshotsPanel(value: RecordValue): string {
  const snapshots = list(value.snapshots);
  const writer = record(value.authoritativeWriter ?? value.writer);
  const rows = snapshots.map((item) => { const snap = record(item); return `<li><strong>${escapeHtml(snap.databaseId ?? "Database")}</strong><span>${escapeHtml(snap.createdAt ?? snap.createdAtUnixMs ?? "Time unavailable")}</span><small>${escapeHtml(snap.id)}</small></li>`; }).join("");
  const options = snapshots.map((item) => { const snap = record(item); return `<option value="${escapeHtml(snap.id)}">${escapeHtml(snap.databaseId ?? snap.id)} · ${escapeHtml(snap.createdAt ?? "snapshot")}</option>`; }).join("");
  return `<div class="summary-card"><h2>Authoritative writer</h2><p>${escapeHtml(writer.databaseId ?? "No database selected")} · ${escapeHtml(writer.targetId ?? "No writer recorded")}</p></div>${rows ? `<ul class="inventory">${rows}</ul>` : empty("No snapshots are available.")}<div class="control-grid"><label>Database<input name="database-id" value="${escapeHtml(writer.databaseId)}"></label><label>Snapshot to restore<select name="snapshot-id">${options}</select></label><label>Promotion target<input name="promotion-target" value="${escapeHtml(writer.targetId)}"></label></div>`;
}

function relayPanel(value: RecordValue): string {
  const upstreams = list(value.upstreams ?? value.servers);
  const rows = upstreams.map((item) => { const upstream = record(item); return `<li><strong>${escapeHtml(upstream.name ?? upstream.id)}</strong><span class="state">${escapeHtml(upstream.state ?? upstream.health)}</span><small>${escapeHtml(upstream.transport ?? upstream.command ?? "Managed transport")}</small></li>`; }).join("");
  return `<div class="summary-card"><h2>Relay ${escapeHtml(value.state ?? "unavailable")}</h2><p>${upstreams.length} configured upstream${upstreams.length === 1 ? "" : "s"}</p></div>${rows ? `<ul class="inventory">${rows}</ul>` : empty("No relay upstreams are configured.")}`;
}

function diagnosticsPanel(value: RecordValue): string {
  const adapters = list(value.adapters ?? value.components ?? value.checks);
  const rows = adapters.map((item) => { const adapter = record(item); return `<li><strong>${escapeHtml(adapter.name ?? adapter.id ?? adapter.component)}</strong><span class="state">${escapeHtml(adapter.state ?? adapter.status)}</span><small>${escapeHtml(adapter.detail ?? adapter.message ?? "No diagnostic detail")}</small></li>`; }).join("");
  return rows ? `<ul class="inventory">${rows}</ul>` : empty("No adapter diagnostics were reported.");
}

function credentialsPanel(value: RecordValue): string {
  const credentials = list(value.credentials ?? value.references);
  const rows = credentials.map((item) => { const credential = record(item); return `<li><strong>${escapeHtml(credential.reference ?? credential.id)}</strong><span class="state">${escapeHtml(credential.readiness ?? credential.state ?? "unknown")}</span><small>Reference only · secret value hidden</small></li>`; }).join("");
  return rows ? `<ul class="inventory">${rows}</ul>` : empty("No credential references were reported.");
}

function genericPanel(value: RecordValue): string {
  const entries = Object.entries(value).filter(([key]) => key !== "error");
  return entries.length ? `<dl>${entries.map(([key, item]) => `<dt>${escapeHtml(label(key))}</dt><dd>${escapeHtml(Array.isArray(item) ? `${item.length} item(s)` : typeof item === "object" ? "Configured" : item)}</dd>`).join("")}</dl>` : empty("No domain state was reported.");
}

export function managementPanel(route: Route, state: ManagementState): string {
  const value = record(state[route] ?? { error: `${route}_unavailable` });
  const detail = error(value) ?? ({ plans: plansPanel, credentials: credentialsPanel, snapshots: snapshotsPanel, relay: relayPanel, diagnostics: diagnosticsPanel } as Partial<Record<Route, (v: RecordValue) => string>>)[route]?.(value) ?? genericPanel(value);
  return `<section class="panel management-panel"><p class="eyebrow">${escapeHtml(route)}</p><h1>${escapeHtml(label(route))}</h1>${detail}${operatorActions(route)}</section>`;
}

function operatorActions(route: Route): string {
  switch (route) {
    case "plans": return '<div class="actions"><button id="plan-sync">Plan selected target</button><button id="verify-state">Verify selected target</button><button id="apply-plan">Apply reviewed plan</button></div>';
    case "snapshots": return '<div class="actions"><button id="snapshot-create">Create snapshot</button><button id="snapshot-restore">Restore snapshot</button><button id="snapshot-promote">Promote writer</button></div>';
    case "relay": return '<div class="actions"><button id="relay-reconcile">Reconcile configured relay</button><button id="relay-restart">Restart relay</button></div>';
    case "schedule": return '<div class="actions"><button id="schedule-enable">Enable drift checks</button><button id="schedule-disable">Disable drift checks</button></div>';
    case "credentials": return '<div class="actions"><button id="credential-readiness">Check readiness</button><button id="credential-apply">Apply credential</button><button id="credential-verify">Verify credential</button></div>';
    case "diagnostics": return '<div class="actions"><button id="diagnostics-export">Export redacted diagnostics</button></div>';
    default: return "";
  }
}
