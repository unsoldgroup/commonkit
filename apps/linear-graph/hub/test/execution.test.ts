import { describe, expect, test } from "bun:test";
import type { WorkBundle } from "@commonkit/linear-graph-protocol";
import { executeApprovedBundle, type CreateWorktreeInput, type ExecutionProcess, type WorktreeAdapter } from "../src/execution.js";

const bundle = (status: WorkBundle["status"] = "approved"): WorkBundle => ({
  id: "bundle:product-auth", campaignId: "campaign:drain", title: "Auth reliability",
  summary: "Make authentication recovery reliable.", issueIds: ["issue-1"],
  issues: [{ issueId: "issue-1", order: 1, rationale: "Primary implementation issue" }], dependencyIssueIds: [],
  expectedReduction: 1, status, createdAt: "2026-08-08T08:00:00.000Z", updatedAt: "2026-08-08T08:00:00.000Z",
  approvedAt: status === "approved" ? "2026-08-08T08:05:00.000Z" : null, approvalNote: "Approved for implementation",
});

const stream = (text: string) => new ReadableStream<Uint8Array>({
  start(controller) { controller.enqueue(new TextEncoder().encode(text)); controller.close(); },
});

function fakeProcess(stdout: string, stderr: string, exitCode: number): ExecutionProcess {
  return {
    stdin: { write() {}, end() {} }, stdout: stream(stdout), stderr: stream(stderr), exited: Promise.resolve(exitCode), kill() {},
  };
}

const options = (overrides: Record<string, unknown> = {}) => ({
  repositoryAllowlist: { commonkit: process.cwd() },
  now: () => new Date("2026-08-08T09:00:00.000Z"),
  ...overrides,
});

describe("approved bundle execution", () => {
  test("creates an isolated worktree, invokes writable headless Codex, and captures evidence", async () => {
    const calls: { command: string[]; cwd?: string; prompt?: string; removed: boolean } = { command: [], removed: false };
    const worktree: WorktreeAdapter = {
      async create(input) { calls.cwd = "/tmp/linear-graph-test-worktree"; return { path: calls.cwd, repoRoot: input.repoRoot, branch: input.branch }; },
      async remove() { calls.removed = true; },
    };
    const result = await executeApprovedBundle({
      bundle: bundle(), repository: "commonkit", instruction: "Implement the recovery path.",
      issueContext: [{ identifier: "USG-1", title: "Recovery", description: "Recovery details" }],
    }, options({
      worktree,
      spawn(command: string[], spawnOptions: { cwd: string; env: Record<string, string> }) {
        calls.command = command;
        calls.cwd = spawnOptions.cwd;
        return {
          ...fakeProcess('{"type":"item.completed","item":{"text":"tests passed"}}\n', "", 0),
          stdin: { write(prompt: string) { calls.prompt = prompt; }, end() {} },
        };
      },
    }));

    expect(result.status).toBe("completed");
    expect(result.exitCode).toBe(0);
    expect(result.stdout).toContain("tests passed");
    expect(result.evidence[0]?.kind).toBe("codex_stdout");
    expect(calls.command).toContain("exec");
    expect(calls.command).toContain("--sandbox");
    expect(calls.command).toContain("workspace-write");
    expect(calls.command).toContain("--ask-for-approval");
    expect(calls.command).toContain("never");
    expect(calls.command).not.toContain("merge");
    expect(calls.command).not.toContain("deploy");
    expect(calls.prompt).toContain("Do not run git push, git merge, git rebase");
    expect(calls.prompt).toContain("USG-1");
    expect(result.branch).toMatch(/^linear-graph\//);
    expect(result.worktreePath).toBe("/tmp/linear-graph-test-worktree");
    expect(calls.removed).toBe(true);
  });

  test("refuses an unapproved bundle before creating a worktree or spawning Codex", async () => {
    let created = false;
    let spawned = false;
    const result = await executeApprovedBundle({ bundle: bundle("proposed"), repository: "commonkit" }, options({
      worktree: { async create() { created = true; throw new Error("must not create"); }, async remove() {} },
      spawn() { spawned = true; return fakeProcess("", "", 0); },
    }));

    expect(result.status).toBe("failed");
    expect(result.error).toContain("explicitly approved");
    expect(created).toBe(false);
    expect(spawned).toBe(false);
  });

  test("refuses repositories outside the explicit allowlist", async () => {
    let created = false;
    const result = await executeApprovedBundle({ bundle: bundle(), repository: "unknown" }, options({
      worktree: { async create() { created = true; throw new Error("must not create"); }, async remove() {} },
    }));

    expect(result.status).toBe("failed");
    expect(result.error).toContain("not in the execution allowlist");
    expect(created).toBe(false);
  });

  test("preserves non-zero exit, stderr, and cleanup evidence", async () => {
    let removed = false;
    const result = await executeApprovedBundle({ bundle: bundle(), repository: "commonkit" }, options({
      worktree: { async create(input: CreateWorktreeInput) { return { path: "/tmp/worktree", repoRoot: input.repoRoot, branch: input.branch }; }, async remove() { removed = true; } },
      spawn() { return fakeProcess("partial output", "test failed", 17); },
    }));

    expect(result.status).toBe("failed");
    expect(result.exitCode).toBe(17);
    expect(result.stderr).toBe("test failed");
    expect(result.evidence.map((item) => item.kind)).toEqual(["codex_stdout", "codex_stderr"]);
    expect(result.error).toContain("17");
    expect(removed).toBe(true);
  });
});
