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

export function updatePanel(state: UpdateUiState): string {
  let action = '<button id="check-update" type="button">Check for updates</button>';
  if (state.kind === "checking") action = '<button type="button" disabled>Checking for updates…</button>';
  if (state.kind === "current") action = '<p class="update-current">CommonKit is up to date.</p><button id="check-update" type="button">Check again</button>';
  if (state.kind === "error") action = `<p class="update-error" role="alert">${escapeHtml(state.message)}</p><button id="check-update" type="button">Try again</button>`;
  if (state.kind === "available" || state.kind === "installing") {
    const notes = state.update.notes ? `<pre class="release-notes">${escapeHtml(state.update.notes)}</pre>` : '<p class="release-notes">No release notes were provided.</p>';
    const published = state.update.publishedAt ? `<p class="update-date">Published ${escapeHtml(state.update.publishedAt)}</p>` : "";
    action = `<div class="update-available"><h2>Version ${escapeHtml(state.update.version)}</h2><p>Installed: ${escapeHtml(state.update.currentVersion)}</p>${published}<h3>Release notes</h3>${notes}<button id="install-update" type="button"${state.kind === "installing" ? " disabled" : ""}>${state.kind === "installing" ? "Installing…" : "Install update"}</button></div>`;
  }
  return `<section class="panel update-panel"><p class="eyebrow">Settings</p><h1>Signed updates</h1><p>Checking is read-only. Installation starts only after you explicitly confirm a specific version.</p>${action}</section>`;
}
