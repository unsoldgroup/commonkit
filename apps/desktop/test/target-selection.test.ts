import assert from "node:assert/strict";
import test from "node:test";

import { assertPlanTarget, convergenceTarget } from "../src/target-selection.ts";

const inventory = {
  targets: [
    { id: "workstation-a", transport: { type: "local" as const }, identityDigest: "sha256:a" },
    { id: "workstation-b", transport: { type: "local" as const }, identityDigest: "sha256:b" },
  ],
  selected: ["workstation-a"],
};

test("convergence binds to the one current selected target", () => {
  assert.equal(convergenceTarget(inventory), "workstation-a");
});

test("a plan for another target is rejected before desktop apply", () => {
  assert.doesNotThrow(() => assertPlanTarget({ targetId: "workstation-a" }, "workstation-a"));
  assert.throws(
    () => assertPlanTarget({ targetId: "workstation-b" }, "workstation-a"),
    /target_plan_mismatch/,
  );
  assert.throws(() => assertPlanTarget({}, "workstation-a"), /target_plan_mismatch/);
});

test("convergence rejects stale, absent, or ambiguous selections", () => {
  assert.throws(
    () => convergenceTarget({ ...inventory, selected: ["removed-target"] }),
    /stale_target_selection/,
  );
  assert.throws(() => convergenceTarget({ ...inventory, selected: [] }), /target_selection_required/);
  assert.throws(
    () => convergenceTarget({ ...inventory, selected: ["workstation-a", "workstation-b"] }),
    /single_target_required/,
  );
});
