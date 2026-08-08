import { describe, expect, test } from "bun:test";
import { GraphStore } from "../src/store.js";

const legacyIssue = {
  id: "issue-1", identifier: "USG-1", title: "Legacy snapshot", description: null,
  url: "https://linear.app/unsold/issue/USG-1", team: { id: "team-1", key: "USG", name: "Unsold" },
  project: null, status: { id: "state-1", name: "Todo", type: "unstarted" }, priority: 2, labels: [], cycle: null,
  parentId: null, dueDate: null, createdAt: "2026-08-01T00:00:00.000Z", updatedAt: "2026-08-02T00:00:00.000Z",
  completedAt: null, repo: "commonkit", zone: "platform", topicTags: [],
} as const;

describe("graph store compatibility", () => {
  test("backfills teams when loading a pre-team snapshot", () => {
    const store = new GraphStore(":memory:");
    store.db.query("INSERT INTO snapshots (id, payload) VALUES (1, ?)").run(JSON.stringify({
      generatedAt: "2026-08-02T00:00:00.000Z", syncedAt: "2026-08-02T00:00:00.000Z", stale: false,
      nodes: [legacyIssue], edges: [], zones: [], recommendations: [], latestAnalysisRunId: null,
    }));
    expect(store.loadSnapshot()?.teams).toEqual([{
      id: "team-1", key: "USG", name: "Unsold", issueCount: 1, activeIssueCount: 1,
      completedIssueCount: 0, canceledIssueCount: 0,
    }]);
    store.close();
  });
});
