import { describe, expect, test } from "bun:test";
import { findClaudePane, findTerminalHandle, OrcaSteerer } from "../src/steer.js";

const worktrees = {
  ok: true,
  result: {
    worktrees: [
      {
        path: "/worktrees/other",
        agents: [{ agentType: "claude", paneKey: "tab-other:leaf-other" }],
      },
      {
        path: "/worktrees/commonkit",
        agents: [
          { agentType: "codex", paneKey: "tab-1:leaf-codex" },
          { agentType: "claude", paneKey: "tab-1:leaf-claude" },
        ],
      },
    ],
  },
};

const terminals = {
  ok: true,
  result: {
    terminals: [
      { handle: "term_codex", tabId: "tab-1", leafId: "leaf-codex" },
      { handle: "term_claude", tabId: "tab-1", leafId: "leaf-claude" },
    ],
  },
};

describe("Orca deny steering", () => {
  test("matches cwd to the worktree's Claude pane", () => {
    expect(findClaudePane(worktrees, "/worktrees/commonkit/")).toBe("tab-1:leaf-claude");
    expect(findClaudePane(worktrees, "/worktrees/missing")).toBeUndefined();
  });

  test("resolves the pane's terminal handle", () => {
    expect(findTerminalHandle(terminals, "tab-1:leaf-claude")).toBe("term_claude");
    expect(findTerminalHandle(terminals, "tab-9:leaf-gone")).toBeUndefined();
  });

  test("sends guidance to the matched Claude pane's terminal handle", async () => {
    const calls: string[][] = [];
    const steerer = new OrcaSteerer(async (argv) => {
      calls.push(argv);
      if (calls.length === 1) return { exitCode: 0, stdout: JSON.stringify(worktrees), stderr: "" };
      if (calls.length === 2) return { exitCode: 0, stdout: JSON.stringify(terminals), stderr: "" };
      return { exitCode: 0, stdout: "{}", stderr: "" };
    });

    await steerer.send("/worktrees/commonkit", "Use the read-only command.");

    expect(calls[1]).toEqual(["orca", "terminal", "list", "--json"]);
    expect(calls[2]).toEqual([
      "orca", "orchestration", "send",
      "--to", "term_claude",
      "--subject", "Permission denied — user guidance",
      "--body", "Use the read-only command.",
      "--type", "status",
      "--json",
    ]);
  });

  test("returns without sending when cwd does not match", async () => {
    const calls: string[][] = [];
    const steerer = new OrcaSteerer(async (argv) => {
      calls.push(argv);
      return { exitCode: 0, stdout: JSON.stringify(worktrees), stderr: "" };
    });
    await steerer.send("/worktrees/missing", "Try something else.");
    expect(calls).toHaveLength(1);
  });

  test("returns without sending when the pane has no live terminal", async () => {
    const calls: string[][] = [];
    const steerer = new OrcaSteerer(async (argv) => {
      calls.push(argv);
      return { exitCode: 0, stdout: JSON.stringify(calls.length === 1 ? worktrees : { ok: true, result: { terminals: [] } }), stderr: "" };
    });
    await steerer.send("/worktrees/commonkit", "Try something else.");
    expect(calls).toHaveLength(2);
  });

  test("fails open when Orca lookup or send fails", async () => {
    const lookupFailure = new OrcaSteerer(async () => {
      throw new Error("Orca unavailable");
    });
    await expect(lookupFailure.send("/worktrees/commonkit", "Try something else.")).resolves.toBeUndefined();

    let calls = 0;
    const sendFailure = new OrcaSteerer(async () => {
      calls += 1;
      if (calls === 1) return { exitCode: 0, stdout: JSON.stringify(worktrees), stderr: "" };
      if (calls === 2) return { exitCode: 0, stdout: JSON.stringify(terminals), stderr: "" };
      throw new Error("terminal disappeared");
    });
    await expect(sendFailure.send("/worktrees/commonkit", "Try something else.")).resolves.toBeUndefined();
  });
});
