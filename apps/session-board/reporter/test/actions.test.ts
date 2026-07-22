import { describe, expect, test } from "bun:test";
import type { PendingAction } from "@commonkit/session-board-protocol";
import { DecisionExecutor, normalizeOpenGates } from "../src/actions.js";

describe("Orca gate actions", () => {
  test("normalizes only open gates into human-decision actions", () => {
    const actions = normalizeOpenGates({ ok: true, result: { gates: [
      { id: "gate-1", task_id: "task-1", status: "open", question: "Deploy?", options: ["Now", "Later"], created_at: 1784664000000 },
      { id: "gate-2", status: "resolved", question: "Closed" },
    ] } }, "studio");
    expect(actions).toHaveLength(1);
    expect(actions[0]?.detail.options).toEqual(["Now", "Later"]);
    expect(actions[0]?.sessionRef.machineId).toBe("studio");
  });

  test("resolves a gate only after a hub decision", async () => {
    const action = normalizeOpenGates({ ok: true, result: { gates: [{ id: "gate-1", status: "open", question: "Deploy?", options: ["Now", "Later"] }] } }, "studio")[0]!;
    const calls: string[][] = [];
    const executor = new DecisionExecutor(
      () => new Map<string, PendingAction>([[action.id, action]]),
      undefined,
      async (argv) => { calls.push(argv); return { exitCode: 0, stdout: JSON.stringify({ ok: true }), stderr: "" }; },
    );
    expect(calls).toEqual([]);
    await executor.execute({ actionId: action.id, verdict: "option:2", decidedAt: new Date().toISOString() });
    expect(calls[0]).toEqual(["orca", "orchestration", "gate-resolve", "--id", "gate-1", "--resolution", "Later", "--json"]);
  });
});
