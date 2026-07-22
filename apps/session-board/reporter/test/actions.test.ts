import { describe, expect, test } from "bun:test";
import type { PendingAction } from "@commonkit/session-board-protocol";
import { CodexTerminalExecutor, DecisionExecutor, normalizeOpenGates } from "../src/actions.js";

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

describe("Codex terminal decisions", () => {
  test("waits, verifies the selected option, sends its number, and confirms the prompt was consumed", async () => {
    const action: PendingAction = {
      id: "codex-1",
      kind: "codex-prompt",
      sessionRef: { machineId: "studio", worktreeId: "commonkit::/worktree", paneKey: "term-1" },
      summary: "Allow Codex command",
      detail: { prompt: "Allow Codex to run `pnpm test`?", options: ["Yes, proceed", "No, tell Codex what to do differently"] },
      createdAt: new Date().toISOString(),
      expiresAt: new Date(Date.now() + 60_000).toISOString(),
    };
    const calls: string[][] = [];
    const results = [
      { exitCode: 0, stdout: JSON.stringify({ ok: true }), stderr: "" },
      { exitCode: 0, stdout: JSON.stringify({ ok: true, result: { terminal: { tail: ["Allow Codex to run `pnpm test`?", "1. Yes, proceed", "2. No, tell Codex what to do differently"], nextCursor: 41 } } }), stderr: "" },
      { exitCode: 0, stdout: JSON.stringify({ ok: true }), stderr: "" },
      { exitCode: 0, stdout: JSON.stringify({ ok: true, result: { terminal: { tail: ["Running pnpm test"], nextCursor: 42 } } }), stderr: "" },
    ];
    const closed: unknown[] = [];
    const executor = new CodexTerminalExecutor(async (argv) => {
      calls.push(argv);
      return results.shift()!;
    }, (outcome) => closed.push(outcome));

    await executor.execute(action, "allow");

    expect(calls).toEqual([
      ["orca", "terminal", "wait", "--terminal", "term-1", "--for", "tui-idle", "--timeout-ms", "5000", "--json"],
      ["orca", "terminal", "read", "--terminal", "term-1", "--json"],
      ["orca", "terminal", "send", "--terminal", "term-1", "--text", "1", "--enter", "--json"],
      ["orca", "terminal", "read", "--terminal", "term-1", "--cursor", "41", "--json"],
    ]);
    expect(closed).toEqual([{ actionId: "codex-1", outcome: "allowed" }]);
  });
});
