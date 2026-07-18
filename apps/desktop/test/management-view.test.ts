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
