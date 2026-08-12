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

  test("redacts persisted untrusted brief and execution output", () => {
    const store = new GraphStore(":memory:");
    const timestamp = "2026-08-02T00:00:00.000Z";
    const secret = "sk-persisted-12345678901234567890";
    store.saveBrief({ text: `Review ${secret}`, updatedAt: timestamp, status: "ready", source: "manual", error: null });
    store.saveExecutionRun({
      id: "execution:redaction", bundleId: "bundle:1", repository: "commonkit", branch: null, worktreePath: null,
      status: "failed", startedAt: timestamp, completedAt: timestamp, exitCode: 1,
      stdout: `Bearer ${secret}`, stderr: `x${secret}`, evidence: [{ kind: "runner", text: secret }], error: secret,
      instruction: secret, issueIds: ["issue-1"], createdAt: timestamp,
    });
    expect(JSON.stringify(store.loadBrief())).not.toContain(secret);
    expect(JSON.stringify(store.loadExecutionRun("execution:redaction"))).not.toContain(secret);
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

  test("CAS-serializes bundle approval and execution claims", () => {
    const store = new GraphStore(":memory:");
    const timestamp = "2026-08-02T00:00:00.000Z";
    const campaign = { id: "campaign:cas", title: "CAS", prompt: "cas", status: "ready" as const, issueIds: ["issue-1"], processedIssueCount: 1, totalIssueCount: 1, bundles: [{ id: "bundle:cas", campaignId: "campaign:cas", title: "CAS", summary: "One issue", issueIds: ["issue-1"], issues: [{ issueId: "issue-1", order: 1 }], dependencyIssueIds: [], expectedReduction: 1, status: "proposed" as const, createdAt: timestamp, updatedAt: timestamp, approvedAt: null, approvalNote: null }], resolutionSet: null, snapshotAt: timestamp, createdAt: timestamp, updatedAt: timestamp, error: null };
    store.saveCampaign(campaign);
    const approved = store.approveBundle("bundle:cas", "approve", "first", timestamp);
    expect(approved.ok && approved.bundle.status).toBe("approved");
    expect(store.approveBundle("bundle:cas", "approve", "duplicate", timestamp)).toMatchObject({ ok: false, reason: "compare_failed" });
    const queued = { id: "execution:cas", bundleId: "bundle:cas", repository: "commonkit", branch: null, worktreePath: null, status: "queued" as const, startedAt: timestamp, completedAt: null, exitCode: null, stdout: "", stderr: "", evidence: [], error: null, createdAt: timestamp };
    const claimed = store.claimBundleExecution("bundle:cas", queued);
    expect(claimed.ok && claimed.execution.status).toBe("queued");
    const duplicate = { ...queued, id: "execution:cas-duplicate" };
    expect(store.claimBundleExecution("bundle:cas", duplicate)).toMatchObject({ ok: false, reason: "compare_failed" });
    store.close();
  });
});
