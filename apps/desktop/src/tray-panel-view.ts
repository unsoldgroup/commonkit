/* The menu-bar panel: the same instrument plate as the window, cut down to a popover.
 *
 * It reads; it does not act. The only two controls open the window or quit the app,
 * which is exactly what the native menu it replaces was allowed to do. */

import type { DesktopSnapshot, ManagementSnapshot, TargetInventorySnapshot } from "./contracts.ts";
import { escapeHtml } from "./html.ts";
import { TRAY_PACK, sixPackMarkup } from "./instruments.ts";
import { annunciators, readPanel } from "./readings.ts";
import { icon, lampIcon } from "./icons.ts";
import { statusView } from "./view-model.ts";

export function trayPanelMarkup(
  snapshot: DesktopSnapshot | null,
  management: ManagementSnapshot | null,
  targets: TargetInventorySnapshot | null,
): string {
  const heading = snapshot ? statusView(snapshot.status).heading : "Service offline";
  const registered = (targets?.targets.length ?? 0) > 0;
  const spoken = readPanel(snapshot, management, targets, TRAY_PACK)
    .map(([spec, reading]) => `${spec.tag}: ${reading.spoken}`)
    .join(" ");
  const lamps = annunciators(snapshot, management, registered)
    .map(({ label, state }) => `<p class="lamp" data-state="${state}">${lampIcon(state)}<span>${escapeHtml(label)}</span></p>`)
    .join("");

  return `<div class="tray-head">${icon.mark()}<b>CommonKit</b><span>${escapeHtml(heading)}</span></div>
    ${sixPackMarkup(TRAY_PACK, true)}
    <p class="sr-only" role="status" aria-live="polite">${escapeHtml(spoken)}</p>
    <div class="annunciator" aria-label="Panel warnings">${lamps}</div>
    <div class="controls">
      <button class="engage" type="button" data-action="open">Open CommonKit</button>
      <button class="standby" type="button" data-action="quit">Quit</button>
    </div>`;
}
