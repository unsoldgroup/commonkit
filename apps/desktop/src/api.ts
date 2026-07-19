import { invoke } from "@tauri-apps/api/core";
import type { DesktopSnapshot, ManagementSnapshot, TargetInventorySnapshot } from "./contracts.ts";
import type { UpdateSummary } from "./updater-view.ts";

export const desktopApi = {
  onboardingInitialize: (request: Record<string, unknown>) => invoke<unknown>("onboarding_initialize", { request }),
  snapshot: () => invoke<DesktopSnapshot>("desktop_snapshot"),
  managementSnapshot: () => invoke<ManagementSnapshot>("desktop_management_snapshot"),
  targets: () => invoke<TargetInventorySnapshot>("targets_list"),
  selectTargets: (targets: string[], confirmationId: string) => invoke<TargetInventorySnapshot>("targets_select", { targets, confirmationId }),
  planSync: (targetId: string, confirmationId: string) => invoke<unknown>("plan_sync", { targetId, confirmationId }),
  applyPlan: (targetId: string, planId: string, confirmationId: string) => invoke("apply_plan", { targetId, planId, confirmationId }),
  verify: (targetId: string) => invoke<unknown>("desktop_verify", { targetId }),
  snapshotCreate: (databaseId: string, confirmationId: string) => invoke<unknown>("snapshot_create", { databaseId, confirmationId }),
  snapshotRestore: (snapshotId: string, confirmationId: string) => invoke<unknown>("snapshot_restore", { snapshotId, confirmationId }),
  snapshotPromote: (databaseId: string, targetId: string, confirmationId: string) => invoke<unknown>("snapshot_promote", { databaseId, targetId, confirmationId }),
  relayReconcile: (request: unknown, confirmationId: string) => invoke<unknown>("relay_reconcile", { request, confirmationId }),
  relayRestart: (confirmationId: string) => invoke<unknown>("relay_restart", { confirmationId }),
  scheduleConfigure: (enabled: boolean, intervalSeconds: number, confirmationId: string) => invoke<unknown>("schedule_configure", { enabled, intervalSeconds, confirmationId }),
  credentialReadiness: (references: string[]) => invoke<unknown>("credential_readiness", { references }),
  credentialApply: (destinationIds: string[], confirmationId: string) => invoke<unknown>("credential_apply", { destinationIds, confirmationId }),
  credentialVerify: (destinationIds: string[]) => invoke<unknown>("credential_verify", { destinationIds }),
  diagnosticsExport: () => invoke<unknown>("diagnostics_export"),
  setAutostart: (enabled: boolean) => invoke<boolean>("set_autostart", { enabled }),
  checkForUpdate: () => invoke<UpdateSummary | null>("check_for_update"),
  installUpdate: (expectedVersion: string, confirmed: boolean) => invoke<void>("install_update", { expectedVersion, confirmed }),
  showWindow: () => invoke<void>("show_main_window"),
};
