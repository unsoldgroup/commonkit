import { describe, expect, test } from "bun:test";
import { runHook } from "../session-board-hook.js";

describe("Claude PermissionRequest hook", () => {
  test("fails open with no permission decision when the reporter returns no decision", async () => {
    const mockReporter: typeof fetch = async (_input, init) => await new Promise<Response>((_resolve, reject) => {
      init?.signal?.addEventListener("abort", () => reject(init.signal?.reason), { once: true });
    });
    const previousTimeout = process.env.SESSION_BOARD_HOOK_TIMEOUT_MS;
    process.env.SESSION_BOARD_HOOK_TIMEOUT_MS = "5";
    const output = await runHook(JSON.stringify({
      session_id: "session-1",
      transcript_path: "/tmp/transcript.jsonl",
      hook_event_name: "PermissionRequest",
      tool_name: "Bash",
      tool_input: { command: "pnpm test" },
      cwd: "/worktree",
    }), mockReporter);
    if (previousTimeout === undefined) delete process.env.SESSION_BOARD_HOOK_TIMEOUT_MS;
    else process.env.SESSION_BOARD_HOOK_TIMEOUT_MS = previousTimeout;

    expect(output).toBeUndefined();
  });

  test("the executable exits zero and prints no decision when the reporter is unreachable", async () => {
    const child = Bun.spawn(["bun", "run", new URL("../session-board-hook.ts", import.meta.url).pathname], {
      env: {
        ...process.env,
        SESSION_BOARD_REPORTER_URL: "http://127.0.0.1:1",
        SESSION_BOARD_HOOK_TIMEOUT_MS: "5",
      },
      stdin: "pipe",
      stdout: "pipe",
      stderr: "pipe",
    });
    child.stdin.write(JSON.stringify({
      session_id: "session-1",
      hook_event_name: "PermissionRequest",
      tool_name: "Bash",
      tool_input: { command: "pnpm test" },
      cwd: "/worktree",
    }));
    child.stdin.end();

    expect(await child.exited).toBe(0);
    expect(await new Response(child.stdout).text()).toBe("");
  });

  test("returns only the human reporter decision to Claude", async () => {
    const output = await runHook(JSON.stringify({
      session_id: "session-1",
      hook_event_name: "PermissionRequest",
      tool_name: "Bash",
      tool_input: { command: "pnpm test" },
      cwd: "/worktree",
    }), async () => Response.json({ decision: "deny" }));

    expect(JSON.parse(output!)).toEqual({
      hookSpecificOutput: {
        hookEventName: "PermissionRequest",
        decision: { behavior: "deny" },
      },
    });
  });
});
