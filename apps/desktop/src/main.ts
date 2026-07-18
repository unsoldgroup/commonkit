import "./style.css";
import { desktopApi } from "./api.ts";
import { navigation, routeFromHash, type Route } from "./navigation.ts";
import { statusView } from "./view-model.ts";
import type { DesktopSnapshot } from "./contracts.ts";
import { updatePanel, type UpdateUiState } from "./updater-view.ts";

const app = document.querySelector<HTMLElement>("#app")!;
let snapshot: DesktopSnapshot | null = null;
let updateState: UpdateUiState = { kind: "idle" };

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
  const body = route === "settings" ? updatePanel(updateState) : route === "status" && snapshot ? (() => {
    const view = statusView(snapshot.status);
    return `<section class="panel"><p class="eyebrow">Local target</p><h1>${view.heading}</h1><p>${view.detail}</p><dl><dt>Target</dt><dd>${snapshot.status.activeTarget ?? "Not selected"}</dd><dt>Loadout</dt><dd>${snapshot.status.activeLoadout ?? "Not selected"}</dd><dt>Runtime</dt><dd>${snapshot.status.runtimeVersion}</dd></dl>${view.primaryRoute !== "status" ? `<a class="primary" href="#${view.primaryRoute}">Continue</a>` : ""}</section>`;
  })() : placeholder(route);
  app.innerHTML = `<aside><div class="brand">CommonKit</div><nav>${navigation.map(({ route: id, label }) => `<a class="${route === id ? "active" : ""}" href="#${id}">${label}</a>`).join("")}</nav></aside><main>${body}</main>`;
  if (route === "settings") bindUpdateActions();
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

addEventListener("hashchange", render);
render();
desktopApi.snapshot().then((value) => { snapshot = value; render(); }).catch(() => render());
