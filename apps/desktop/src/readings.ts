/* Maps live daemon state onto the six instruments.
 *
 * Channels the daemon publishes today: relay.servers, plans.operations, status.state.
 * Channels the six-pack is wired for but the daemon does not expose yet:
 * kit.skills, kit.tools, relay.sessions. Those instruments stay unpowered until the
 * service publishes them; none of them guesses. */

import type { DesktopSnapshot, ManagementSnapshot } from "./contracts.ts";
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

export function readPanel(
  snapshot: DesktopSnapshot | null,
  management: ManagementSnapshot | null,
): Array<[InstrumentSpec, Reading]> {
  const relay = record(management?.relay);
  return SIX_PACK.map((spec) => {
    switch (spec.id) {
      case "servers":
        return [spec, countReading(management ? counted(relay, "servers", "upstreams") : null, spec.fullScale ?? 1, "MCP servers")];
      case "drift":
        return [spec, driftReading(snapshot)];
      case "plan":
        return [spec, planReading(management, spec.fullScale ?? 1)];
      case "skills":
        return [spec, noSignal("Skills")];
      case "tools":
        return [spec, noSignal("Tools")];
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
  return [
    { label: registered ? "Station registered" : "Not registered", state: registered ? "live" : "caution" },
    capabilityLamp("Changes", management?.plans, available, (value) => typeof record(value.plan ?? value.currentPlan ?? value).id === "string"),
    capabilityLamp("Credentials", management?.credentials, available, (value) => list(value.credentials ?? value.references).length > 0),
    capabilityLamp("Data", management?.snapshots, available, () => true),
    capabilityLamp("MCP connections", management?.relay, available, (value) => list(value.upstreams ?? value.servers).length > 0),
    hasPlan && applying
      ? { label: "Applying", state: "caution" }
      : { label: "Not applied", state: blocked ? "alert" : "caution" },
  ];
}
