import assert from "node:assert/strict";
import test from "node:test";
import { navigation, routeFromHash } from "../src/navigation.ts";

test("management navigation covers every v1 operator workflow", () => {
  assert.deepEqual(navigation.map((item) => item.route), [
    "onboarding", "status", "plans", "credentials", "snapshots",
    "relay", "schedule", "diagnostics", "settings",
  ]);
});

test("unknown locations fail safely to status", () => {
  assert.equal(routeFromHash("#plans"), "plans");
  assert.equal(routeFromHash("#not-a-route"), "status");
  assert.equal(routeFromHash(""), "status");
});
