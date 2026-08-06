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
      panel: {
        contractVersion: "commonkit.panel/v1", observedAtUnixMs: 100,
        channels: {
          skills: { state: "empty", value: 0, source: "skillInventory", observedAtUnixMs: 100 },
          mcpServers: { state: "empty", value: 0, source: "relayStatus", observedAtUnixMs: 100 },
          changes: { state: "empty", value: 0, source: "planStore", observedAtUnixMs: 100 },
          credentials: { state: "empty", value: 0, source: "credentialReferences", observedAtUnixMs: 100 },
          agentSessions: { state: "unavailable", source: "sessionBoardReporter", observedAtUnixMs: 100, reason: "session_source_unconfigured" },
          drift: { state: "unchecked", source: "driftScheduler", observedAtUnixMs: 100, reason: "never_checked" },
        },
      },
      capabilities: [], lastEventId: null, events: [],
      gitSync: { state: "clean", revision: "abc123" },
      policy: { state: "ready", violations: [] },
    },
    { selected: ["local-workstation"], targets: [{ id: "local-workstation", identityDigest: "sha256:x", transport: { type: "local" } }] },
    {
      aboutMe: { error: "about_me_domain_unconfigured" },
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

  assert.doesNotMatch(html, /Environment healthy/i);
  assert.match(html, /Environment unchecked/i);
  assert.match(html, /UNCHECKED/i);
  assert.match(html, /local-workstation/);
  assert.match(html, /github\.com\/al\/commonkit/);
  assert.match(html, /Git sync[\s\S]*clean/i);
  assert.match(html, /Credentials[\s\S]*None configured/i);
  assert.match(html, /Data[\s\S]*Setup needed/i);
  assert.match(html, /MCP connections[\s\S]*None configured/i);
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
      panel: {
        contractVersion: "commonkit.panel/v1", observedAtUnixMs: 0,
        channels: Object.fromEntries(["skills", "mcpServers", "changes", "credentials", "agentSessions", "drift"].map((id) => [id, {
          state: "unavailable", source: "daemonPanel", observedAtUnixMs: 0, reason: "service_unavailable",
        }])),
      },
      capabilities: [], lastEventId: null, events: [],
      gitSync: { state: "unavailable" },
      policy: { state: "unavailable", violations: [] },
    },
    { selected: ["local-workstation"], targets: [{ id: "local-workstation", identityDigest: "sha256:x", transport: { type: "local" } }] },
    null,
    null,
  );

  assert.match(html, /Changes[\s\S]*Unavailable/i);
  assert.match(html, /Data[\s\S]*Unavailable/i);
  assert.doesNotMatch(html, /Changes[\s\S]*Ready/i);
});
