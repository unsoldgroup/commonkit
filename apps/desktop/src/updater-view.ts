import { escapeHtml } from "./html.ts";

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

export function updatePanel(state: UpdateUiState, settings?: SettingsSnapshot | null): string {
  let action = '<button id="check-update" type="button">Check for updates</button>';
  if (state.kind === "checking") action = '<button type="button" disabled>Checking for updates…</button>';
  if (state.kind === "current") action = '<p class="update-current">CommonKit is up to date.</p><button id="check-update" type="button">Check again</button>';
  if (state.kind === "error") action = `<p class="update-error" role="alert">${escapeHtml(state.message)}</p><button id="check-update" type="button">Try again</button>`;
  if (state.kind === "available" || state.kind === "installing") {
    const notes = state.update.notes ? `<pre class="release-notes">${escapeHtml(state.update.notes)}</pre>` : '<p class="release-notes">No release notes were provided.</p>';
    const published = state.update.publishedAt ? `<p class="update-date">Published ${escapeHtml(state.update.publishedAt)}</p>` : "";
    action = `<div class="update-available"><h2>Version ${escapeHtml(state.update.version)}</h2><p>Installed: ${escapeHtml(state.update.currentVersion)}</p>${published}<h3>Release notes</h3>${notes}<button id="install-update" type="button"${state.kind === "installing" ? " disabled" : ""}>${state.kind === "installing" ? "Installing…" : "Install update"}</button></div>`;
  }
  const computer = settings
    ? `<div class="settings-section"><h2>This computer</h2><dl><dt>Kit repository</dt><dd>${escapeHtml(settings.repository ?? "Not connected")}</dd><dt>Managed root</dt><dd>${escapeHtml(settings.targetRoot ?? "Not configured")}</dd><dt>Configuration</dt><dd>${escapeHtml(settings.configDirectory)}</dd><dt>Local state</dt><dd>${escapeHtml(settings.stateDirectory)}</dd></dl><div class="setting-row"><div><strong>Launch at login</strong><p>Keep status and scheduled checks available after you sign in.</p></div><button id="autostart-${settings.autostart ? "disable" : "enable"}" type="button">${settings.autostart ? "Disable" : "Enable"}</button></div></div>`
    : `<div class="readiness readiness-unavailable"><span>Checking</span><h2>This computer</h2><p>Loading local configuration and launch settings…</p></div>`;
  return `<section class="panel update-panel"><p class="eyebrow">System</p><h1>Settings</h1>${computer}<div class="settings-section"><h2>Signed updates</h2><p>Checking is read-only. Installation starts only after you explicitly confirm a specific version.</p>${action}</div><div class="settings-section"><h2>Reconnect or reset</h2><p>Reconnect this computer to another kit or remove its local registration without deleting the portable repository.</p><button id="reconnect-setup" type="button">Review connection options</button></div></section>`;
}
