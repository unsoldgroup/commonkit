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
  if (typeof plan.id !== "string") {
    return `<div class="readiness readiness-ready"><span>Ready</span><h2>Ready to inspect</h2><p>Generate a content-bound plan to see exactly what CommonKit would change. Nothing is applied during planning.</p></div>`;
  }
  const risk = planRisk(plan, operations);
  const operation = record(value.operation ?? value.applyOperation);
  const progress = operation.totalOperations
    ? `<p class="progress"><strong>${escapeHtml(operation.state ?? "Applying")}</strong> · ${escapeHtml(operation.completedOperations ?? 0)} of ${escapeHtml(operation.totalOperations)} operations</p>` : "";
  const rows = operations.map((item) => {
    const op = record(item);
    const summary = typeof op.summary === "string" ? op.summary : label(op.kind);
    return `<li><strong>${escapeHtml(summary)}</strong><span>${escapeHtml(operationResource(op))}</span><small>${escapeHtml(label(op.risk))} risk · From ${escapeHtml(operationSource(op))}</small></li>`;
  }).join("");
  return `${progress}<div class="summary-card"><span class="risk risk-${escapeHtml(risk)}">${escapeHtml(label(risk))} risk</span><h2>${operations.length === 0 ? "Up to date" : `${operations.length} planned operation${operations.length === 1 ? "" : "s"}`}</h2><p>Plan ${escapeHtml(plan.id)}</p></div>${rows ? `<ol class="inventory">${rows}</ol>` : empty("This computer already matches the reviewed loadout.")}<label>Reviewed plan<select name="plan-id"><option value="${escapeHtml(plan.id)}">${escapeHtml(plan.id)}</option></select></label>`;
}

function snapshotsPanel(value: RecordValue): string {
  if (typeof value.error === "string") {
    return `<div class="readiness readiness-setup"><span>Setup needed</span><h2>Protect a database before using snapshots</h2><p>Choose which mutable database CommonKit should back up and which computer is allowed to write to it.</p><a class="primary" href="#settings">Configure database</a></div>`;
  }
  const snapshots = list(value.snapshots);
  const writer = record(value.authoritativeWriter ?? value.writer);
  const rows = snapshots.map((item) => { const snap = record(item); return `<li><strong>${escapeHtml(snap.databaseId ?? "Database")}</strong><span>${escapeHtml(snap.createdAt ?? snap.createdAtUnixMs ?? "Time unavailable")}</span><small>${escapeHtml(snap.id)}</small></li>`; }).join("");
  const options = snapshots.map((item) => { const snap = record(item); return `<option value="${escapeHtml(snap.id)}">${escapeHtml(snap.databaseId ?? snap.id)} · ${escapeHtml(snap.createdAt ?? "snapshot")}</option>`; }).join("");
  return `<div class="summary-card"><h2>Authoritative writer</h2><p>${escapeHtml(writer.databaseId ?? "No database selected")} · ${escapeHtml(writer.targetId ?? "No writer recorded")}</p></div>${rows ? `<ul class="inventory">${rows}</ul>` : empty("No snapshots are available.")}<div class="control-grid"><label>Database<input name="database-id" value="${escapeHtml(writer.databaseId)}"></label><label>Snapshot to restore<select name="snapshot-id">${options}</select></label><label>Promotion target<input name="promotion-target" value="${escapeHtml(writer.targetId)}"></label></div>`;
}

function relayPanel(value: RecordValue): string {
  const upstreams = list(value.upstreams ?? value.servers);
  if (upstreams.length === 0) {
    return `<div class="readiness readiness-setup"><span>Setup needed</span><h2>Add an MCP connection</h2><p>Portable declarations describe the service. CommonKit keeps its authenticated relay running at a stable local endpoint.</p><a class="primary" href="#settings">Add connection</a></div>`;
  }
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
  if (credentials.length === 0) {
    return `<div class="readiness readiness-setup"><span>Setup needed</span><h2>Connect a credential reference</h2><p>CommonKit stores only a reference to your secret provider. Secret values never enter the portable kit.</p><label>Credential reference<input name="credential-reference" placeholder="bws://secret-id" autocomplete="off"></label><button class="primary" id="credential-readiness" type="button">Check reference</button></div>`;
  }
  const rows = credentials.map((item) => { const credential = record(item); return `<li><strong>${escapeHtml(credential.reference ?? credential.id)}</strong><span class="state">${escapeHtml(credential.readiness ?? credential.state ?? "unknown")}</span><small>Reference only · secret value hidden</small></li>`; }).join("");
  const destinations = credentials.map((item) => {
    const credential = record(item);
    const id = credential.destinationId ?? credential.id;
    return id ? `<option value="${escapeHtml(id)}">${escapeHtml(id)}</option>` : "";
  }).join("");
  return `${rows ? `<ul class="inventory">${rows}</ul>` : empty("No credential references were reported.")}<label>Credential destination<select name="credential-destination">${destinations}</select></label>`;
}

function schedulePanel(value: RecordValue): string {
  const enabled = value.enabled === true;
  const interval = typeof value.intervalSeconds === "number" ? value.intervalSeconds : 900;
  const human = interval % 3600 === 0
    ? `${interval / 3600} hour${interval === 3600 ? "" : "s"}`
    : interval % 60 === 0
      ? `${interval / 60} minute${interval === 60 ? "" : "s"}`
      : `${interval} seconds`;
  return `<div class="readiness readiness-ready"><span>${enabled ? "Ready" : "Paused"}</span><h2>${enabled ? `Every ${escapeHtml(human)}` : `Every ${escapeHtml(human)} when enabled`}</h2><p>Drift checks are read-only. CommonKit will never apply changes on a schedule.</p></div><label>Check interval<select name="schedule-interval"><option value="300" ${interval === 300 ? "selected" : ""}>Every 5 minutes</option><option value="900" ${interval === 900 ? "selected" : ""}>Every 15 minutes</option><option value="3600" ${interval === 3600 ? "selected" : ""}>Every hour</option><option value="21600" ${interval === 21600 ? "selected" : ""}>Every 6 hours</option><option value="86400" ${interval === 86400 ? "selected" : ""}>Every day</option></select></label><div class="actions">${enabled ? '<button id="schedule-disable">Disable drift checks</button>' : '<button id="schedule-enable">Enable drift checks</button>'}</div>`;
}

function genericPanel(value: RecordValue): string {
  const entries = Object.entries(value).filter(([key]) => key !== "error");
  return entries.length ? `<dl>${entries.map(([key, item]) => `<dt>${escapeHtml(label(key))}</dt><dd>${escapeHtml(Array.isArray(item) ? `${item.length} item(s)` : typeof item === "object" ? "Configured" : item)}</dd>`).join("")}</dl>` : empty("No domain state was reported.");
}

export function managementPanel(route: Route, state: ManagementState): string {
  const value = record(state[route] ?? { error: `${route}_unavailable` });
  const renderer = ({ plans: plansPanel, credentials: credentialsPanel, snapshots: snapshotsPanel, relay: relayPanel, schedule: schedulePanel, diagnostics: diagnosticsPanel } as Partial<Record<Route, (v: RecordValue) => string>>)[route];
  const detail = route === "snapshots" ? snapshotsPanel(value) : error(value) ?? renderer?.(value) ?? genericPanel(value);
  const suppressActions = (route === "snapshots" && typeof value.error === "string")
    || (route === "credentials" && list(value.credentials ?? value.references).length === 0)
    || (route === "relay" && list(value.upstreams ?? value.servers).length === 0);
  const actions = suppressActions ? "" : operatorActions(route, value);
  const titles: Partial<Record<Route, string>> = {
    plans: "Changes",
    snapshots: "Data",
    relay: "MCP connections",
    schedule: "Drift checks",
  };
  const objectives: Partial<Record<Route, string>> = {
    plans: "Preview, approve, apply, and verify changes to this computer.",
    credentials: "Check secret references and provision approved destinations without exposing secret values.",
    snapshots: "Protect mutable databases, restore known-good state, and control the authoritative writer.",
    relay: "See whether portable MCP connections are reachable through CommonKit’s local relay.",
    schedule: "Choose how often CommonKit checks for drift. Scheduled checks never apply changes.",
    diagnostics: "Understand service and adapter health, then export a redacted support report.",
  };
  return `<section class="panel management-panel"><p class="eyebrow">${escapeHtml(route)}</p><h1>${escapeHtml(titles[route] ?? label(route))}</h1><p class="screen-objective">${escapeHtml(objectives[route] ?? "")}</p>${detail}${actions}</section>`;
}

function operatorActions(route: Route, value: RecordValue): string {
  switch (route) {
    case "plans": {
      const plan = record(value.plan ?? value.currentPlan ?? value);
      const apply = typeof plan.id === "string" && list(plan.operations).length > 0
        ? '<button id="apply-plan">Apply reviewed plan</button>'
        : "";
      return `<div class="actions"><button id="plan-sync">Generate plan</button><button id="verify-state">Verify now</button>${apply}</div>`;
    }
    case "snapshots": return '<div class="actions"><button id="snapshot-create">Create snapshot</button><button id="snapshot-restore">Restore snapshot</button><button id="snapshot-promote">Promote writer</button></div>';
    case "relay": return '<div class="actions"><button id="relay-restart">Restart relay</button></div>';
    case "schedule": return "";
    case "credentials": return '<div class="actions"><button id="credential-readiness">Check readiness</button><button id="credential-apply">Apply credential</button><button id="credential-verify">Verify credential</button></div>';
    case "diagnostics": return '<div class="actions"><button id="diagnostics-refresh">Refresh</button><button id="diagnostics-export">Export redacted report</button></div>';
    default: return "";
  }
}
