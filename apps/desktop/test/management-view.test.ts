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
