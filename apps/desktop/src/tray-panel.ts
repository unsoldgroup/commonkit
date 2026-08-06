/* Menu-bar panel process. Polls the same read-only snapshots as the window and
 * drives the instruments; the only writes it can make are "open" and "quit". */

import "./style.css";
import { desktopApi } from "./api.ts";
import type { DesktopSnapshot, ManagementSnapshot, TargetInventorySnapshot } from "./contracts.ts";
import { TRAY_PACK, driveInstrument } from "./instruments.ts";
import { readPanel } from "./readings.ts";
import { trayPanelMarkup } from "./tray-panel-view.ts";

const panel = document.querySelector<HTMLElement>("#panel")!;
let snapshot: DesktopSnapshot | null = null;
let management: ManagementSnapshot | null = null;
let targets: TargetInventorySnapshot | null = null;

function render(): void {
  panel.innerHTML = trayPanelMarkup(snapshot, management, targets);
  for (const [spec, reading] of readPanel(snapshot, management, targets, TRAY_PACK)) {
    driveInstrument(panel, spec, reading);
  }
}

/** A channel that fails keeps its last honest reading rather than reporting a zero. */
async function refresh(): Promise<void> {
  const [nextSnapshot, nextManagement, nextTargets] = await Promise.allSettled([
    desktopApi.snapshot(), desktopApi.managementSnapshot(), desktopApi.targets(),
  ]);
  if (nextSnapshot.status === "fulfilled") snapshot = nextSnapshot.value;
  if (nextManagement.status === "fulfilled") management = nextManagement.value;
  if (nextTargets.status === "fulfilled") targets = nextTargets.value;
  render();
}

panel.addEventListener("click", (event) => {
  const action = (event.target as HTMLElement).closest<HTMLElement>("[data-action]")?.dataset.action;
  if (action === "open") void desktopApi.showWindow();
  if (action === "quit") void desktopApi.quit();
});

render();
void refresh();
setInterval(() => void refresh(), 5_000);
