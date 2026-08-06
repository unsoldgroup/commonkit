import { describe, expect, test } from "bun:test";
import { TailReader } from "../src/tail.js";

describe("terminal tail reader", () => {
  test("reads the matching worktree terminal and bounds its tail", async () => {
    const calls: string[][] = [];
    const reader = new TailReader(async (argv) => {
      calls.push(argv);
      const body = argv.includes("list")
        ? { ok: true, result: { terminals: [{ handle: "term-1", worktreeId: "worktree-1" }] } }
        : { ok: true, result: { terminal: { tail: ["one", "two"] } } };
      return { exitCode: 0, stdout: JSON.stringify(body), stderr: "" };
    });
    expect(await reader.read({ machineId: "studio", worktreeId: "worktree-1", paneKey: "pane-1" })).toEqual(["one", "two"]);
    expect(calls[1]).toEqual(["orca", "terminal", "read", "--terminal", "term-1", "--json"]);
  });
});
