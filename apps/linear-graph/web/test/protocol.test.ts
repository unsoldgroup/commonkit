import { describe, expect, test } from "bun:test";
import { normalizeIssue } from "../src/protocol.js";

describe("web graph protocol adapter", () => {
  test("preserves full Linear context for selected-ticket rendering", () => {
    const node = normalizeIssue({
      id: "issue-1", identifier: "USG-1", title: "Define graph", description: "## Context\n\nConnect the related work.",
      url: "https://linear.app/unsold/issue/USG-1", team: { id: "team-1", key: "USG", name: "Unsold" }, project: null,
      status: { id: "status-1", name: "Todo", type: "unstarted" }, priority: 2, labels: ["feature"], cycle: null, parentId: null,
      dueDate: null, createdAt: "2026-08-01T00:00:00.000Z", updatedAt: "2026-08-02T00:00:00.000Z", completedAt: null,
      repo: "commonkit", zone: "platform", topicTags: ["graph"],
    });
    expect(node.description).toContain("Connect the related work");
    expect(node.labels).toEqual(["feature"]);
    expect(node.parentId).toBeUndefined();
  });
});
