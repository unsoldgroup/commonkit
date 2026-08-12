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

  test("campaigns only include triage-approved work and exclude blocked or unclear issues", () => {
    const nodes = [issue("ready", "Pricing ready"), issue("blocked", "Pricing blocked"), issue("unclear", "Pricing unclear"), issue("untouched", "Pricing untouched")];
    const decisions = [
      { issueId: "ready", disposition: "ready" as const, rationale: "Concrete next step", confidence: 1, evidenceIssueIds: [], nextAction: "Implement", source: "codex" as const, createdAt: "2026-08-02T00:00:00.000Z", updatedAt: "2026-08-02T00:00:00.000Z" },
      { issueId: "blocked", disposition: "blocked" as const, rationale: "Waiting on API", confidence: 1, evidenceIssueIds: [], nextAction: null, source: "codex" as const, createdAt: "2026-08-02T00:00:00.000Z", updatedAt: "2026-08-02T00:00:00.000Z" },
      { issueId: "unclear", disposition: "needs_clarification" as const, rationale: "Missing acceptance criteria", confidence: 1, evidenceIssueIds: [], nextAction: null, source: "codex" as const, createdAt: "2026-08-02T00:00:00.000Z", updatedAt: "2026-08-02T00:00:00.000Z" },
    ];
    const result = proposeCampaign(snapshot(nodes), { prompt: "pricing" }, new Date("2026-08-02T00:00:00.000Z"), decisions);
    expect(result.issueIds).toEqual(["ready"]);
  });

  test("actual reduction is limited to closed issues in approved campaign outcomes", () => {
    const planningNodes = [issue("outside", "Closed elsewhere"), issue("inside", "Closed in campaign"), issue("open", "Still open")];
    const campaign = proposeCampaign(snapshot(planningNodes), { prompt: "campaign" });
    const approved = { ...campaign, bundles: campaign.bundles.map((bundle) => ({ ...bundle, status: "verified" as const, approvedAt: "2026-08-02T00:00:00.000Z", issueIds: ["inside"], issues: [{ issueId: "inside", order: 1 }] })) };
    const nodes = planningNodes.map((node) => node.id === "outside" || node.id === "inside" ? { ...node, status: { id: "done", name: "Done", type: "completed" as const } } : node);
    const metrics = computeDrainMetrics(snapshot(nodes), [], [approved]);
    expect(metrics.actualReduction).toBe(1);
  });
});
