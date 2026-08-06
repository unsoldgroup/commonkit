import { describe, expect, test } from "bun:test";
import { deriveRuleSuggestion, describePermission, responseForDecision } from "../src/claude-hook-server.js";
import type { ClaudePermissionAction, Decision } from "@commonkit/session-board-protocol";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

describe("deriveRuleSuggestion", () => {
  test.each([
    ["Bash", { command: "ls -la" }, "Bash(ls:*)"],
    ["Bash", { command: "git push --force" }, "Bash(git push:*)"],
    ["Bash", { command: "npm publish --tag next" }, "Bash(npm publish:*)"],
    ["Bash", { command: "pnpm test --filter reporter" }, "Bash(pnpm test:*)"],
    ["Bash", { command: "gh pr create" }, "Bash(gh pr:*)"],
    ["Edit", { file_path: "/x/a.ts" }, "Edit"],
    ["Write", { file_path: "/x/a.ts" }, "Write"],
    ["NotebookEdit", { notebook_path: "/x/a.ipynb" }, "NotebookEdit"],
    ["WebFetch", { url: "https://example.com" }, "WebFetch"],
  ])("derives %s rules", (tool, input, expected) => {
    expect(deriveRuleSuggestion(tool, input)).toBe(expected);
  });
});

describe("responseForDecision", () => {
  const action: ClaudePermissionAction = {
    id: "action-1",
    kind: "claude-permission",
    sessionRef: { machineId: "studio", worktreeId: "/worktree", paneKey: "session-1" },
    summary: "Push branch",
    detail: { tool: "Bash", input: { command: "git push" }, ruleSuggestion: "Bash(git push:*)" },
    createdAt: "2026-07-28T12:00:00.000Z",
    expiresAt: "2026-07-28T12:00:54.000Z",
  };
  const decision = (verdict: Decision["verdict"]): Decision => ({
    actionId: action.id,
    verdict,
    decidedAt: "2026-07-28T12:00:01.000Z",
  });

  test("turns always into allow with the previewed project-local rule", () => {
    expect(responseForDecision(action, decision("always"))).toEqual({
      decision: "allow",
      updatedPermissions: [{ rule: "Bash(git push:*)", destination: "localSettings" }],
    });
  });

  test("keeps allow and deny plain", () => {
    expect(responseForDecision(action, decision("allow"))).toEqual({ decision: "allow" });
    expect(responseForDecision(action, decision("deny"))).toEqual({ decision: "deny" });
  });
});

describe("describePermission", () => {
  test("shows the bash command with project context", () => {
    expect(describePermission("Bash", { command: "rm -rf build" }, "/Users/al/code/commonkit"))
      .toBe("[commonkit] rm -rf build");
  });

  test("prefers description then command for bash", () => {
    expect(describePermission("Bash", { command: "git push", description: "Push branch" }, "/x/repo"))
      .toBe("[repo] Push branch — git push");
  });

  test("shows file path for edits and url for fetches", () => {
    expect(describePermission("Edit", { file_path: "/x/repo/a.ts" }, "/x/repo")).toBe("[repo] Edit a.ts");
    expect(describePermission("WebFetch", { url: "https://e.com" }, "/x/repo")).toBe("[repo] Fetch https://e.com");
  });

  test("falls back to Allow <tool> and truncates long commands", () => {
    expect(describePermission("Mystery", {}, "/x/repo")).toBe("[repo] Allow Mystery");
    expect(describePermission("Bash", { command: "x".repeat(300) }, "/x/repo").length).toBeLessThanOrEqual(200);
  });
});

import { lastAssistantText } from "../src/claude-hook-server.js";

describe("lastAssistantText", () => {
  test("extracts the latest assistant text from a transcript", async () => {
    const directory = await mkdtemp(join(tmpdir(), "session-board-transcript-"));
    const path = join(directory, "session.jsonl");
    try {
      await writeFile(path, [
        JSON.stringify({ type: "assistant", message: { content: [{ type: "text", text: "Earlier context." }] } }),
        JSON.stringify({
          type: "assistant",
          message: { content: [{ type: "text", text: "Recording the ADR for import budgeting before wiring the distiller." }] },
        }),
        "",
      ].join("\n"));

      expect(await lastAssistantText(path)).toBe("Recording the ADR for import budgeting before wiring the distiller.");
      expect(await lastAssistantText(join(directory, "does-not-exist.jsonl"))).toBeUndefined();
      expect(await lastAssistantText(undefined)).toBeUndefined();
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });
});
