import { invoke } from "@tauri-apps/api/core";
import type { DesktopSnapshot } from "./contracts.ts";

export const desktopApi = {
  snapshot: () => invoke<DesktopSnapshot>("desktop_snapshot"),
  applyPlan: (planId: string, confirmationId: string) => invoke("apply_plan", { planId, confirmationId }),
  setAutostart: (enabled: boolean) => invoke<boolean>("set_autostart", { enabled }),
  checkForUpdate: () => invoke<string | null>("check_for_update"),
  showWindow: () => invoke<void>("show_main_window"),
};
