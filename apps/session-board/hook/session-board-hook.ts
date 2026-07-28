#!/usr/bin/env bun

import { claudeHookRequestSchema, claudeHookResponseSchema } from "@commonkit/session-board-protocol";

type HookInput = {
  session_id?: unknown;
  transcript_path?: unknown;
  hook_event_name?: unknown;
  tool_name?: unknown;
  tool_input?: unknown;
  cwd?: unknown;
};

export async function runHook(rawInput: string, fetcher: typeof fetch = fetch): Promise<string | undefined> {
  let hook: HookInput;
  try {
    hook = JSON.parse(rawInput) as HookInput;
  } catch {
    return;
  }
  if (hook.hook_event_name !== "PermissionRequest" || typeof hook.tool_name !== "string" || typeof hook.session_id !== "string" || typeof hook.cwd !== "string") return;

  try {
    const request = claudeHookRequestSchema.parse({
      tool: hook.tool_name,
      input: hook.tool_input,
      session: {
        id: hook.session_id,
        cwd: hook.cwd,
        ...(typeof hook.transcript_path === "string" ? { transcriptPath: hook.transcript_path } : {}),
      },
    });
    const timeoutMs = Number(process.env.SESSION_BOARD_HOOK_TIMEOUT_MS ?? 55_000);
    const reporterUrl = process.env.SESSION_BOARD_REPORTER_URL ?? "http://127.0.0.1:47821";
    const response = await fetcher(`${reporterUrl.replace(/\/$/, "")}/claude/permission`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(request),
      signal: AbortSignal.timeout(Math.min(55_000, Math.max(1, timeoutMs))),
    });
    if (!response.ok || response.status === 204) return;
    const { decision } = claudeHookResponseSchema.parse(await response.json());
    return JSON.stringify({
      hookSpecificOutput: {
        hookEventName: "PermissionRequest",
        decision: { behavior: decision },
      },
    });
  } catch {
    // Deliberately undecided: Claude falls back to its normal human TUI prompt.
  }
}

if (import.meta.main) {
  const output = await runHook(await Bun.stdin.text());
  if (output) console.log(output);
}
