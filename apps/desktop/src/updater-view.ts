import { escapeHtml } from "./html.ts";
import { icon } from "./icons.ts";

export type UpdateSummary = {
  currentVersion: string;
  version: string;
  notes: string | null;
  publishedAt: string | null;
};

export type UpdateUiState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "current" }
  | { kind: "available"; update: UpdateSummary }
  | { kind: "installing"; update: UpdateSummary }
  | { kind: "error"; message: string };

export interface SettingsSnapshot {
  autostart: boolean;
  configDirectory: string;
  stateDirectory: string;
  repository: string | null;
  targetRoot: string | null;
}

function plate(title: string, meta: string, body: string): string {
  return `<div class="plate-block"><div class="plate-head"><h2>${escapeHtml(title)}</h2><span>${escapeHtml(meta)}</span></div><div class="plate-body">${body}</div></div>`;
}

export function updatePanel(state: UpdateUiState, settings?: SettingsSnapshot | null): string {
  let updates: string;
  if (state.kind === "checking") {
    updates = plate("Signed updates", "Checking", `<p>Checking is read-only. Installation starts only after you confirm a specific version.</p><div class="controls"><button class="standby" type="button" disabled>Checking</button></div>`);
  } else if (state.kind === "current") {
    updates = plate("Signed updates", "Up to date", `<p>CommonKit is running the latest signed release.</p><div class="controls"><button class="standby" type="button" id="check-update">Check again</button></div>`);
  } else if (state.kind === "error") {
    updates = plate("Signed updates", "Failed", `<div class="notice alert" role="alert">${icon.alert()}<p>${escapeHtml(state.message)}</p></div><div class="controls" style="margin-top:14px"><button class="standby" type="button" id="check-update">Try again</button></div>`);
  } else if (state.kind === "available" || state.kind === "installing") {
    const { update } = state;
    const notes = update.notes
      ? `<pre class="log">${escapeHtml(update.notes)}</pre>`
      : `<p class="none-yet">No release notes were provided.</p>`;
    updates = plate(`Version ${update.version}`, update.publishedAt ? `Published ${update.publishedAt}` : "Signed", `
      <dl class="readout"><dt>Installed</dt><dd>${escapeHtml(update.currentVersion)}</dd><dt>Available</dt><dd>${escapeHtml(update.version)}</dd></dl>
      <p class="field" style="margin:18px 0 8px"><span>Release notes</span></p>${notes}
      <div class="controls" style="margin-top:16px"><button class="engage" type="button" id="install-update"${state.kind === "installing" ? " disabled" : ""}>${state.kind === "installing" ? "Installing" : "Install update"}</button></div>`);
  } else {
    updates = plate("Signed updates", "Not checked", `<p>Checking is read-only. Installation starts only after you confirm a specific version.</p><div class="controls"><button class="standby" type="button" id="check-update">Check for updates</button></div>`);
  }

  const computer = settings
    ? plate("This computer", settings.autostart ? "Launches at login" : "Manual start", `
        <dl class="readout">
          <dt>Kit repository</dt><dd${settings.repository ? "" : ' class="none"'}>${escapeHtml(settings.repository ?? "Not connected")}</dd>
          <dt>Managed root</dt><dd${settings.targetRoot ? "" : ' class="none"'}>${escapeHtml(settings.targetRoot ?? "Not configured")}</dd>
          <dt>Configuration</dt><dd>${escapeHtml(settings.configDirectory)}</dd>
          <dt>Local state</dt><dd>${escapeHtml(settings.stateDirectory)}</dd>
        </dl>
        <div class="controls" style="margin-top:16px"><button class="standby" type="button" id="autostart-${settings.autostart ? "disable" : "enable"}">${settings.autostart ? "Disable launch at login" : "Launch at login"}</button></div>`)
    : plate("This computer", "Loading", `<p class="none-yet">Reading local configuration and launch settings.</p>`);

  return `<section>
    <div class="placard"><h1>Settings</h1></div>
    <p class="brief">Autostart, signed updates, and how this station is connected to its kit.</p>
    ${computer}
    ${updates}
    ${plate("Reconnect or reset", "Non-destructive", `<p>Point this computer at another kit, or remove its local registration. The portable repository is not deleted.</p><div class="controls"><button class="standby" type="button" id="reconnect-setup">Review connection options</button></div>`)}
  </section>`;
}
