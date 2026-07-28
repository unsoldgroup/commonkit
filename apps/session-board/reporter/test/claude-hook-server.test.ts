import { describe, expect, test } from "bun:test";
import { describePermission } from "../src/claude-hook-server.js";

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
    const path = "/tmp/sb-transcript-test.jsonl";
    expect(await lastAssistantText(path)).toBe("Recording the ADR for import budgeting before wiring the distiller.");
    expect(await lastAssistantText("/tmp/does-not-exist.jsonl")).toBeUndefined();
    expect(await lastAssistantText(undefined)).toBeUndefined();
  });
});
