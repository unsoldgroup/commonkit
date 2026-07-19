export type OverallState = "healthy" | "drifted" | "blocked" | "applying" | "degraded" | "offline";

export interface ServiceStatus {
  apiVersion: string;
  contractVersion: string;
  schemaVersion: number;
  runtimeVersion: string;
  state: OverallState;
  activeTarget: string | null;
  activeLoadout: string | null;
  lastDriftCheckUnixMs: number | null;
  lastDriftErrorCode: string | null;
}

export interface CapabilityState { id: string; ready: boolean; detail: string; }
export interface DesktopSnapshot {
  status: ServiceStatus;
  capabilities: CapabilityState[];
  lastEventId: number | null;
}

export interface ManagementSnapshot {
  plans: unknown;
  credentials: unknown;
  snapshots: unknown;
  relay: unknown;
  schedule: unknown;
  diagnostics: unknown;
}

export interface TargetRecord {
  id: string;
  transport: { type: "local" } | { type: "ssh"; host: string; user: string; port: number };
  identityDigest: string;
}

export interface TargetInventorySnapshot {
  targets: TargetRecord[];
  selected: string[];
}
