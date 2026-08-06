import assert from "node:assert/strict";
import test from "node:test";

import { checkForDesktopUpdate, refreshDesktopState, shouldRunLiveRefresh } from "../src/live-refresh.ts";

test("background refresh retries readiness but pauses during active onboarding", () => {
  assert.equal(shouldRunLiveRefresh(false, "checking"), true);
  assert.equal(shouldRunLiveRefresh(false, "required"), false);
  assert.equal(shouldRunLiveRefresh(true, "required"), true);
  assert.equal(shouldRunLiveRefresh(false, "complete"), true);
});

test("one live refresh atomically obtains status, management, and target domains", async () => {
  const calls: string[] = [];
  const result = await refreshDesktopState({
    snapshot: async () => { calls.push("snapshot"); return { status: { state: "applying" }, lastEventId: 7 }; },
    managementSnapshot: async () => { calls.push("management"); return { plans: { operation: { state: "running" } } }; },
    targets: async () => { calls.push("targets"); return { targets: [], selected: [] }; },
  });
  assert.deepEqual(calls.sort(), ["management", "snapshot", "targets"]);
  assert.equal(result.snapshot.lastEventId, 7);
  assert.deepEqual(result.management.plans, { operation: { state: "running" } });
});

test("a transient domain failure preserves the last known state", async () => {
  const previous = { snapshot: { status: { state: "healthy" }, lastEventId: 4 }, management: { plans: {} }, targets: { targets: [], selected: [] } };
  const result = await refreshDesktopState({
    snapshot: async () => { throw new Error("offline"); },
    managementSnapshot: async () => ({ plans: { operation: { state: "done" } } }),
    targets: async () => { throw new Error("offline"); },
  }, previous);
  assert.equal(result.snapshot, previous.snapshot);
  assert.deepEqual(result.management.plans, { operation: { state: "done" } });
  assert.equal(result.targets, previous.targets);
});

test("startup update checks surface a newer signed build without installing it", async () => {
  let checks = 0;
  const state = await checkForDesktopUpdate({
    checkForUpdate: async () => {
      checks += 1;
      return { currentVersion: "0.1.0", version: "0.2.0", notes: "Real status channels", publishedAt: null };
    },
  });

  assert.equal(checks, 1);
  assert.deepEqual(state, {
    kind: "available",
    update: { currentVersion: "0.1.0", version: "0.2.0", notes: "Real status channels", publishedAt: null },
  });
});
