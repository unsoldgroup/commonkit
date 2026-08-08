import { describe, expect, test } from "bun:test";
import { codexAnalysisSchema, graphSnapshotSchema, DEFAULT_ZONES } from "../src/index.js";

const issue = {
  id: "issue-1", identifier: "USG-1", title: "Define graph", description: null,
  url: "https://linear.app/unsold/issue/USG-1", team: { id: "team-1", key: "USG", name: "Unsold" },
  project: null, status: { id: "status-1", name: "Todo", type: "unstarted" as const }, priority: 2,
  labels: ["feature"], cycle: null, parentId: null, dueDate: null,
  createdAt: "2026-08-01T00:00:00.000Z", updatedAt: "2026-08-02T00:00:00.000Z", completedAt: null,
  repo: "commonkit", zone: "platform", topicTags: ["graph"],
};

describe("linear graph protocol", () => {
  test("round-trips snapshots and analysis results", () => {
    const snapshot = {
      generatedAt: "2026-08-02T00:00:00.000Z", syncedAt: "2026-08-02T00:00:00.000Z", stale: false,
      nodes: [issue], edges: [], teams: [{ id: "team-1", key: "USG", name: "Unsold", issueCount: 1, activeIssueCount: 1, completedIssueCount: 0, canceledIssueCount: 0 }], zones: DEFAULT_ZONES.map((zone) => ({ ...zone })), recommendations: [], latestAnalysisRunId: null,
    };
    expect(graphSnapshotSchema.parse(JSON.parse(JSON.stringify(snapshot)))).toEqual(snapshot);
    const analysis = { assignments: [{ issueId: issue.id, zone: "platform", topicTags: ["graph"], confidence: 0.9, rationale: "Shared tooling" }], semanticEdges: [], recommendations: [] };
    expect(codexAnalysisSchema.parse(analysis)).toEqual(analysis);
  });

  test("rejects malformed issue identifiers and unsafe topic zones", () => {
    expect(() => graphSnapshotSchema.parse({ nodes: [{ ...issue, zone: "Platform" }] })).toThrow();
    expect(() => codexAnalysisSchema.parse({ assignments: [{ issueId: issue.id, zone: "../secrets", topicTags: [], confidence: 1, rationale: "x" }], semanticEdges: [], recommendations: [] })).toThrow();
  });
});
