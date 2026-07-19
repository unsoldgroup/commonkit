import assert from "node:assert/strict";
import test from "node:test";
import { navigation, navigationForSetup, routeForSetup, routeFromHash } from "../src/navigation.ts";

test("management navigation covers every v1 operator workflow", () => {
  assert.deepEqual(navigation.map((item) => item.route), [
    "onboarding", "status", "plans", "credentials", "snapshots",
    "relay", "schedule", "diagnostics", "settings",
  ]);
});

test("unconfigured first run exposes only the setup wizard", () => {
  assert.deepEqual(navigationForSetup(false).map((item) => item.route), ["onboarding"]);
  assert.equal(routeForSetup("status", false), "onboarding");
  assert.equal(routeForSetup("plans", false), "onboarding");
  assert.equal(routeForSetup("onboarding", false), "onboarding");
});

test("completing setup unlocks all operational routes", () => {
  assert.equal(navigationForSetup(true), navigation);
  assert.equal(routeForSetup("plans", true), "plans");
});

test("unknown locations fail safely to status", () => {
  assert.equal(routeFromHash("#plans"), "plans");
  assert.equal(routeFromHash("#not-a-route"), "status");
  assert.equal(routeFromHash(""), "status");
});
