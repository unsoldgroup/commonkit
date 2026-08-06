import assert from "node:assert/strict";
import test from "node:test";
import { trayPanelMarkup } from "../src/tray-panel-view.ts";
import { TRAY_PACK } from "../src/instruments.ts";
import { readPanel } from "../src/readings.ts";

const status = {
  apiVersion: "v1", contractVersion: "1.0", schemaVersion: 1, runtimeVersion: "0.1.0",
  state: "healthy" as const, activeTarget: "local-workstation", activeLoadout: null,
  lastDriftCheckUnixMs: null, lastDriftErrorCode: null,
};
const snapshot = {
  status, capabilities: [], lastEventId: null, events: [],
  gitSync: { state: "clean" }, policy: { state: "ready", violations: [] },
};
const targets = {
  selected: ["local-workstation"],
  targets: [{ id: "local-workstation", identityDigest: "sha256:x", transport: { type: "local" as const } }],
};

test("the menu-bar panel carries the five instruments the owner asked for", () => {
  const html = trayPanelMarkup(snapshot, null, targets);

  for (const tag of ["Skills", "MCP Servers", "Plan", "Devices", "Agent Sessions"]) {
    assert.match(html, new RegExp(tag));
  }
  assert.match(html, /class="sixpack is-rail"/);
  assert.match(html, /data-action="open"/);
  assert.match(html, /data-action="quit"/);
});

test("devices reads the live target inventory while unpublished channels stay unpowered", () => {
  const readings = new Map(
    readPanel(snapshot, null, targets, TRAY_PACK).map(([spec, reading]) => [spec.id, reading]),
  );

  assert.equal(readings.get("devices")?.readout, "1/1");
  assert.equal(readings.get("devices")?.signal, "live");
  for (const unpublished of ["skills", "agents", "plan"]) {
    assert.equal(readings.get(unpublished)?.readout, "NO SIGNAL");
  }
});

test("skills use the canonical inventory count instead of showing no signal", () => {
  const management = {
    aboutMe: {}, plans: {}, credentials: {}, snapshots: {}, relay: {},
    schedule: {}, diagnostics: {},
    skills: [{ id: "review" }, { id: "research" }],
  };
  const readings = new Map(
    readPanel(snapshot, management, targets, TRAY_PACK).map(([spec, reading]) => [spec.id, reading]),
  );

  assert.equal(readings.get("skills")?.readout, "02");
  assert.equal(readings.get("skills")?.signal, "live");
});

test("an unanswered target inventory reads no signal rather than zero devices", () => {
  const readings = new Map(
    readPanel(snapshot, null, null, TRAY_PACK).map(([spec, reading]) => [spec.id, reading]),
  );

  assert.equal(readings.get("devices")?.readout, "NO SIGNAL");
});

test("panel instruments preserve daemon-owned empty, unchecked, and unavailable states", () => {
  const daemonPanel = {
    ...snapshot,
    panel: {
      contractVersion: "commonkit.panel/v1",
      observedAtUnixMs: 100,
      channels: {
        skills: { state: "available", value: 12, source: "skillInventory", observedAtUnixMs: 100 },
        mcpServers: { state: "empty", value: 0, source: "relayStatus", observedAtUnixMs: 100 },
        changes: { state: "empty", value: 0, source: "planStore", observedAtUnixMs: 100 },
        agentSessions: { state: "unavailable", source: "sessionBoardReporter", observedAtUnixMs: 100, reason: "session_source_unconfigured" },
        drift: { state: "unchecked", source: "driftScheduler", observedAtUnixMs: 100, reason: "never_checked" },
      },
    },
  } as const;

  const readings = new Map(
    readPanel(daemonPanel, null, targets, TRAY_PACK).map(([spec, reading]) => [spec.id, reading]),
  );

  assert.equal(readings.get("skills")?.readout, "12");
  assert.equal(readings.get("servers")?.readout, "00");
  assert.equal(readings.get("plan")?.readout, "NONE");
  assert.equal(readings.get("agents")?.readout, "UNAVAILABLE");
  assert.equal(readings.get("drift")?.readout, "UNCHECKED");
});
