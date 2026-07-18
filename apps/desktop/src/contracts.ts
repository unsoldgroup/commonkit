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
