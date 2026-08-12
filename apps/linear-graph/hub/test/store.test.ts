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

  test("persists brief provenance, triage, campaigns, and drain metrics", () => {
    const store = new GraphStore(":memory:");
    const timestamp = "2026-08-02T00:00:00.000Z";
    store.saveBrief({ text: "Drain graph work", updatedAt: timestamp, status: "ready", source: "manual", generatedAt: timestamp, snapshotAt: timestamp, error: null });
    store.saveTriageDecision({ issueId: "issue-1", disposition: "ready", rationale: "Has a concrete next step", confidence: 0.9, evidenceIssueIds: [], nextAction: "Implement", source: "manual", createdAt: timestamp, updatedAt: timestamp });
    const campaign = { id: "campaign:1", title: "Drain graph", prompt: "graph", status: "ready" as const, issueIds: ["issue-1"], processedIssueCount: 1, totalIssueCount: 1, bundles: [{ id: "bundle:1", campaignId: "campaign:1", title: "Graph", summary: "One issue", issueIds: ["issue-1"], issues: [{ issueId: "issue-1", order: 1 }], dependencyIssueIds: [], expectedReduction: 1, status: "proposed" as const, createdAt: timestamp, updatedAt: timestamp, approvedAt: null, approvalNote: null }], resolutionSet: null, snapshotAt: timestamp, createdAt: timestamp, updatedAt: timestamp, error: null };
    store.saveCampaign(campaign);
    store.saveDrainMetrics({ snapshotAt: timestamp, activeIssueCount: 1, completedIssueCount: 0, canceledIssueCount: 0, untriagedIssueCount: 0, readyIssueCount: 1, blockedIssueCount: 0, needsClarificationIssueCount: 0, duplicateStaleIssueCount: 0, bundleCandidateIssueCount: 0, approvedBundleIssueCount: 0, expectedReduction: 0, actualReduction: 0, activeCampaignCount: 1, proposedBundleCount: 1, approvedBundleCount: 0, drained: true, computedAt: timestamp });
    expect(store.loadBrief()?.status).toBe("ready");
    expect(store.loadTriageDecisions()).toHaveLength(1);
    expect(store.loadCampaign("campaign:1")?.bundles[0]?.issueIds).toEqual(["issue-1"]);
    expect(store.loadDrainMetrics()?.drained).toBe(true);
    store.close();
  });

  test("persists queued execution intent and detailed evidence across reload", () => {
    const store = new GraphStore(":memory:");
    store.saveExecutionRun({
      id: "execution:intent", bundleId: "bundle:1", repository: "commonkit", branch: null, worktreePath: null,
      status: "queued", startedAt: "2026-08-02T00:00:00.000Z", completedAt: null, exitCode: null,
      stdout: "", stderr: "", evidence: [{ kind: "runner", text: "Intent persisted before Codex start" }], error: null,
      instruction: "Implement the approved bundle", issueIds: ["issue-1"], createdAt: "2026-08-02T00:00:00.000Z",
    });
    const reloaded = store.loadExecutionRun("execution:intent");
    expect(reloaded).toMatchObject({ status: "queued", instruction: "Implement the approved bundle", issueIds: ["issue-1"] });
    expect(reloaded?.evidence[0]?.text).toContain("Intent persisted");
    store.close();
  });
});
