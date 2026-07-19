import "./style.css";
import { desktopApi } from "./api.ts";
import { navigation, routeFromHash, type Route } from "./navigation.ts";
import { statusView } from "./view-model.ts";
import type { DesktopSnapshot, ManagementSnapshot, TargetInventorySnapshot } from "./contracts.ts";
import { updatePanel, type UpdateUiState } from "./updater-view.ts";
import { managementPanel } from "./management-view.ts";
import { onboardingPanel, type OnboardingProvider } from "./onboarding-view.ts";
import { open } from "@tauri-apps/plugin-dialog";

const app = document.querySelector<HTMLElement>("#app")!;
let snapshot: DesktopSnapshot | null = null;
let management: ManagementSnapshot | null = null;
let targets: TargetInventorySnapshot | null = null;
let updateState: UpdateUiState = { kind: "idle" };
let onboardingProvider: OnboardingProvider = "native";
let onboardingMessage = "";

function placeholder(route: Route): string {
  const copy: Record<Route, [string, string]> = {
    onboarding: ["Get started", "Connect or create a GitHub-backed kit, then select this machine’s target and loadout."],
    status: ["Status", "Loading the local CommonKit service…"],
    plans: ["Plan review", "Plans show semantic operations, provenance, risk, and required confirmation before apply."],
    credentials: ["Credential readiness", "CommonKit reports references and readiness here. Secret values never enter this window."],
    snapshots: ["Snapshots", "Review encrypted snapshot history, writer authority, restore plans, and promotions."],
    relay: ["Relay", "Review persistent relay health and authenticated upstream readiness."],
    schedule: ["Drift schedule", "Configure read-only checks and notifications. Mutations remain explicitly confirmed."],
    diagnostics: ["Diagnostics", "Inspect redacted component health and export a schema-bound diagnostic bundle."],
    settings: ["Settings", "Control autostart and signed update checks."],
  };
  const [heading, detail] = copy[route];
  return `<section class="panel"><p class="eyebrow">${heading}</p><h1>${heading}</h1><p>${detail}</p></section>`;
}

function render(): void {
  const route = routeFromHash(location.hash);
  const body = route === "onboarding" ? onboardingPanel(onboardingProvider, onboardingMessage) : route === "settings" ? updatePanel(updateState) : route === "status" && snapshot ? (() => {
    const view = statusView(snapshot.status);
    const inventory = targets ? `<fieldset><legend>Managed targets</legend>${targets.targets.map((target) => `<label><input type="checkbox" name="managed-target" value="${target.id}" ${targets?.selected.includes(target.id) ? "checked" : ""}> ${target.id} · ${target.transport.type}</label>`).join("")}<button id="save-target-selection" type="button">Save target selection</button></fieldset>` : "";
    return `<section class="panel"><p class="eyebrow">Selected targets</p><h1>${view.heading}</h1><p>${view.detail}</p><dl><dt>Active target</dt><dd>${snapshot.status.activeTarget ?? "Multiple or not selected"}</dd><dt>Loadout</dt><dd>${snapshot.status.activeLoadout ?? "Not selected"}</dd><dt>Runtime</dt><dd>${snapshot.status.runtimeVersion}</dd></dl>${inventory}${view.primaryRoute !== "status" ? `<a class="primary" href="#${view.primaryRoute}">Continue</a>` : ""}</section>`;
  })() : management && route in management ? managementPanel(route, management) : placeholder(route);
  app.innerHTML = `<aside><div class="brand">CommonKit</div><nav>${navigation.map(({ route: id, label }) => `<a class="${route === id ? "active" : ""}" href="#${id}">${label}</a>`).join("")}</nav></aside><main>${body}</main>`;
  if (route === "onboarding") bindOnboardingActions();
  else if (route === "settings") bindUpdateActions();
  else if (route === "status") bindTargetActions();
  else if (management && route in management) bindManagementActions(route);
}

function bindTargetActions(): void {
  document.querySelector<HTMLButtonElement>("#save-target-selection")?.addEventListener("click", async () => {
    const selected = [...document.querySelectorAll<HTMLInputElement>('input[name="managed-target"]:checked')].map((input) => input.value);
    if (!window.confirm(`Run read-only checks for ${selected.length} selected target(s)? Future mutations still require per-target confirmation.`)) return;
    try {
      targets = await desktopApi.selectTargets(selected, confirmationId("target-select"));
    } catch (error) {
      window.alert(errorMessage(error));
    }
    render();
  });
}

function bindOnboardingActions(): void {
  for (const button of document.querySelectorAll<HTMLButtonElement>("[data-pick], [data-pick-directory]")) {
    button.addEventListener("click", async () => {
      const name = button.dataset.pick ?? button.dataset.pickDirectory;
      if (!name) return;
      const selected = await open({ directory: Boolean(button.dataset.pickDirectory), multiple: false });
      if (typeof selected === "string") {
        const input = document.querySelector<HTMLInputElement>(`[name="${name}"]`);
        if (input) input.value = selected;
      }
    });
  }
  document.querySelector<HTMLSelectElement>("#onboarding-provider")?.addEventListener("change", (event) => {
    onboardingProvider = (event.currentTarget as HTMLSelectElement).value as OnboardingProvider;
    onboardingMessage = "";
    render();
  });
  document.querySelector<HTMLFormElement>("#onboarding-form")?.addEventListener("submit", async (event) => {
    event.preventDefault();
    const values: Record<string, unknown> = Object.fromEntries(new FormData(event.currentTarget as HTMLFormElement).entries());
    values.publishRegistration = values.publishRegistration === "true";
    onboardingMessage = "Validating pinned provider and materializing…";
    render();
    try {
      const result = await desktopApi.onboardingInitialize(values);
      const plan = (result as { firstPlanId?: string }).firstPlanId ?? "ready";
      onboardingMessage = `First plan ${plan} is ready for review.`;
    } catch (error) {
      onboardingMessage = errorMessage(error);
    }
    render();
  });
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function bindUpdateActions(): void {
  document.querySelector<HTMLButtonElement>("#check-update")?.addEventListener("click", async () => {
    updateState = { kind: "checking" };
    render();
    try {
      const update = await desktopApi.checkForUpdate();
      updateState = update ? { kind: "available", update } : { kind: "current" };
    } catch (error) {
      updateState = { kind: "error", message: errorMessage(error) };
    }
    render();
  });
  document.querySelector<HTMLButtonElement>("#install-update")?.addEventListener("click", async () => {
    if (updateState.kind !== "available") return;
    const update = updateState.update;
    if (!window.confirm(`Install signed CommonKit ${update.version}?`)) return;
    updateState = { kind: "installing", update };
    render();
    try {
      await desktopApi.installUpdate(update.version, true);
    } catch (error) {
      updateState = { kind: "error", message: errorMessage(error) };
      render();
    }
  });
}

function confirmationId(action: string): string {
  return `desktop-${action}-${Date.now()}`;
}

function required(promptText: string): string | null {
  const value = window.prompt(promptText)?.trim();
  return value || null;
}

async function showResult(route: Route, action: () => Promise<unknown>): Promise<void> {
  try {
    const value = await action();
    if (management) (management as unknown as Record<string, unknown>)[route] = value;
  } catch (error) {
    if (management) (management as unknown as Record<string, unknown>)[route] = { error: errorMessage(error) };
  }
  render();
}

function bindManagementActions(route: Route): void {
  document.querySelector("#verify-state")?.addEventListener("click", () => void showResult(route, desktopApi.verify));
  document.querySelector("#snapshot-create")?.addEventListener("click", () => {
    const databaseId = required("Database ID to snapshot");
    if (!databaseId || !window.confirm(`Create an encrypted snapshot of ${databaseId}?`)) return;
    void showResult(route, () => desktopApi.snapshotCreate(databaseId, confirmationId("snapshot-create")));
  });
  document.querySelector("#snapshot-restore")?.addEventListener("click", () => {
    const snapshotId = required("Snapshot ID to restore");
    if (!snapshotId || !window.confirm(`Restore ${snapshotId}? The current database will be backed up first.`)) return;
    void showResult(route, () => desktopApi.snapshotRestore(snapshotId, confirmationId("snapshot-restore")));
  });
  document.querySelector("#snapshot-promote")?.addEventListener("click", () => {
    const databaseId = required("Database ID");
    const targetId = databaseId ? required("Target ID to promote as authoritative writer") : null;
    if (!databaseId || !targetId || !window.confirm(`Promote ${targetId} as writer for ${databaseId}?`)) return;
    void showResult(route, () => desktopApi.snapshotPromote(databaseId, targetId, confirmationId("snapshot-promote")));
  });
  document.querySelector("#relay-restart")?.addEventListener("click", () => {
    if (!window.confirm("Restart the managed relay runtime?")) return;
    void showResult(route, () => desktopApi.relayRestart(confirmationId("relay-restart")));
  });
  document.querySelector("#relay-reconcile")?.addEventListener("click", () => {
    const source = required("Paste the reviewed relay reconciliation request JSON");
    if (!source) return;
    try {
      const request: unknown = JSON.parse(source);
      if (!window.confirm("Apply this reviewed relay desired state?")) return;
      void showResult(route, () => desktopApi.relayReconcile(request, confirmationId("relay-reconcile")));
    } catch {
      if (management) (management as unknown as Record<string, unknown>)[route] = { error: "invalid_relay_request" };
      render();
    }
  });
  document.querySelector("#schedule-enable")?.addEventListener("click", () => {
    const raw = required("Drift-check interval in seconds");
    const interval = raw ? Number(raw) : 0;
    if (!Number.isSafeInteger(interval) || interval < 1 || !window.confirm(`Enable read-only drift checks every ${interval} seconds?`)) return;
    void showResult(route, () => desktopApi.scheduleConfigure(true, interval, confirmationId("schedule-enable")));
  });
  document.querySelector("#schedule-disable")?.addEventListener("click", () => {
    if (!window.confirm("Disable scheduled drift checks?")) return;
    void showResult(route, () => desktopApi.scheduleConfigure(false, 1, confirmationId("schedule-disable")));
  });
  document.querySelector("#credential-readiness")?.addEventListener("click", () => {
    const reference = required("Credential reference (for example bws://secret-id)");
    if (reference) void showResult(route, () => desktopApi.credentialReadiness([reference]));
  });
  document.querySelector("#credential-apply")?.addEventListener("click", () => {
    const id = required("Configured credential destination ID");
    if (!id || !window.confirm(`Provision credential destination ${id}? Secret values remain hidden.`)) return;
    void showResult(route, () => desktopApi.credentialApply([id], confirmationId("credential-apply")));
  });
  document.querySelector("#credential-verify")?.addEventListener("click", () => {
    const id = required("Configured credential destination ID to verify");
    if (id) void showResult(route, () => desktopApi.credentialVerify([id]));
  });
  document.querySelector("#diagnostics-export")?.addEventListener("click", () => {
    void desktopApi.diagnosticsExport().then((value) => {
      const url = URL.createObjectURL(new Blob([JSON.stringify(value, null, 2)], { type: "application/json" }));
      const link = document.createElement("a");
      link.href = url;
      link.download = "commonkit-diagnostics.json";
      link.click();
      URL.revokeObjectURL(url);
    }).catch((error) => void showResult(route, async () => { throw error; }));
  });
}

addEventListener("hashchange", render);
render();
desktopApi.snapshot().then((value) => { snapshot = value; render(); }).catch(() => render());
desktopApi.managementSnapshot().then((value) => { management = value; render(); }).catch(() => render());
desktopApi.targets().then((value) => { targets = value; render(); }).catch(() => render());
