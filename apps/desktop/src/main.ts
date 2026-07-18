import "./style.css";
import { desktopApi } from "./api.ts";
import { navigation, routeFromHash, type Route } from "./navigation.ts";
import { statusView } from "./view-model.ts";
import type { DesktopSnapshot } from "./contracts.ts";

const app = document.querySelector<HTMLElement>("#app")!;
let snapshot: DesktopSnapshot | null = null;

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
  const body = route === "status" && snapshot ? (() => {
    const view = statusView(snapshot.status);
    return `<section class="panel"><p class="eyebrow">Local target</p><h1>${view.heading}</h1><p>${view.detail}</p><dl><dt>Target</dt><dd>${snapshot.status.activeTarget ?? "Not selected"}</dd><dt>Loadout</dt><dd>${snapshot.status.activeLoadout ?? "Not selected"}</dd><dt>Runtime</dt><dd>${snapshot.status.runtimeVersion}</dd></dl>${view.primaryRoute !== "status" ? `<a class="primary" href="#${view.primaryRoute}">Continue</a>` : ""}</section>`;
  })() : placeholder(route);
  app.innerHTML = `<aside><div class="brand">CommonKit</div><nav>${navigation.map(({ route: id, label }) => `<a class="${route === id ? "active" : ""}" href="#${id}">${label}</a>`).join("")}</nav></aside><main>${body}</main>`;
}

addEventListener("hashchange", render);
render();
desktopApi.snapshot().then((value) => { snapshot = value; render(); }).catch(() => render());
