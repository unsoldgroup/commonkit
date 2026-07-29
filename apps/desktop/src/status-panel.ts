import type { DesktopSnapshot, ManagementSnapshot, TargetInventorySnapshot } from "./contracts.ts";
import type { SettingsSnapshot } from "./updater-view.ts";
import { escapeHtml } from "./html.ts";
import { statusView } from "./view-model.ts";

type RecordValue = Record<string, unknown>;
function record(value: unknown): RecordValue {
  return value && typeof value === "object" && !Array.isArray(value) ? value as RecordValue : {};
}
function list(value: unknown): unknown[] { return Array.isArray(value) ? value : []; }
type Readiness = "ready" | "setup" | "unavailable";
function capability(name: string, readiness: Readiness, detail: string, route: string): string {
  const label = readiness === "ready" ? "Ready" : readiness === "setup" ? "Setup needed" : "Unavailable";
  return `<a class="capability-card" href="#${route}"><span class="readiness-label is-${readiness}">${label}</span><strong>${escapeHtml(name)}</strong><small>${escapeHtml(detail)}</small></a>`;
}

export function statusPanel(
  snapshot: DesktopSnapshot,
  targets: TargetInventorySnapshot,
  management: ManagementSnapshot | null,
  settings: SettingsSnapshot | null,
): string {
  const view = statusView(snapshot.status);
  const selected = targets.selected.filter((id) => targets.targets.some((target) => target.id === id));
  const target = selected.length === 1 ? selected[0] : `${selected.length} selected computers`;
  const plans = record(management?.plans);
  const credentials = record(management?.credentials);
  const snapshots = record(management?.snapshots);
  const relay = record(management?.relay);
  const schedule = record(management?.schedule);
  const available = management !== null;
  const capabilityState = (value: RecordValue, configured: boolean): Readiness => {
    if (!available) return "unavailable";
    if (typeof value.error === "string") {
      return value.error.includes("unconfigured") ? "setup" : "unavailable";
    }
    return configured ? "ready" : "setup";
  };
  const currentPlan = record(plans.plan ?? plans.currentPlan ?? plans);
  const changesState = capabilityState(plans, typeof currentPlan.id === "string");
  const credentialState = capabilityState(credentials, list(credentials.credentials ?? credentials.references).length > 0);
  const dataState = capabilityState(snapshots, true);
  const relayState = capabilityState(relay, list(relay.upstreams ?? relay.servers).length > 0);
  const scheduleState = capabilityState(schedule, schedule.enabled === true);
  const gitState = snapshot.gitSync.state ?? "unavailable";
  const lastCheck = snapshot.status.lastDriftCheckUnixMs
    ? new Date(snapshot.status.lastDriftCheckUnixMs).toLocaleString()
    : "Never";

  return `<section class="panel status-panel"><p class="eyebrow">Overview</p><h1>${escapeHtml(view.heading)}</h1><p>${escapeHtml(view.detail)}</p><div class="summary-card"><h2>${escapeHtml(target)}</h2><dl><dt>Kit repository</dt><dd>${escapeHtml(settings?.repository ?? "Not connected")}</dd><dt>Managed root</dt><dd>${escapeHtml(settings?.targetRoot ?? "Not configured")}</dd><dt>Git sync</dt><dd>${escapeHtml(gitState)}</dd><dt>Last drift check</dt><dd>${escapeHtml(lastCheck)}</dd></dl></div><h2>Capabilities</h2><div class="capability-grid">${capability("Changes", changesState, changesState === "ready" ? "A bound plan is ready to review" : "Generate a plan to inspect this computer", "plans")}${capability("Credentials", credentialState, credentialState === "ready" ? "References are configured" : "No references configured", "credentials")}${capability("Data", dataState, dataState === "ready" ? "Snapshot domain available" : "No protected database", "snapshots")}${capability("MCP connections", relayState, relayState === "ready" ? "Relay declarations available" : "No upstream services", "relay")}${capability("Drift checks", scheduleState, scheduleState === "ready" ? "Scheduled checks enabled" : "Scheduled checks paused", "schedule")}</div><a class="primary next-action" href="#plans">Review changes</a></section>`;
}
