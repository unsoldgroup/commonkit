/* Maps the daemon-owned panel snapshot onto the six instruments. Legacy domain
 * responses remain as a compatibility fallback for older daemons; the current
 * contract preserves empty, unchecked, unavailable, and stale as distinct states. */

import type { DesktopSnapshot, ManagementSnapshot, PanelChannel, TargetInventorySnapshot } from "./contracts.ts";
import { SIX_PACK, countReading, noSignal, type InstrumentSpec, type Reading } from "./instruments.ts";

type RecordValue = Record<string, unknown>;

function record(value: unknown): RecordValue {
  return value && typeof value === "object" && !Array.isArray(value) ? value as RecordValue : {};
}

function list(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

/** Count a collection only when the domain actually answered; an error is not a zero. */
function counted(domain: RecordValue, ...keys: string[]): number | null {
  if (typeof domain.error === "string") return null;
  for (const key of keys) {
    if (Array.isArray(domain[key])) return (domain[key] as unknown[]).length;
  }
  return null;
}

function driftReading(snapshot: DesktopSnapshot | null): Reading {
  if (!snapshot) return noSignal("Drift");
  const channel = snapshot.panel?.channels.drift;
  if (channel) {
    if (channel.state === "unchecked") {
      return { fraction: 0, readout: "UNCHECKED", signal: "cold", spoken: "Drift has never been checked." };
    }
    if (channel.state === "unavailable") return unavailableReading("Drift");
    if (channel.state === "stale") return staleReading("Drift");
    if (channel.state === "empty" || channel.value === 0) {
      return { fraction: 0, readout: "LEVEL", signal: "live", spoken: "The last drift check found no drift." };
    }
    return { fraction: .7, readout: "OFF", signal: "caution", spoken: "The last drift check found drift." };
  }
  const state = snapshot.status.state;
  if (state === "healthy") {
    return { fraction: 0, readout: "LEVEL", signal: "live", spoken: "Target matches its desired state." };
  }
  if (state === "applying") {
    return { fraction: .4, readout: "APPLY", signal: "caution", spoken: "An approved plan is being applied." };
  }
  if (state === "drifted") {
    return { fraction: .7, readout: "OFF", signal: "caution", spoken: "Target has drifted from its desired state." };
  }
  if (state === "offline") return noSignal("Drift");
  return { fraction: 1, readout: state.toUpperCase(), signal: "alert", spoken: `Service reports ${state}.` };
}

function unavailableReading(noun: string): Reading {
  return { fraction: 0, readout: "UNAVAILABLE", signal: "cold", spoken: `${noun} is unavailable.` };
}

function staleReading(noun: string): Reading {
  return { fraction: 0, readout: "STALE", signal: "caution", spoken: `${noun} data is stale.` };
}

function panelCountReading(channel: PanelChannel | undefined, fullScale: number, noun: string): Reading {
  if (!channel) return noSignal(noun);
  if (channel.state === "unavailable") return unavailableReading(noun);
  if (channel.state === "unchecked") {
    return { fraction: 0, readout: "UNCHECKED", signal: "cold", spoken: `${noun} has not been checked.` };
  }
  if (channel.state === "stale") return staleReading(noun);
  return countReading(channel.value ?? 0, fullScale, noun);
}

function planReading(management: ManagementSnapshot | null, fullScale: number): Reading {
  if (!management) return noSignal("Plan");
  const domain = record(management.plans);
  if (typeof domain.error === "string") return noSignal("Plan");
  const plan = record(domain.plan ?? domain.currentPlan ?? domain);
  if (typeof plan.id !== "string") {
    return { fraction: 0, readout: "NONE", signal: "cold", spoken: "No plan is waiting for review." };
  }
  const operations = list(plan.operations).length;
  return {
    fraction: Math.min(operations / fullScale, 1),
    readout: String(operations).padStart(2, "0"),
    signal: operations > 0 ? "caution" : "live",
    spoken: operations > 0
      ? `${operations} operations waiting for review. None have been applied.`
      : "A plan exists with no operations.",
  };
}

/** Stations answer for themselves: the inventory the service returned, nothing inferred. */
function devicesReading(targets: TargetInventorySnapshot | null, fullScale: number): Reading {
  if (!targets) return noSignal("Devices");
  const known = targets.targets.length;
  const selected = targets.selected.filter((id) => targets.targets.some((target) => target.id === id)).length;
  return {
    fraction: Math.min(known / fullScale, 1),
    readout: `${selected}/${known}`,
    signal: known > 0 ? "live" : "cold",
    spoken: known > 0 ? `${known} devices known, ${selected} selected.` : "No devices are registered.",
  };
}

export function readPanel(
  snapshot: DesktopSnapshot | null,
  management: ManagementSnapshot | null,
  targets: TargetInventorySnapshot | null = null,
  specs: readonly InstrumentSpec[] = SIX_PACK,
): Array<[InstrumentSpec, Reading]> {
  const relay = record(management?.relay);
  const panel = snapshot?.panel?.channels;
  return specs.map((spec) => {
    switch (spec.id) {
      case "devices":
        return [spec, panel?.devices
          ? panelCountReading(panel.devices, spec.fullScale ?? 1, "devices")
          : devicesReading(targets, spec.fullScale ?? 1)];
      case "servers":
        return [spec, panel?.mcpServers
          ? panelCountReading(panel.mcpServers, spec.fullScale ?? 1, "MCP servers")
          : countReading(management ? counted(relay, "servers", "upstreams") : null, spec.fullScale ?? 1, "MCP servers")];
      case "drift":
        return [spec, driftReading(snapshot)];
      case "plan":
        if (panel?.changes) {
          if (panel.changes.state === "unavailable") return [spec, unavailableReading("Changes")];
          if (panel.changes.state === "stale") return [spec, staleReading("Changes")];
          if (panel.changes.state === "empty") {
            return [spec, { fraction: 0, readout: "NONE", signal: "cold", spoken: "No plan is waiting for review." }];
          }
          return [spec, panelCountReading(panel.changes, spec.fullScale ?? 1, "operations")];
        }
        return [spec, planReading(management, spec.fullScale ?? 1)];
      case "skills":
        return [spec, panel?.skills
          ? panelCountReading(panel.skills, spec.fullScale ?? 1, "skills")
          : countReading(
          management
            ? Array.isArray(management.skills)
              ? management.skills.length
              : counted(record(management.skills), "skills", "items")
            : null,
          spec.fullScale ?? 1,
          "skills",
        )];
      case "agents":
        return [spec, panelCountReading(panel?.agentSessions, spec.fullScale ?? 1, "Agent sessions")];
      default:
        return [spec, noSignal("Agent sessions")];
    }
  });
}

export type LampState = "caution" | "alert" | "live" | "cold";
export interface Lamp { label: string; state: LampState; }

/** A domain that answered "unconfigured" needs setup; one that did not answer at all
 *  is unavailable. The overview must never present those two as the same thing. */
function capabilityLamp(name: string, domain: unknown, available: boolean, configured: (value: RecordValue) => boolean): Lamp {
  if (!available) return { label: `${name} unavailable`, state: "cold" };
  const value = record(domain);
  if (typeof value.error === "string") {
    return value.error.includes("unconfigured")
      ? { label: `${name} setup needed`, state: "caution" }
      : { label: `${name} unavailable`, state: "cold" };
  }
  return configured(value) ? { label: `${name} ready`, state: "live" } : { label: `${name} setup needed`, state: "caution" };
}

function panelLamp(name: string, channel: PanelChannel | undefined): Lamp {
  if (!channel || channel.state === "unavailable") return { label: `${name} unavailable`, state: "cold" };
  if (channel.state === "stale") return { label: `${name} stale`, state: "caution" };
  if (channel.state === "unchecked") return { label: `${name} unchecked`, state: "cold" };
  if (channel.state === "empty") return { label: `${name} none configured`, state: "cold" };
  return { label: `${name} ready`, state: "live" };
}

/** The lamps that carry CommonKit's promise, derived from real state only. */
export function annunciators(
  snapshot: DesktopSnapshot | null,
  management: ManagementSnapshot | null,
  registered: boolean,
): Lamp[] {
  const domain = record(management?.plans);
  const plan = record(domain.plan ?? domain.currentPlan ?? domain);
  const hasPlan = typeof plan.id === "string";
  const applying = snapshot?.status.state === "applying";
  const blocked = snapshot?.status.state === "blocked";
  const available = management !== null;
  const panel = snapshot?.panel?.channels;
  return [
    { label: registered ? "Station registered" : "Not registered", state: registered ? "live" : "caution" },
    panel?.changes
      ? panelLamp("Changes", panel.changes)
      : capabilityLamp("Changes", management?.plans, available, (value) => typeof record(value.plan ?? value.currentPlan ?? value).id === "string"),
    panel?.credentials
      ? panelLamp("Credentials", panel.credentials)
      : capabilityLamp("Credentials", management?.credentials, available, (value) => list(value.credentials ?? value.references).length > 0),
    capabilityLamp("Data", management?.snapshots, available, () => true),
    panel?.mcpServers
      ? panelLamp("MCP connections", panel.mcpServers)
      : capabilityLamp("MCP connections", management?.relay, available, (value) => list(value.upstreams ?? value.servers).length > 0),
    hasPlan && applying
      ? { label: "Applying", state: "caution" }
      : { label: "Not applied", state: blocked ? "alert" : "caution" },
  ];
}
