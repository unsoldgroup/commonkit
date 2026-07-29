import assert from "node:assert/strict";
import test from "node:test";
import { statusPanel } from "../src/status-panel.ts";

test("status answers whether the selected computer and its capabilities are ready", () => {
  const html = statusPanel(
    {
      status: {
        apiVersion: "v1", contractVersion: "1.0", schemaVersion: 1,
        runtimeVersion: "0.1.0", state: "healthy", activeTarget: null,
        activeLoadout: null, lastDriftCheckUnixMs: null, lastDriftErrorCode: null,
      },
      capabilities: [], lastEventId: null, events: [],
      gitSync: { state: "clean", revision: "abc123" },
      policy: { state: "ready", violations: [] },
    },
    { selected: ["al-macbook"], targets: [{ id: "al-macbook", identityDigest: "sha256:x", transport: { type: "local" } }] },
    {
      plans: {}, credentials: { credentials: [] },
      snapshots: { error: "snapshot_domain_unconfigured" },
      relay: { configured: false, servers: [] },
      schedule: { enabled: false, intervalSeconds: 900 },
      diagnostics: {},
    },
    {
      autostart: true, configDirectory: "/config", stateDirectory: "/state",
      repository: "https://github.com/al/commonkit.git", targetRoot: "/managed",
    },
  );

  assert.match(html, /Environment healthy/i);
  assert.match(html, /al-macbook/);
  assert.match(html, /github\.com\/al\/commonkit/);
  assert.match(html, /Git sync[\s\S]*clean/i);
  assert.match(html, /Credentials[\s\S]*Setup needed/i);
  assert.match(html, /Data[\s\S]*Setup needed/i);
  assert.match(html, /MCP connections[\s\S]*Setup needed/i);
  assert.doesNotMatch(html, /Multiple or not selected/);
});

test("status distinguishes unavailable service data from capability setup", () => {
  const html = statusPanel(
    {
      status: {
        apiVersion: "v1", contractVersion: "1.0", schemaVersion: 1,
        runtimeVersion: "0.1.0", state: "offline", activeTarget: null,
        activeLoadout: null, lastDriftCheckUnixMs: null, lastDriftErrorCode: null,
      },
      capabilities: [], lastEventId: null, events: [],
      gitSync: { state: "unavailable" },
      policy: { state: "unavailable", violations: [] },
    },
    { selected: ["al-macbook"], targets: [{ id: "al-macbook", identityDigest: "sha256:x", transport: { type: "local" } }] },
    null,
    null,
  );

  assert.match(html, /Changes[\s\S]*Unavailable/i);
  assert.match(html, /Data[\s\S]*Unavailable/i);
  assert.doesNotMatch(html, /Changes[\s\S]*Ready/i);
});
