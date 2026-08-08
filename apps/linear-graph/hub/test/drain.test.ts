import { describe, expect, test } from "bun:test";
import { DEFAULT_ZONES, type GraphSnapshot } from "@commonkit/linear-graph-protocol";
import { computeDrainMetrics, proposeCampaign } from "../src/drain.js";

const issue = (id: string, title: string, overrides: Record<string, unknown> = {}) => ({
  id, identifier: `USG-${id}`, title, description: `${title} description`, url: `https://linear.app/unsold/issue/USG-${id}`,
  team: { id: "team", key: "USG", name: "Unsold" }, project: { id: "project", name: "Graph" },
  status: { id: "state", name: "Todo", type: "unstarted" as const }, priority: 2, labels: ["graph"], cycle: null,
  parentId: null, dueDate: null, createdAt: "2026-08-01T00:00:00.000Z", updatedAt: "2026-08-02T00:00:00.000Z",
  completedAt: null, repo: "linear-issues-graph", zone: "platform" as const, topicTags: ["graph"], ...overrides,
});

const snapshot = (nodes: GraphSnapshot["nodes"], edges: GraphSnapshot["edges"] = []): GraphSnapshot => ({
  generatedAt: "2026-08-02T00:00:00.000Z", syncedAt: "2026-08-02T00:00:00.000Z", stale: false, nodes, edges,
  teams: [{ id: "team", key: "USG", name: "Unsold", issueCount: nodes.length, activeIssueCount: nodes.length, completedIssueCount: 0, canceledIssueCount: 0 }],
  zones: DEFAULT_ZONES.map((zone) => ({ ...zone })), recommendations: [], latestAnalysisRunId: null,
});

describe("work drain proposals", () => {
  test("searches the complete snapshot and stages execution bundles without mutating Linear", () => {
    const nodes = Array.from({ length: 22 }, (_, index) => issue(String(index), "Pricing refresh"));
    const result = proposeCampaign(snapshot(nodes), { prompt: "pricing" }, new Date("2026-08-02T00:00:00.000Z"));
    expect(result.issueIds).toHaveLength(22);
    expect(result.bundles.every((bundle) => bundle.status === "proposed")).toBe(true);
    expect(result.bundles.map((bundle) => bundle.issueIds.length)).toEqual([11, 11]);
  });

  test("includes seeded graph neighbors and exposes duplicate resolution for review", () => {
    const nodes = [issue("one", "Seed"), issue("two", "Neighbor"), issue("three", "Duplicate")];
    const edges = [
      { id: "related:one:two", sourceId: "one", targetId: "two", kind: "related" as const, source: "linear" as const },
      { id: "duplicate:one:three", sourceId: "one", targetId: "three", kind: "duplicate" as const, source: "linear" as const },
    ];
    const result = proposeCampaign(snapshot(nodes, edges), { prompt: "unmatched", seedIssueIds: ["one"] });
    expect(result.issueIds).toEqual(expect.arrayContaining(["one", "two", "three"]));
    expect(result.resolutionSet?.issueIds).toEqual(["three"]);
    expect(result.resolutionSet?.status).toBe("proposed");
  });

  test("computes untriaged and approval metrics without claiming Linear changes", () => {
    const nodes = [issue("one", "One"), issue("two", "Two"), issue("done", "Done", { status: { id: "done", name: "Done", type: "completed" as const } })];
    const campaign = proposeCampaign(snapshot(nodes), { prompt: "One" });
    const metrics = computeDrainMetrics(snapshot(nodes), [{ issueId: "one", disposition: "ready", rationale: "ready", confidence: 1, evidenceIssueIds: [], nextAction: "start", source: "manual", createdAt: "2026-08-02T00:00:00.000Z", updatedAt: "2026-08-02T00:00:00.000Z" }], [campaign]);
    expect(metrics.activeIssueCount).toBe(2);
    expect(metrics.untriagedIssueCount).toBe(1);
    expect(metrics.completedIssueCount).toBe(1);
    expect(metrics.drained).toBe(false);
  });
});
