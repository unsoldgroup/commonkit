import type { Route } from "./navigation.ts";
import { escapeHtml } from "./html.ts";
import { icon } from "./icons.ts";

export type ManagementState = Partial<Record<Route, unknown>>;
type RecordValue = Record<string, unknown>;

function record(value: unknown): RecordValue { return value && typeof value === "object" && !Array.isArray(value) ? value as RecordValue : {}; }
function list(value: unknown): unknown[] { return Array.isArray(value) ? value : []; }
function label(value: unknown): string { return String(value ?? "unknown").replaceAll("_", " "); }
function empty(text: string): string { return `<p class="none-yet">${escapeHtml(text)}</p>`; }
function error(value: RecordValue): string | null {
  return typeof value.error === "string"
    ? `<div class="notice alert" role="alert">${icon.alert()}<p><strong>${escapeHtml(label(value.error))}</strong></p></div>`
    : null;
}

/** A bordered plate. Every panel in this app is one of these. */
function plate(title: string, meta: string, body: string): string {
  return `<div class="plate-block"><div class="plate-head"><h2>${escapeHtml(title)}</h2><span>${escapeHtml(meta)}</span></div><div class="plate-body">${body}</div></div>`;
}

function field(labelText: string, control: string, hint = ""): string {
  return `<label class="field"><span>${escapeHtml(labelText)}</span>${control}${hint ? `<small class="hint">${escapeHtml(hint)}</small>` : ""}</label>`;
}

const riskRank = new Map([["read_only", 0], ["low", 1], ["medium", 2], ["high", 3], ["destructive", 4]]);

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
  return String(resource.managedPath ?? resource.resourceId ?? operation.id ?? "Managed resource");
}

function operationSource(operation: RecordValue): string {
  const provenance = record(operation.provenance);
  return String(provenance.layer ?? provenance.source ?? operation.source ?? operation.adapterId ?? "composed loadout");
}

function plansPanel(value: RecordValue): string {
  const plan = record(value.plan ?? value.currentPlan ?? value);
  const operations = list(plan.operations);
  if (typeof plan.id !== "string") {
    return plate("Plan", "Ready to inspect", `<p>Generate a content-bound plan to see exactly what CommonKit would change. Planning reads state and writes nothing.</p>`);
  }
  const risk = planRisk(plan, operations);
  const operation = record(value.operation ?? value.applyOperation);
  const progress = operation.totalOperations
    ? `<div class="notice caution">${icon.caution()}<p><strong>${escapeHtml(label(operation.state ?? "Applying"))}</strong> · ${escapeHtml(String(operation.completedOperations ?? 0))} of ${escapeHtml(String(operation.totalOperations))} operations</p></div>`
    : "";
  const rows = operations.map((item) => {
    const op = record(item);
    const summary = typeof op.summary === "string" ? op.summary : label(op.kind);
    return `<li><b>${escapeHtml(summary)}</b><span>${escapeHtml(operationResource(op))}</span><small>${escapeHtml(label(op.risk))} risk · from ${escapeHtml(operationSource(op))}</small></li>`;
  }).join("");
  const heading = operations.length === 0 ? "Up to date" : `${operations.length} operation${operations.length === 1 ? "" : "s"}`;
  return `${progress}${plate("Plan " + String(plan.id), heading, `
    <p><span class="risk risk-${escapeHtml(risk)}">${escapeHtml(label(risk))} risk</span></p>
    ${rows ? `<ol class="stack">${rows}</ol>` : empty("This computer already matches the reviewed loadout.")}
    ${field("Reviewed plan", `<select name="plan-id"><option value="${escapeHtml(String(plan.id))}">${escapeHtml(String(plan.id))}</option></select>`, "Applying is confirmed per plan and per station.")}`)}`;
}

function snapshotsPanel(value: RecordValue): string {
  if (typeof value.error === "string") {
    return plate("Data", "Setup needed", `<p>CommonKit can protect a database by snapshotting it before anything changes. Choose which mutable database to protect, and which station is allowed to write to it.</p><div class="controls"><a class="engage" href="#settings" style="display:grid;place-items:center;text-decoration:none">Configure database</a></div>`);
  }
  const snapshots = list(value.snapshots);
  const writer = record(value.authoritativeWriter ?? value.writer);
  const rows = snapshots.map((item) => { const snap = record(item); return `<li><b>${escapeHtml(String(snap.databaseId ?? "Database"))}</b><span>${escapeHtml(String(snap.createdAt ?? snap.createdAtUnixMs ?? "Time unavailable"))}</span><small>${escapeHtml(String(snap.id))}</small></li>`; }).join("");
  const options = snapshots.map((item) => { const snap = record(item); return `<option value="${escapeHtml(String(snap.id))}">${escapeHtml(String(snap.databaseId ?? snap.id))}</option>`; }).join("");
  return plate("Authoritative writer", String(writer.targetId ?? "None recorded"), `
    <dl class="readout"><dt>Database</dt><dd${writer.databaseId ? "" : ' class="none"'}>${escapeHtml(String(writer.databaseId ?? "No database selected"))}</dd><dt>Writer</dt><dd${writer.targetId ? "" : ' class="none"'}>${escapeHtml(String(writer.targetId ?? "No writer recorded"))}</dd></dl>`)
    + plate("Snapshots", `${snapshots.length} stored`, `${rows ? `<ul class="stack">${rows}</ul>` : empty("No snapshots are available.")}
      <div style="margin-top:16px">${field("Database", `<input name="database-id" value="${escapeHtml(String(writer.databaseId ?? ""))}">`)}${field("Snapshot to restore", `<select name="snapshot-id">${options}</select>`)}${field("Promotion target", `<input name="promotion-target" value="${escapeHtml(String(writer.targetId ?? ""))}">`)}</div>`);
}

function aboutMePanel(value: RecordValue): string {
  if (typeof value.error === "string") {
    return plate("About Me", "Not created", `<p>Your profile is encrypted and kept separate from project memory. Nothing personal is written to Git, and nothing is saved until you review these answers.</p>
      <form id="about-me-setup">
        ${field("What should agents call you?", '<input name="about-me-name" autocomplete="name">')}
        ${field("How should agents explain unfamiliar things?", '<textarea name="about-me-explanations"></textarea>')}
        ${field("When there are choices, how should agents help you decide?", '<textarea name="about-me-decisions"></textarea>')}
        ${field("Which tools, languages, or areas do you use often?", '<textarea name="about-me-tools"></textarea>')}
        ${field("What recurring limits or working rules should agents remember?", '<textarea name="about-me-constraints"></textarea>')}
        ${field("What should agents never assume about you?", '<textarea name="about-me-never-assume"></textarea>')}
        <div class="controls"><button class="engage" type="submit">Review profile</button></div>
      </form>`);
  }
  const summary = typeof value.summary === "string" ? value.summary : "";
  const pending = Number(value.pendingSuggestions ?? 0);
  const suggestions = list(value.suggestions).map((item) => {
    const suggestion = record(item);
    const claim = record(suggestion.claim);
    return `<li><b>${escapeHtml(String(claim.text ?? "Suggested memory"))}</b><span>${escapeHtml(String(suggestion.evidenceQuote ?? ""))}</span><small><span class="controls" style="margin-top:8px"><button class="standby" type="button" data-about-me-id="${escapeHtml(String(suggestion.id))}" data-about-me-decision="accept">Remember</button><button class="standby" type="button" data-about-me-id="${escapeHtml(String(suggestion.id))}" data-about-me-decision="reject">Forget</button></span></small></li>`;
  }).join("");
  return plate("Encrypted profile", `Revision ${escapeHtml(String(value.revision ?? 0))}`, `
    <p>${summary ? escapeHtml(summary) : "No approved summary yet."}</p>
    <p>${pending} suggestion${pending === 1 ? "" : "s"} to review. Detailed claims are shared only with the current loadout and project.</p>`)
    + plate("Suggestions", pending ? "Awaiting you" : "Clear", suggestions ? `<ul class="stack">${suggestions}</ul>` : empty("No new memories are waiting for review."));
}

function relayPanel(value: RecordValue): string {
  const upstreams = list(value.upstreams ?? value.servers);
  if (upstreams.length === 0) {
    return plate("MCP connections", "Setup needed", `<p>Portable declarations describe the service. CommonKit keeps its authenticated relay running at a stable local endpoint.</p><div class="controls"><a class="engage" href="#settings" style="display:grid;place-items:center;text-decoration:none">Add connection</a></div>`);
  }
  const rows = upstreams.map((item) => { const upstream = record(item); return `<li><b>${escapeHtml(String(upstream.name ?? upstream.id))}</b><span>${escapeHtml(String(upstream.state ?? upstream.health ?? "unknown"))}</span><small>${escapeHtml(String(upstream.transport ?? upstream.command ?? "Managed transport"))}</small></li>`; }).join("");
  return plate(`Relay ${label(value.state ?? "unavailable")}`, `${upstreams.length} upstream${upstreams.length === 1 ? "" : "s"}`, `<ul class="stack">${rows}</ul>`);
}

function diagnosticsPanel(value: RecordValue): string {
  const adapters = list(value.adapters ?? value.components ?? value.checks);
  const rows = adapters.map((item) => { const adapter = record(item); return `<li><b>${escapeHtml(String(adapter.name ?? adapter.id ?? adapter.component))}</b><span>${escapeHtml(String(adapter.state ?? adapter.status ?? "unknown"))}</span><small>${escapeHtml(String(adapter.detail ?? adapter.message ?? "No diagnostic detail"))}</small></li>`; }).join("");
  return plate("Component health", `${adapters.length} reported`, rows ? `<ul class="stack">${rows}</ul>` : empty("No adapter diagnostics were reported."));
}

function credentialsPanel(value: RecordValue): string {
  const credentials = list(value.credentials ?? value.references);
  if (credentials.length === 0) {
    return plate("Credentials", "Setup needed", `<p>CommonKit stores only a reference to your secret provider. Secret values never enter the portable kit or this window.</p>
      ${field("Credential reference", '<input name="credential-reference" placeholder="bws://secret-id" autocomplete="off">')}
      <div class="controls"><button class="engage" id="credential-readiness" type="button">Check reference</button></div>`);
  }
  const rows = credentials.map((item) => { const credential = record(item); return `<li><b>${escapeHtml(String(credential.reference ?? credential.id))}</b><span>${escapeHtml(String(credential.readiness ?? credential.state ?? "unknown"))}</span><small>Reference only, secret value hidden</small></li>`; }).join("");
  const destinations = credentials.map((item) => {
    const credential = record(item);
    const id = credential.destinationId ?? credential.id;
    return id ? `<option value="${escapeHtml(String(id))}">${escapeHtml(String(id))}</option>` : "";
  }).join("");
  return plate("References", `${credentials.length} configured`, `<ul class="stack">${rows}</ul><div style="margin-top:16px">${field("Destination", `<select name="credential-destination">${destinations}</select>`)}</div>`);
}

function schedulePanel(value: RecordValue): string {
  const enabled = value.enabled === true;
  const interval = typeof value.intervalSeconds === "number" ? value.intervalSeconds : 900;
  const human = interval % 3600 === 0
    ? `${interval / 3600} hour${interval === 3600 ? "" : "s"}`
    : interval % 60 === 0 ? `${interval / 60} minute${interval === 60 ? "" : "s"}` : `${interval} seconds`;
  const options = [[300, "Every 5 minutes"], [900, "Every 15 minutes"], [3600, "Every hour"], [21600, "Every 6 hours"], [86400, "Every day"]] as const;
  return plate("Drift checks", enabled ? `Every ${human}` : "Paused", `
    <p>Drift checks are read-only. CommonKit never applies changes on a schedule.</p>
    ${field("Interval", `<select name="schedule-interval">${options.map(([seconds, text]) => `<option value="${seconds}" ${interval === seconds ? "selected" : ""}>${text}</option>`).join("")}</select>`)}
    <div class="controls">${enabled ? '<button class="standby" type="button" id="schedule-disable">Disable</button>' : '<button class="engage" type="button" id="schedule-enable">Enable</button>'}</div>`);
}

function genericPanel(value: RecordValue): string {
  const entries = Object.entries(value).filter(([key]) => key !== "error");
  return entries.length
    ? plate("State", `${entries.length} field${entries.length === 1 ? "" : "s"}`, `<dl class="readout">${entries.map(([key, item]) => `<dt>${escapeHtml(label(key))}</dt><dd>${escapeHtml(Array.isArray(item) ? `${item.length} item(s)` : typeof item === "object" ? "Configured" : String(item))}</dd>`).join("")}</dl>`)
    : empty("No domain state was reported.");
}

const TITLES: Partial<Record<Route, string>> = {
  plans: "Changes", snapshots: "Data", aboutMe: "About Me", relay: "MCP connections", schedule: "Drift checks",
};

const OBJECTIVES: Partial<Record<Route, string>> = {
  plans: "Preview, approve, apply, and verify changes to this computer.",
  credentials: "Check secret references and provision approved destinations without exposing secret values.",
  snapshots: "Protect mutable databases, restore known-good state, and control the authoritative writer.",
  aboutMe: "Review the encrypted personal context that authorized agents may use.",
  relay: "See whether portable MCP connections are reachable through CommonKit's local relay.",
  schedule: "Choose how often CommonKit checks for drift. Scheduled checks never apply changes.",
  diagnostics: "Understand service and adapter health, then export a redacted support report.",
};

export function managementPanel(route: Route, state: ManagementState): string {
  const value = record(state[route] ?? { error: `${route}_unavailable` });
  const renderer = ({ plans: plansPanel, credentials: credentialsPanel, snapshots: snapshotsPanel, aboutMe: aboutMePanel, relay: relayPanel, schedule: schedulePanel, diagnostics: diagnosticsPanel } as Partial<Record<Route, (v: RecordValue) => string>>)[route];
  const detail = route === "snapshots" ? snapshotsPanel(value)
    : route === "aboutMe" ? aboutMePanel(value)
    : error(value) ?? renderer?.(value) ?? genericPanel(value);
  const suppressActions = (route === "snapshots" && typeof value.error === "string")
    || (route === "credentials" && list(value.credentials ?? value.references).length === 0)
    || (route === "relay" && list(value.upstreams ?? value.servers).length === 0);
  return `<section>
    <div class="placard"><h1>${escapeHtml(TITLES[route] ?? label(route))}</h1></div>
    <p class="brief">${escapeHtml(OBJECTIVES[route] ?? "")}</p>
    ${detail}
    ${suppressActions ? "" : operatorActions(route, value)}
  </section>`;
}

function operatorActions(route: Route, value: RecordValue): string {
  const wrap = (inner: string) => `<div class="controls" style="margin-top:18px">${inner}</div>`;
  switch (route) {
    case "plans": {
      const plan = record(value.plan ?? value.currentPlan ?? value);
      const apply = typeof plan.id === "string" && list(plan.operations).length > 0
        ? '<button class="engage" type="button" id="apply-plan">Apply reviewed plan</button>' : "";
      return wrap(`<button class="standby" type="button" id="plan-sync">Generate plan</button><button class="standby" type="button" id="verify-state">Verify now</button>${apply}`);
    }
    case "snapshots": return wrap('<button class="standby" type="button" id="snapshot-create">Create snapshot</button><button class="standby" type="button" id="snapshot-restore">Restore</button><button class="standby" type="button" id="snapshot-promote">Promote writer</button>');
    case "relay": return wrap('<button class="standby" type="button" id="relay-restart">Restart relay</button>');
    case "schedule": return "";
    case "credentials": return wrap('<button class="standby" type="button" id="credential-readiness">Check readiness</button><button class="engage" type="button" id="credential-apply">Apply credential</button><button class="standby" type="button" id="credential-verify">Verify</button>');
    case "diagnostics": return wrap('<button class="standby" type="button" id="diagnostics-refresh">Refresh</button><button class="standby" type="button" id="diagnostics-export">Export redacted report</button>');
    default: return "";
  }
}
