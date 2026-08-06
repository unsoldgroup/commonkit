import "./style.css";
import { desktopApi } from "./api.ts";
import { navigationForSetup, routeForSetup, routeFromHash, type Route } from "./navigation.ts";
import type { DesktopSnapshot, ManagementSnapshot, TargetInventorySnapshot } from "./contracts.ts";
import { updatePanel, type SettingsSnapshot, type UpdateUiState } from "./updater-view.ts";
import { managementPanel } from "./management-view.ts";
import { defaultOnboardingDraft, onboardingPanel, type OnboardingViewState } from "./onboarding-view.ts";
import { applyOnboardingValues, onboardingRequest } from "./onboarding-controller.ts";
import { ask, message, open } from "@tauri-apps/plugin-dialog";
import { assertPlanTarget, convergenceTarget } from "./target-selection.ts";
import { checkForDesktopUpdate, refreshDesktopState, shouldRunLiveRefresh } from "./live-refresh.ts";
import { setupCompletion } from "./setup-gate.ts";
import { withDeadline } from "./async-deadline.ts";
import { statusPanel } from "./status-panel.ts";
import { profileInterviewPanel } from "./profile-interview-view.ts";
import { icon } from "./icons.ts";
import { driveInstrument } from "./instruments.ts";
import { readPanel } from "./readings.ts";
import {
  advanceProfileQuestion, cancelProfileDraft, initialProfileDraft, recoveryRecipients,
  reviewProfileAnswer,
} from "./profile-interview-controller.ts";

const app = document.querySelector<HTMLElement>("#app")!;
let snapshot: DesktopSnapshot | null = null;
let management: ManagementSnapshot | null = null;
let targets: TargetInventorySnapshot | null = null;
let updateState: UpdateUiState = { kind: "idle" };
let settingsSnapshot: SettingsSnapshot | null = null;
let onboardingState: OnboardingViewState = {
  step: 1,
  auth: { state: "checking" },
  draft: defaultOnboardingDraft(),
  message: "",
  submitting: false,
};
let setupUnlocked = false;
let setupCheckFailed = false;
let reconnectMode = false;
let profileState = initialProfileDraft();
const profileId = `profile-${crypto.randomUUID()}`;

/** Native macOS sheets. A browser confirm() would announce that this is a webview. */
async function confirmAction(prompt: string, title = "CommonKit"): Promise<boolean> {
  return ask(prompt, { title, kind: "warning", okLabel: "Confirm", cancelLabel: "Cancel" });
}

async function reportError(detail: string): Promise<void> {
  await message(detail, { title: "CommonKit", kind: "error" });
}

function placeholder(route: Route): string {
  const copy: Record<Route, [string, string]> = {
    onboarding: ["Get started", "Connect or create a GitHub-backed kit, then select this station's target and loadout."],
    status: ["Status", "Loading the local CommonKit service."],
    profile: ["My work profile", "Create and review personal context that is encrypted before synchronization."],
    plans: ["Changes", "Plans show semantic operations, provenance, risk, and the confirmation each one needs before apply."],
    credentials: ["Credentials", "CommonKit reports references and readiness here. Secret values never enter this window."],
    snapshots: ["Data", "Encrypted snapshot history, writer authority, restore plans, and promotions."],
    aboutMe: ["About Me", "The encrypted personal context available to this loadout and project."],
    relay: ["MCP connections", "Persistent relay health and authenticated upstream readiness."],
    schedule: ["Drift checks", "Read-only checks and notifications. Mutations stay explicitly confirmed."],
    diagnostics: ["Diagnostics", "Redacted component health and a schema-bound diagnostic bundle."],
    settings: ["Settings", "Autostart and signed update checks."],
  };
  const [heading, detail] = copy[route];
  return `<section><div class="placard"><h1>${heading}</h1></div><p class="brief">${detail}</p></section>`;
}

let lastShell = "";

function render(): void {
  // Live domain refreshes can arrive while a user is typing in a form. Preserve the
  // visible values before replacing the DOM so no keystroke is lost.
  captureOnboardingDraft();
  captureProfileDraft();
  const completion = reconnectMode ? "required" : setupUnlocked ? "complete" : setupCheckFailed ? "required" : setupCompletion(snapshot?.status ?? null, targets);
  const setupComplete = completion === "complete";
  const route = routeForSetup(routeFromHash(location.hash), setupComplete);
  const visibleNavigation = navigationForSetup(setupComplete);
  const body = completion === "checking"
    ? `<section><div class="placard"><h1>Checking this computer</h1><span>First run</span></div><p class="brief">CommonKit is checking whether setup is already complete.</p></section>`
    : route === "onboarding" ? onboardingPanel(onboardingState)
    : route === "profile" ? profileInterviewPanel(profileState)
    : route === "settings" ? updatePanel(updateState, settingsSnapshot)
    : route === "status" && snapshot && targets ? statusPanel(snapshot, targets, management, settingsSnapshot)
    : management && route in management ? managementPanel(route, management)
    : placeholder(route);

  const groups = visibleNavigation.reduce<Record<string, typeof visibleNavigation[number][]>>((result, item) => {
    (result[item.group] ??= []).push(item);
    return result;
  }, {});
  const navigation = Object.entries(groups).map(([group, items]) =>
    `<section class="rail-group"><p>${group}</p>${items.map(({ route: id, label }) => `<a href="#${id}"${route === id ? ' aria-current="page"' : ""}>${label}</a>`).join("")}</section>`
  ).join("");

  const shell = `<aside class="rail">
      <div class="mark">${icon.mark()}<b>CommonKit</b></div>
      <nav aria-label="CommonKit">${navigation}</nav>
      <p class="rail-foot">${snapshot ? `v${snapshot.status.runtimeVersion}` : "service offline"}</p>
    </aside><main class="deck">${body}</main>`;

  // Skipping an identical rewrite keeps focus, scroll, selection and needle motion
  // alive across the two-second poll.
  if (shell !== lastShell) {
    app.innerHTML = shell;
    lastShell = shell;
    if (route === "onboarding") bindOnboardingActions();
    else if (route === "profile") bindProfileActions();
    else if (route === "settings") bindUpdateActions();
    else if (route === "status") bindTargetActions();
    else if (management && route in management) bindManagementActions(route);
  }
  driveInstruments();
}

function driveInstruments(): void {
  if (!app.querySelector("[data-instrument]")) return;
  for (const [spec, reading] of readPanel(snapshot, management, targets)) driveInstrument(app, spec, reading);
}

/** A background poll must never yank the field the user is typing into. */
function isEditing(): boolean {
  const active = document.activeElement;
  return active instanceof HTMLInputElement || active instanceof HTMLTextAreaElement || active instanceof HTMLSelectElement;
}

function captureProfileDraft(): void {
  const answer = document.querySelector<HTMLInputElement | HTMLTextAreaElement>("#profile-answer");
  if (answer) profileState.pendingValue = answer.value;
  const recipients = document.querySelector<HTMLTextAreaElement>("#profile-recipients");
  if (recipients) profileState.recipientsText = recipients.value;
}

function bindProfileActions(): void {
  document.querySelector<HTMLButtonElement>("#profile-cancel")?.addEventListener("click", () => {
    cancelProfileDraft(profileState);
    profileState = initialProfileDraft();
    profileState.message = "Draft discarded. No profile values were written.";
    render();
  });
  document.querySelector<HTMLButtonElement>("#profile-skip")?.addEventListener("click", () => {
    advanceProfileQuestion(profileState, false);
    render();
  });
  document.querySelector<HTMLFormElement>("#profile-interview-form")?.addEventListener("submit", (event) => {
    event.preventDefault();
    const value = new FormData(event.currentTarget as HTMLFormElement).get("answer");
    reviewProfileAnswer(profileState, typeof value === "string" ? value : "");
    render();
  });
  document.querySelector<HTMLButtonElement>("#profile-confirm")?.addEventListener("click", () => {
    captureProfileDraft();
    advanceProfileQuestion(profileState, true);
    render();
  });
  document.querySelector<HTMLFormElement>("#profile-recovery-form")?.addEventListener("submit", async (event) => {
    event.preventDefault();
    captureProfileDraft();
    const recipients = recoveryRecipients(profileState.recipientsText);
    if (recipients.length < 3) {
      profileState.message = "Add three distinct public recipients: device, offline recovery, and password manager.";
      render();
      return;
    }
    profileState.submitting = true;
    profileState.message = "Encrypting this revision locally.";
    render();
    try {
      await desktopApi.encryptProfileRevision({
        binding: {
          schemaVersion: 1,
          profileId,
          profileSchemaId: "commonkit-work-profile",
          profileSchemaVersion: 1,
          revisionId: `revision-${crypto.randomUUID()}`,
          parentHashes: [],
        },
        recipients,
        fields: Object.fromEntries(profileState.answers),
      });
      profileState.answers.clear();
      profileState.recipientsText = "";
      profileState.encrypted = true;
      profileState.message = "Encrypted revision staged for synchronization.";
    } catch (error) {
      profileState.message = errorMessage(error);
    } finally {
      profileState.submitting = false;
      render();
    }
  });
}

function bindTargetActions(): void {
  document.querySelector<HTMLButtonElement>("#save-target-selection")?.addEventListener("click", async () => {
    const selected = [...document.querySelectorAll<HTMLInputElement>('input[name="managed-target"]:checked')].map((input) => input.value);
    if (!await confirmAction(`Run read-only checks for ${selected.length} station(s)? Changes still need their own confirmation, per station.`)) return;
    try {
      targets = await desktopApi.selectTargets(selected, confirmationId("target-select"));
    } catch (error) {
      await reportError(errorMessage(error));
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
      onboardingState.message = status.state === "authenticated" ? "GitHub connected. Your kit repository will be private." : "GitHub sign-in was not completed.";
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
  document.querySelector<HTMLInputElement>("#safe-test-folder")?.addEventListener("change", (event) => {
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
    if (setupCompletion(snapshot?.status ?? null, targets) === "complete") {
      setupUnlocked = true;
      location.hash = "#status";
      render();
      return;
    }
    onboardingState.draft.publishRegistration = new FormData(form).get("publishRegistration") === "true";
    onboardingState.submitting = true;
    onboardingState.message = "Checking the setup and preparing a plan.";
    render();
    try {
      const result = await withDeadline(
        desktopApi.onboardingInitialize(onboardingRequest(onboardingState.draft, onboardingState.auth.login)),
        45_000,
        "Setup is taking too long. CommonKit stopped waiting for a response. Do not retry immediately; reopen CommonKit and check Diagnostics first.",
      );
      const plan = (result as { firstPlanId?: string }).firstPlanId ?? "ready";
      onboardingState.message = `Plan ${plan} is ready. Nothing has been applied to the managed root.`;
      setupUnlocked = true;
      reconnectMode = false;
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
  document.querySelector<HTMLButtonElement>("#autostart-enable")?.addEventListener("click", async () => {
    await updateAutostart(true);
  });
  document.querySelector<HTMLButtonElement>("#autostart-disable")?.addEventListener("click", async () => {
    await updateAutostart(false);
  });
  document.querySelector<HTMLButtonElement>("#reconnect-setup")?.addEventListener("click", () => {
    reconnectMode = true;
    onboardingState.step = 1;
    location.hash = "#onboarding";
    render();
  });
  document.querySelector<HTMLButtonElement>("#check-update")?.addEventListener("click", async () => {
    updateState = { kind: "checking" };
    render();
    updateState = await checkForDesktopUpdate(desktopApi);
    render();
  });
  document.querySelector<HTMLButtonElement>("#install-update")?.addEventListener("click", async () => {
    if (updateState.kind !== "available") return;
    const update = updateState.update;
    if (!await confirmAction(`Install signed CommonKit ${update.version}?`)) return;
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

async function updateAutostart(enabled: boolean): Promise<void> {
  try {
    const result = await desktopApi.setAutostart(enabled);
    if (settingsSnapshot) settingsSnapshot.autostart = result;
  } catch (error) {
    updateState = { kind: "error", message: errorMessage(error) };
  }
  render();
}

function confirmationId(action: string): string {
  return `desktop-${action}-${Date.now()}`;
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
  document.querySelector<HTMLFormElement>("#about-me-setup")?.addEventListener("submit", async (event) => {
    event.preventDefault();
    const values = new FormData(event.currentTarget as HTMLFormElement);
    const answers = {
      name: String(values.get("about-me-name") ?? "").trim(),
      explanationStyle: String(values.get("about-me-explanations") ?? "").trim(),
      decisionStyle: String(values.get("about-me-decisions") ?? "").trim(),
      tools: String(values.get("about-me-tools") ?? "").trim(),
      constraints: String(values.get("about-me-constraints") ?? "").trim(),
      neverAssume: String(values.get("about-me-never-assume") ?? "").trim(),
    };
    const review = Object.values(answers).filter(Boolean).map((value) => `• ${value}`).join("\n");
    if (!review || !await confirmAction(`CommonKit will remember:\n\n${review}\n\nSave this encrypted profile?`)) return;
    void showResult(route, async () => {
      await desktopApi.aboutMeSetup(answers, true);
      return desktopApi.managementSnapshot().then((value) => value.aboutMe);
    });
  });
  document.querySelectorAll<HTMLButtonElement>("[data-about-me-decision]").forEach((button) => {
    button.addEventListener("click", async () => {
      const suggestionId = button.dataset.aboutMeId;
      const decision = button.dataset.aboutMeDecision;
      if (!suggestionId || (decision !== "accept" && decision !== "reject")) return;
      const verb = decision === "accept" ? "Remember this" : "Forget and stop suggesting this";
      if (!await confirmAction(`${verb}?`)) return;
      void showResult(route, async () => {
        await desktopApi.aboutMeDecide(suggestionId, decision, confirmationId(`about-me-${decision}`));
        return desktopApi.managementSnapshot().then((value) => value.aboutMe);
      });
    });
  });
  document.querySelector("#plan-sync")?.addEventListener("click", async () => {
    try {
      const targetId = convergenceTarget(targets);
      if (!await confirmAction(`Stage providers and create a plan for ${targetId}? This reads state and writes nothing.`)) return;
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
  document.querySelector("#apply-plan")?.addEventListener("click", async () => {
    const planId = controlValue("plan-id");
    if (!planId) return;
    try {
      const targetId = convergenceTarget(targets);
      if (!await confirmAction(`Apply reviewed plan ${planId} to ${targetId}? This changes the managed root.`)) return;
      void showResult(route, () => desktopApi.applyPlan(targetId, planId, confirmationId("apply-plan")));
    } catch (error) {
      showError(route, error);
    }
  });
  document.querySelector("#snapshot-create")?.addEventListener("click", async () => {
    const databaseId = controlValue("database-id");
    if (!databaseId || !await confirmAction(`Create an encrypted snapshot of ${databaseId}?`)) return;
    void showResult(route, () => desktopApi.snapshotCreate(databaseId, confirmationId("snapshot-create")));
  });
  document.querySelector("#snapshot-restore")?.addEventListener("click", async () => {
    const snapshotId = controlValue("snapshot-id");
    if (!snapshotId || !await confirmAction(`Restore ${snapshotId}? The current database is backed up first.`)) return;
    void showResult(route, () => desktopApi.snapshotRestore(snapshotId, confirmationId("snapshot-restore")));
  });
  document.querySelector("#snapshot-promote")?.addEventListener("click", async () => {
    const databaseId = controlValue("database-id");
    const targetId = databaseId ? controlValue("promotion-target") : null;
    if (!databaseId || !targetId || !await confirmAction(`Promote ${targetId} as writer for ${databaseId}?`)) return;
    void showResult(route, () => desktopApi.snapshotPromote(databaseId, targetId, confirmationId("snapshot-promote")));
  });
  document.querySelector("#relay-restart")?.addEventListener("click", async () => {
    if (!await confirmAction("Restart the managed relay runtime?")) return;
    void showResult(route, () => desktopApi.relayRestart(confirmationId("relay-restart")));
  });
  document.querySelector("#schedule-enable")?.addEventListener("click", async () => {
    const raw = controlValue("schedule-interval");
    const interval = raw ? Number(raw) : 0;
    if (!Number.isSafeInteger(interval) || interval < 1) return;
    if (!await confirmAction(`Enable read-only drift checks every ${interval} seconds?`)) return;
    void showResult(route, () => desktopApi.scheduleConfigure(true, interval, confirmationId("schedule-enable")));
  });
  document.querySelector("#schedule-disable")?.addEventListener("click", async () => {
    if (!await confirmAction("Disable scheduled drift checks?")) return;
    void showResult(route, () => desktopApi.scheduleConfigure(false, 1, confirmationId("schedule-disable")));
  });
  document.querySelector("#credential-readiness")?.addEventListener("click", () => {
    const reference = controlValue("credential-reference");
    if (reference) void showResult(route, () => desktopApi.credentialReadiness([reference]));
  });
  document.querySelector("#credential-apply")?.addEventListener("click", () => {
    const id = controlValue("credential-destination");
    if (!id) return;
    void showResult(route, async () => {
      const plan = await desktopApi.credentialPlan([id]);
      const operations = plan.operations
        .map((operation) => `${operation.action} ${operation.destinationId} at ${operation.path}`)
        .join("\n");
      if (!await confirmAction(`Credential plan ${plan.planId}:\n\n${operations}\n\nApply this exact plan? Secret values stay hidden.`)) {
        return { plan, applied: false };
      }
      return desktopApi.credentialApply(plan.planId, confirmationId("credential-apply"));
    });
  });
  document.querySelector("#credential-verify")?.addEventListener("click", () => {
    const id = controlValue("credential-destination");
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
  document.querySelector("#diagnostics-refresh")?.addEventListener("click", () => {
    void refreshLiveState();
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
void desktopApi.settingsSnapshot().then((value) => {
  settingsSnapshot = value;
  render();
}).catch(() => {
  settingsSnapshot = null;
});
async function initializeSetupGate(): Promise<void> {
  const [observed, inventory] = await Promise.allSettled([desktopApi.snapshot(), desktopApi.targets()]);
  if (observed.status === "fulfilled") snapshot = observed.value;
  if (inventory.status === "fulfilled") targets = inventory.value;
  setupCheckFailed = observed.status === "rejected" || inventory.status === "rejected";
  if (setupCheckFailed) onboardingState.message = "The CommonKit background service is unavailable. Restart CommonKit, then retry setup.";
  render();
}
void initializeSetupGate();
void checkForDesktopUpdate(desktopApi).then((state) => {
  updateState = state;
  render();
});
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
    // Instruments still track live state while a field has focus; only the
    // surrounding markup waits until the user is done typing.
    if (isEditing()) driveInstruments();
    else render();
  } catch { if (!isEditing()) render(); }
  finally { refreshRunning = false; }
}
void refreshLiveState();
// The panel stops polling when the window is not visible; a menu-bar app should not
// wake the daemon every two seconds while it sits behind other windows.
const liveRefreshTimer = window.setInterval(() => {
  if (document.visibilityState === "visible") void refreshLiveState();
}, 2_000);
addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible") void refreshLiveState();
});
addEventListener("beforeunload", () => window.clearInterval(liveRefreshTimer));
