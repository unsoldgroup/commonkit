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

export type PanelChannelState = "available" | "empty" | "unchecked" | "unavailable" | "stale";
export interface PanelChannel {
  state: PanelChannelState;
  value?: number;
  source: string;
  observedAtUnixMs: number;
  reason?: string;
}
export interface PanelSnapshot {
  contractVersion: "commonkit.panel/v1";
  observedAtUnixMs: number;
  channels: Record<string, PanelChannel>;
}

export interface CapabilityState { id: string; ready: boolean; detail: string; }
export interface DesktopSnapshot {
  status: ServiceStatus;
  panel?: PanelSnapshot;
  capabilities: CapabilityState[];
  lastEventId: number | null;
  events: Array<{ id?: number; event?: string; data?: unknown }>;
  gitSync: { state?: string; branch?: string; revision?: string; upstreamRevision?: string; fetched?: boolean };
  policy: { state?: string; violations?: Array<{ code?: string; detail?: string }> };
}

export interface ManagementSnapshot {
  aboutMe: unknown;
  plans: unknown;
  credentials: unknown;
  snapshots: unknown;
  relay: unknown;
  schedule: unknown;
  diagnostics: unknown;
  skills?: unknown;
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
