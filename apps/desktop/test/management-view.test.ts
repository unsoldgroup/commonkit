import assert from "node:assert/strict";
import test from "node:test";

import { managementPanel } from "../src/management-view.ts";

test("management panels render live domain state without secret values", () => {
  const html = managementPanel("credentials", {
    credentials: { credentials: [{ reference: "bws://shared/GITHUB_TOKEN", readiness: "ready" }] },
  });

  assert.match(html, /bws:\/\/shared\/GITHUB_TOKEN/);
  assert.match(html, /ready/);
  assert.doesNotMatch(html, /secretValue/);
});

test("management panels report unavailable domains as unavailable", () => {
  const html = managementPanel("snapshots", {
    snapshots: { error: "snapshot_domain_unconfigured" },
  });

  assert.match(html, /snapshot_domain_unconfigured/);
});

test("operator panels expose explicit fixed actions without rendering secret inputs", () => {
  assert.match(managementPanel("snapshots", { snapshots: { snapshots: [] } }), /id="snapshot-create"/);
  assert.match(managementPanel("relay", { relay: { state: "healthy" } }), /id="relay-restart"/);
  assert.match(managementPanel("schedule", { schedule: { enabled: false } }), /id="schedule-enable"/);
  assert.match(managementPanel("credentials", { credentials: { credentials: [] } }), /id="credential-apply"/);
  assert.match(managementPanel("diagnostics", { diagnostics: { status: "healthy" } }), /id="diagnostics-export"/);
  assert.doesNotMatch(managementPanel("credentials", { credentials: {} }), /type="password"/);
});

test("plan review is human-readable and selects a bound digest without raw JSON", () => {
  const html = managementPanel("plans", { plans: {
    plan: {
      id: `sha256:${"a".repeat(64)}`,
      risk: "high",
      operations: [{ kind: "write_file", path: ".config/tool.json", provenance: { layer: "team" } }],
    },
    operation: { state: "running", completedOperations: 1, totalOperations: 3 },
  } });
  assert.match(html, /High risk/i);
  assert.match(html, /write file/i);
  assert.match(html, /team/i);
  assert.match(html, /1 of 3/i);
  assert.match(html, /name="plan-id"/);
  assert.doesNotMatch(html, /<pre>/);
});

test("plan review renders the production Rust plan contract faithfully", () => {
  const digest = `sha256:${"a".repeat(64)}`;
  const html = managementPanel("plans", { plans: {
    schemaVersion: 1,
    contractVersion: "1.0.0",
    id: digest,
    targetId: "workstation",
    operations: [{
      id: digest,
      adapterId: "filesystem",
      kind: "update",
      resource: {
        resourceType: "file",
        resourceId: "editor-config",
        managedPath: ".config/editor.json",
      },
      risk: "high",
      requiresConfirmation: true,
      dependsOn: [],
      payloadDigest: digest,
      summary: "Update editor configuration",
    }],
  } });

  assert.match(html, /High risk/i);
  assert.match(html, /Update editor configuration/);
  assert.match(html, /\.config\/editor\.json/);
  assert.match(html, /filesystem/);
  assert.doesNotMatch(html, /\[object Object\]/);
  assert.doesNotMatch(html, /Unknown risk/i);
});

test("snapshot, relay, and diagnostics panels expose typed inventory controls", () => {
  const snapshots = managementPanel("snapshots", { snapshots: {
    authoritativeWriter: { databaseId: "catalog", targetId: "macbook" },
    snapshots: [{ id: "snap-1", databaseId: "catalog", createdAt: "2026-07-19T10:00:00Z" }],
  } });
  assert.match(snapshots, /Authoritative writer/i);
  assert.match(snapshots, /value="snap-1"/);
  assert.doesNotMatch(snapshots, /<pre>/);

  const relay = managementPanel("relay", { relay: {
    state: "healthy", upstreams: [{ id: "docs", state: "ready", transport: "stdio" }],
  } });
  assert.match(relay, /docs/);
  assert.match(relay, /ready/);

  const diagnostics = managementPanel("diagnostics", { diagnostics: {
    adapters: [{ id: "filesystem", state: "healthy", detail: "3 managed paths" }],
  } });
  assert.match(diagnostics, /filesystem/);
  assert.match(diagnostics, /3 managed paths/);
});
