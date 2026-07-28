import "./style.css";
import { desktopApi } from "./api.ts";
import { navigationForSetup, routeForSetup, routeFromHash, type Route } from "./navigation.ts";
import { statusView } from "./view-model.ts";
import type { DesktopSnapshot, ManagementSnapshot, TargetInventorySnapshot } from "./contracts.ts";
import { updatePanel, type UpdateUiState } from "./updater-view.ts";
import { managementPanel } from "./management-view.ts";
import { defaultOnboardingDraft, onboardingPanel, type OnboardingViewState } from "./onboarding-view.ts";
import { applyOnboardingValues, onboardingRequest } from "./onboarding-controller.ts";
import { open } from "@tauri-apps/plugin-dialog";
import { assertPlanTarget, convergenceTarget } from "./target-selection.ts";
import { refreshDesktopState, shouldRunLiveRefresh } from "./live-refresh.ts";
import { setupCompletion } from "./setup-gate.ts";

const app = document.querySelector<HTMLElement>("#app")!;
let snapshot: DesktopSnapshot | null = null;
let management: ManagementSnapshot | null = null;
let targets: TargetInventorySnapshot | null = null;
let updateState: UpdateUiState = { kind: "idle" };
let onboardingState: OnboardingViewState = {
  step: 1,
  auth: { state: "checking" },
  draft: defaultOnboardingDraft(),
  message: "",
  submitting: false,
};
let setupUnlocked = false;
let setupCheckFailed = false;

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
  // Live domain refreshes can arrive while a user is typing in the wizard.
  // Preserve the visible form before replacing the DOM so no keystroke is lost.
  captureOnboardingDraft();
  const completion = setupUnlocked ? "complete" : setupCheckFailed ? "required" : setupCompletion(snapshot?.status ?? null, targets);
  const setupComplete = completion === "complete";
  const route = routeForSetup(routeFromHash(location.hash), setupComplete);
  const visibleNavigation = navigationForSetup(setupComplete);
  const body = completion === "checking" ? `<section class="panel setup-check"><p class="eyebrow">First run</p><h1>Checking this computer…</h1><p>CommonKit is checking whether setup is already complete.</p></section>` : route === "onboarding" ? onboardingPanel(onboardingState) : route === "settings" ? updatePanel(updateState) : route === "status" && snapshot ? (() => {
    const view = statusView(snapshot.status);
    const inventory = targets ? `<fieldset><legend>Managed targets</legend>${targets.targets.map((target) => `<label><input type="checkbox" name="managed-target" value="${target.id}" ${targets?.selected.includes(target.id) ? "checked" : ""}> ${target.id} · ${target.transport.type}</label>`).join("")}<button id="save-target-selection" type="button">Save target selection</button></fieldset>` : "";
    return `<section class="panel"><p class="eyebrow">Selected targets</p><h1>${view.heading}</h1><p>${view.detail}</p><dl><dt>Active target</dt><dd>${snapshot.status.activeTarget ?? "Multiple or not selected"}</dd><dt>Loadout</dt><dd>${snapshot.status.activeLoadout ?? "Not selected"}</dd><dt>Runtime</dt><dd>${snapshot.status.runtimeVersion}</dd></dl>${inventory}${view.primaryRoute !== "status" ? `<a class="primary" href="#${view.primaryRoute}">Continue</a>` : ""}</section>`;
  })() : management && route in management ? managementPanel(route, management) : placeholder(route);
  app.innerHTML = `<aside><div class="brand">CommonKit</div><nav>${visibleNavigation.map(({ route: id, label }) => `<a class="${route === id ? "active" : ""}" href="#${id}">${label}</a>`).join("")}</nav></aside><main>${body}</main>`;
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
        if (input) {
          input.value = selected;
          applyOnboardingValues(onboardingState.draft, new Map([[name, selected]]));
        }
      }
    });
  }
  document.querySelector<HTMLButtonElement>("#github-sign-in")?.addEventListener("click", async () => {
    onboardingState.auth = { state: "authenticating" };
    onboardingState.message = "Finish signing in in the GitHub window. Your one-time code is copied and ready to paste; CommonKit never sees your password.";
    render();
    try {
      const status = await desktopApi.githubAuthLogin();
      onboardingState.auth = status;
      onboardingState.message = status.state === "authenticated" ? "GitHub connected. Your setup repository will be private." : "GitHub sign-in was not completed.";
    } catch (error) {
      onboardingState.auth = { state: "error", message: errorMessage(error) };
      onboardingState.message = "";
    }
    render();
  });
  document.querySelectorAll<HTMLInputElement>('input[name="mode"], input[name="provider"]').forEach((input) => input.addEventListener("change", () => {
    captureOnboardingDraft();
    onboardingState.message = "";
    render();
  }));
  document.querySelector<HTMLButtonElement>("#safe-test-folder")?.addEventListener("change", (event) => {
    const checked = (event.currentTarget as HTMLInputElement).checked;
    if (checked) {
      const home = onboardingState.draft.kitDirectory.match(/^(.*)\/\.config\//)?.[1];
      if (home) onboardingState.draft.targetRoot = `${home}/CommonKitManaged`;
    }
    render();
  });
  document.querySelector<HTMLButtonElement>("#setup-back")?.addEventListener("click", () => {
    captureOnboardingDraft();
    if (onboardingState.step > 1) onboardingState.step = (onboardingState.step - 1) as OnboardingViewState["step"];
    onboardingState.message = "";
    renderAndFocusSetup();
  });
  document.querySelector<HTMLFormElement>("#onboarding-form")?.addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = event.currentTarget as HTMLFormElement;
    if (!form.reportValidity()) return;
    captureOnboardingDraft(form);
    if (onboardingState.step < 4) {
      onboardingState.step = (onboardingState.step + 1) as OnboardingViewState["step"];
      onboardingState.message = "";
      renderAndFocusSetup();
      return;
    }
    if (onboardingState.submitting || onboardingState.auth.state !== "authenticated") return;
    onboardingState.draft.publishRegistration = new FormData(form).get("publishRegistration") === "true";
    onboardingState.submitting = true;
    onboardingState.message = "Checking your setup and preparing the preview…";
    render();
    try {
      const result = await desktopApi.onboardingInitialize(onboardingRequest(onboardingState.draft, onboardingState.auth.login));
      const plan = (result as { firstPlanId?: string }).firstPlanId ?? "ready";
      onboardingState.message = `Your setup preview ${plan} is ready. Review it before applying any changes.`;
      setupUnlocked = true;
      location.hash = "#plans";
    } catch (error) {
      onboardingState.message = errorMessage(error);
    } finally {
      onboardingState.submitting = false;
    }
    render();
  });
}

function captureOnboardingDraft(form = document.querySelector<HTMLFormElement>("#onboarding-form")): void {
  if (!form) return;
  applyOnboardingValues(onboardingState.draft, new FormData(form).entries());
}

function renderAndFocusSetup(): void {
  render();
  document.querySelector<HTMLElement>("#setup-heading")?.focus();
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

function controlValue(name: string): string | null {
  const value = document.querySelector<HTMLInputElement | HTMLSelectElement>(`[name="${name}"]`)?.value.trim();
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

function showError(route: Route, error: unknown): void {
  if (management) (management as unknown as Record<string, unknown>)[route] = { error: errorMessage(error) };
  render();
}

function bindManagementActions(route: Route): void {
  document.querySelector("#plan-sync")?.addEventListener("click", () => {
    try {
      const targetId = convergenceTarget(targets);
      if (!window.confirm(`Stage providers and create a plan for ${targetId}?`)) return;
      void showResult(route, async () => {
        const plan = await desktopApi.planSync(targetId, confirmationId("plan-sync"));
        assertPlanTarget(plan, targetId);
        return plan;
      });
    } catch (error) {
      showError(route, error);
    }
  });
  document.querySelector("#verify-state")?.addEventListener("click", () => {
    try {
      const targetId = convergenceTarget(targets);
      void showResult(route, () => desktopApi.verify(targetId));
    } catch (error) {
      showError(route, error);
    }
  });
  document.querySelector("#apply-plan")?.addEventListener("click", () => {
    const planId = controlValue("plan-id");
    if (!planId) return;
    try {
      const targetId = convergenceTarget(targets);
      if (!window.confirm(`Apply reviewed plan ${planId} to ${targetId}?`)) return;
      void showResult(route, () => desktopApi.applyPlan(targetId, planId, confirmationId("apply-plan")));
    } catch (error) {
      showError(route, error);
    }
  });
  document.querySelector("#snapshot-create")?.addEventListener("click", () => {
    const databaseId = controlValue("database-id");
    if (!databaseId || !window.confirm(`Create an encrypted snapshot of ${databaseId}?`)) return;
    void showResult(route, () => desktopApi.snapshotCreate(databaseId, confirmationId("snapshot-create")));
  });
  document.querySelector("#snapshot-restore")?.addEventListener("click", () => {
    const snapshotId = controlValue("snapshot-id");
    if (!snapshotId || !window.confirm(`Restore ${snapshotId}? The current database will be backed up first.`)) return;
    void showResult(route, () => desktopApi.snapshotRestore(snapshotId, confirmationId("snapshot-restore")));
  });
  document.querySelector("#snapshot-promote")?.addEventListener("click", () => {
    const databaseId = controlValue("database-id");
    const targetId = databaseId ? controlValue("promotion-target") : null;
    if (!databaseId || !targetId || !window.confirm(`Promote ${targetId} as writer for ${databaseId}?`)) return;
    void showResult(route, () => desktopApi.snapshotPromote(databaseId, targetId, confirmationId("snapshot-promote")));
  });
  document.querySelector("#relay-restart")?.addEventListener("click", () => {
    if (!window.confirm("Restart the managed relay runtime?")) return;
    void showResult(route, () => desktopApi.relayRestart(confirmationId("relay-restart")));
  });
  document.querySelector("#relay-reconcile")?.addEventListener("click", () => {
    const request = management?.relay;
    if (!request || !window.confirm("Reconcile the configured relay inventory shown above?")) return;
    void showResult(route, () => desktopApi.relayReconcile(request, confirmationId("relay-reconcile")));
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
    if (!id) return;
    void showResult(route, async () => {
      const plan = await desktopApi.credentialPlan([id]);
      const operations = plan.operations
        .map((operation) => `${operation.action} ${operation.destinationId} at ${operation.path}`)
        .join("\n");
      if (!window.confirm(`Review credential plan ${plan.planId}:\n\n${operations}\n\nApply this exact plan? Secret values remain hidden.`)) {
        return { plan, applied: false };
      }
      return desktopApi.credentialApply(plan.planId, confirmationId("credential-apply"));
    });
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
    }).catch((error) => showError(route, error));
  });
}

addEventListener("hashchange", () => {
  window.scrollTo(0, 0);
  render();
});
render();
async function initializeOnboarding(): Promise<void> {
  const [auth, defaults] = await Promise.allSettled([desktopApi.githubAuthStatus(), desktopApi.onboardingDefaults()]);
  onboardingState.auth = auth.status === "fulfilled" ? auth.value : { state: "error", message: errorMessage(auth.reason) };
  if (defaults.status === "fulfilled") {
    onboardingState.draft.kitDirectory ||= defaults.value.kitDirectory;
    onboardingState.draft.targetRoot ||= defaults.value.targetRoot;
    if (onboardingState.draft.computerName === "workstation") onboardingState.draft.computerName = defaults.value.computerName;
  } else {
    onboardingState.message = "CommonKit could not determine safe local folders. Choose them manually to continue.";
  }
  render();
}
void initializeOnboarding();
async function initializeSetupGate(): Promise<void> {
  const [observed, inventory] = await Promise.allSettled([desktopApi.snapshot(), desktopApi.targets()]);
  if (observed.status === "fulfilled") snapshot = observed.value;
  if (inventory.status === "fulfilled") targets = inventory.value;
  setupCheckFailed = observed.status === "rejected" || inventory.status === "rejected";
  if (setupCheckFailed) onboardingState.message = "The CommonKit background service is unavailable. Restart CommonKit, then retry setup.";
  render();
}
void initializeSetupGate();
let refreshRunning = false;
async function refreshLiveState(): Promise<void> {
  if (refreshRunning) return;
  const completion = setupUnlocked ? "complete" : setupCompletion(snapshot?.status ?? null, targets);
  if (!shouldRunLiveRefresh(setupUnlocked, completion)) return;
  refreshRunning = true;
  try {
    const state = await refreshDesktopState(desktopApi, {
      ...(snapshot ? { snapshot } : {}), ...(management ? { management } : {}), ...(targets ? { targets } : {}),
    });
    snapshot = state.snapshot;
    management = state.management;
    targets = state.targets;
    setupCheckFailed = false;
    if (onboardingState.message.startsWith("The CommonKit background service is unavailable.")) {
      onboardingState.message = "";
    }
    const operation = [...snapshot.events].reverse().find((event) => event.event === "operation.updated")?.data;
    if (operation && management) {
      management.plans = { ...(typeof management.plans === "object" && management.plans ? management.plans : {}), operation };
    }
    render();
  } catch { render(); }
  finally { refreshRunning = false; }
}
void refreshLiveState();
const liveRefreshTimer = window.setInterval(() => void refreshLiveState(), 2_000);
addEventListener("beforeunload", () => window.clearInterval(liveRefreshTimer));
