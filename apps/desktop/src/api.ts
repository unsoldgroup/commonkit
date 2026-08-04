import { invoke } from "@tauri-apps/api/core";
import type { DesktopSnapshot, ManagementSnapshot, TargetInventorySnapshot } from "./contracts.ts";
import type { SettingsSnapshot, UpdateSummary } from "./updater-view.ts";

export type GithubAuthStatus =
  | { state: "authenticated"; login: string; method: "githubCli" }
  | { state: "signedOut" };
export interface OnboardingDefaults { kitDirectory: string; targetRoot: string; computerName: string; }
export interface CredentialPlan {
  planId: string;
  operations: Array<{ destinationId: string; path: string; action: string; credential: "<redacted>" }>;
}
export interface ProfileEncryptionRequest {
  binding: {
    schemaVersion: number;
    profileId: string;
    profileSchemaId: string;
    profileSchemaVersion: number;
    revisionId: string;
    parentHashes: string[];
  };
  recipients: string[];
  fields: Record<string, string | null>;
}

export const desktopApi = {
  onboardingInitialize: (request: Record<string, unknown>) => invoke<unknown>("onboarding_initialize", { request }),
  githubAuthStatus: () => invoke<GithubAuthStatus>("github_auth_status"),
  githubAuthLogin: () => invoke<GithubAuthStatus>("github_auth_login"),
  onboardingDefaults: () => invoke<OnboardingDefaults>("onboarding_defaults"),
  settingsSnapshot: () => invoke<SettingsSnapshot>("desktop_settings_snapshot"),
  snapshot: () => invoke<DesktopSnapshot>("desktop_snapshot"),
  managementSnapshot: () => invoke<ManagementSnapshot>("desktop_management_snapshot"),
  aboutMeSetup: (answers: Record<string, string>, confirmed: boolean) =>
    invoke<unknown>("desktop_about_me_setup", { answers, confirmed }),
  aboutMeDecide: (suggestionId: string, decision: "accept" | "reject", confirmationId: string) =>
    invoke<unknown>("desktop_about_me_decide", { suggestionId, decision, confirmationId }),
  targets: () => invoke<TargetInventorySnapshot>("targets_list"),
  selectTargets: (targets: string[], confirmationId: string) => invoke<TargetInventorySnapshot>("targets_select", { targets, confirmationId }),
  planSync: (targetId: string, confirmationId: string) => invoke<unknown>("plan_sync", { targetId, confirmationId }),
  applyPlan: (targetId: string, planId: string, confirmationId: string) => invoke("apply_plan", { targetId, planId, confirmationId }),
  verify: (targetId: string) => invoke<unknown>("desktop_verify", { targetId }),
  snapshotCreate: (databaseId: string, confirmationId: string) => invoke<unknown>("snapshot_create", { databaseId, confirmationId }),
  snapshotRestore: (snapshotId: string, confirmationId: string) => invoke<unknown>("snapshot_restore", { snapshotId, confirmationId }),
  snapshotPromote: (databaseId: string, targetId: string, confirmationId: string) => invoke<unknown>("snapshot_promote", { databaseId, targetId, confirmationId }),
  relayRestart: (confirmationId: string) => invoke<unknown>("relay_restart", { confirmationId }),
  scheduleConfigure: (enabled: boolean, intervalSeconds: number, confirmationId: string) => invoke<unknown>("schedule_configure", { enabled, intervalSeconds, confirmationId }),
  credentialReadiness: (references: string[]) => invoke<unknown>("credential_readiness", { references }),
  credentialPlan: (destinationIds: string[]) => invoke<CredentialPlan>("credential_plan", { destinationIds }),
  credentialApply: (planId: string, confirmationId: string) => invoke<unknown>("credential_apply", { planId, confirmationId }),
  credentialVerify: (destinationIds: string[]) => invoke<unknown>("credential_verify", { destinationIds }),
  diagnosticsExport: () => invoke<unknown>("diagnostics_export"),
  encryptProfileRevision: (request: ProfileEncryptionRequest) =>
    invoke<unknown>("encrypt_profile_revision", { request }),
  setAutostart: (enabled: boolean) => invoke<boolean>("set_autostart", { enabled }),
  checkForUpdate: () => invoke<UpdateSummary | null>("check_for_update"),
  installUpdate: (expectedVersion: string, confirmed: boolean) => invoke<void>("install_update", { expectedVersion, confirmed }),
  showWindow: () => invoke<void>("show_main_window"),
};
