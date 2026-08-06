import type { DesktopSnapshot, ManagementSnapshot, TargetInventorySnapshot } from "./contracts.ts";
import type { SettingsSnapshot } from "./updater-view.ts";
import { escapeHtml } from "./html.ts";
import { statusView } from "./view-model.ts";
import { sixPackMarkup } from "./instruments.ts";
import { annunciators, readPanel } from "./readings.ts";
import { lampIcon } from "./icons.ts";

export function annunciatorMarkup(
  snapshot: DesktopSnapshot | null,
  management: ManagementSnapshot | null,
  registered: boolean,
): string {
  const lamps = annunciators(snapshot, management, registered)
    .map(({ label, state }) => `<p class="lamp" data-state="${state}">${lampIcon(state)}<span>${escapeHtml(label)}</span></p>`)
    .join("");
  return `<div class="annunciator" aria-label="Panel warnings">${lamps}</div>`;
}

export function statusPanel(
  snapshot: DesktopSnapshot,
  targets: TargetInventorySnapshot,
  management: ManagementSnapshot | null,
  settings: SettingsSnapshot | null,
): string {
  const view = statusView(snapshot.status);
  const selected = targets.selected.filter((id) => targets.targets.some((target) => target.id === id));
  const station = selected.length === 1 ? selected[0]! : `${selected.length} stations selected`;
  const lastCheck = snapshot.status.lastDriftCheckUnixMs
    ? new Date(snapshot.status.lastDriftCheckUnixMs).toLocaleString()
    : "Never";
  // A wall of dials is not a screen-reader experience; the panel also reads aloud.
  const spoken = readPanel(snapshot, management, targets)
    .map(([spec, reading]) => `${spec.tag}: ${reading.spoken}`)
    .join(" ");

  const stations = targets.targets.length > 1
    ? `<div class="plate-block" style="margin-top:16px"><div class="plate-head"><h2>Stations</h2><span>${targets.targets.length} known</span></div><div class="plate-body">
        <p>Read-only checks run against the stations you select. Every change still needs its own confirmation, per station.</p>
        <ul class="stack">${targets.targets.map((target) => `<li><b><label class="toggle" style="margin:0"><input type="checkbox" name="managed-target" value="${escapeHtml(target.id)}" ${selected.includes(target.id) ? "checked" : ""}> ${escapeHtml(target.id)}</label></b><span>${target.transport.type === "ssh" ? `ssh ${escapeHtml(target.transport.user)}@${escapeHtml(target.transport.host)}:${target.transport.port}` : "local"}</span><small>${escapeHtml(target.identityDigest)}</small></li>`).join("")}</ul>
        <div class="controls" style="margin-top:14px"><button class="standby" type="button" id="save-target-selection">Run read-only checks</button></div>
      </div></div>`
    : "";

  return `<section>
    <div class="placard"><h1>${escapeHtml(view.heading)}</h1><span>${escapeHtml(station)}</span></div>
    <p class="brief">${escapeHtml(view.detail)}</p>
    ${sixPackMarkup()}
    <p class="hint" style="margin:10px 0 20px">Skills and agent sessions are unpowered: the local service does not publish those channels yet, so CommonKit shows no reading rather than a guess.</p>
    <p class="sr-only" role="status" aria-live="polite">${escapeHtml(spoken)}</p>
    ${annunciatorMarkup(snapshot, management, true)}
    <div class="controls" style="margin:16px 0 0"><a class="engage" href="#plans" style="display:grid;place-items:center;text-decoration:none">Review changes</a></div>
    <div class="plate-block" style="margin-top:18px">
      <div class="plate-head"><h2>Station</h2><span>${escapeHtml(snapshot.status.runtimeVersion)}</span></div>
      <div class="plate-body"><dl class="readout">
        <dt>Kit repository</dt><dd${settings?.repository ? "" : ' class="none"'}>${escapeHtml(settings?.repository ?? "Not connected")}</dd>
        <dt>Managed root</dt><dd${settings?.targetRoot ? "" : ' class="none"'}>${escapeHtml(settings?.targetRoot ?? "Not configured")}</dd>
        <dt>Loadout</dt><dd${snapshot.status.activeLoadout ? "" : ' class="none"'}>${escapeHtml(snapshot.status.activeLoadout ?? "None active")}</dd>
        <dt>Git sync</dt><dd>${escapeHtml(snapshot.gitSync.state ?? "unavailable")}${snapshot.gitSync.branch ? ` \u00b7 ${escapeHtml(snapshot.gitSync.branch)}` : ""}</dd>
        <dt>Last drift check</dt><dd${snapshot.status.lastDriftCheckUnixMs ? "" : ' class="none"'}>${escapeHtml(lastCheck)}</dd>
      </dl></div>
    </div>
    ${stations}
  </section>`;
}
