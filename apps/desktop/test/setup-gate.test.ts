import assert from "node:assert/strict";
import test from "node:test";
import { setupCompletion } from "../src/setup-gate.ts";

test("setup remains locked until a composed target and loadout are active", () => {
  assert.equal(setupCompletion(null, null), "checking");
  assert.equal(setupCompletion({ activeTarget: null, activeLoadout: null }, { targets: [] }), "required");
  assert.equal(setupCompletion({ activeTarget: "macbook", activeLoadout: null }, { targets: [{ id: "macbook" }] }), "required");
});

test("an active registered target unlocks the management application", () => {
  assert.equal(
    setupCompletion(
      { activeTarget: "macbook", activeLoadout: "personal" },
      { targets: [{ id: "macbook" }] },
    ),
    "complete",
  );
});

test("a persisted selected target unlocks setup when legacy status fields are empty", () => {
  assert.equal(
    setupCompletion(
      { activeTarget: null, activeLoadout: null },
      { selected: ["macbook"], targets: [{ id: "macbook" }] },
    ),
    "complete",
  );
});

test("a stale selected target does not unlock setup", () => {
  assert.equal(
    setupCompletion(
      { activeTarget: null, activeLoadout: null },
      { selected: ["removed"], targets: [{ id: "macbook" }] },
    ),
    "required",
  );
});
