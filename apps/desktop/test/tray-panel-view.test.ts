import assert from "node:assert/strict";
import test from "node:test";
import { trayPanelMarkup } from "../src/tray-panel-view.ts";
import { TRAY_PACK } from "../src/instruments.ts";
import { readPanel } from "../src/readings.ts";

const status = {
  apiVersion: "v1", contractVersion: "1.0", schemaVersion: 1, runtimeVersion: "0.1.0",
  state: "healthy" as const, activeTarget: "al-macbook", activeLoadout: null,
  lastDriftCheckUnixMs: null, lastDriftErrorCode: null,
};
const snapshot = {
  status, capabilities: [], lastEventId: null, events: [],
  gitSync: { state: "clean" }, policy: { state: "ready", violations: [] },
};
const targets = {
  selected: ["al-macbook"],
  targets: [{ id: "al-macbook", identityDigest: "sha256:x", transport: { type: "local" as const } }],
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
