import assert from "node:assert/strict";
import test from "node:test";
import { statusView } from "../src/view-model.ts";

test("offline service produces actionable, non-mutating quick actions", () => {
  const view = statusView({
    apiVersion: "v1", contractVersion: "1.0", schemaVersion: 1,
    runtimeVersion: "0.1.0", state: "offline", activeTarget: null,
    activeLoadout: null, lastDriftCheckUnixMs: null, lastDriftErrorCode: null,
  });
  assert.equal(view.heading, "Service offline");
  assert.equal(view.canMutate, false);
  assert.match(view.detail, /start CommonKit/i);
});

test("drift requires plan review before apply", () => {
  const view = statusView({
    apiVersion: "v1", contractVersion: "1.0", schemaVersion: 1,
    runtimeVersion: "0.1.0", state: "drifted", activeTarget: "laptop",
    activeLoadout: "personal", lastDriftCheckUnixMs: 10, lastDriftErrorCode: null,
  });
  assert.equal(view.heading, "Changes available");
  assert.equal(view.canMutate, true);
  assert.equal(view.primaryRoute, "plans");
});
