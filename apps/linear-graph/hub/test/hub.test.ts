import { afterEach, describe, expect, test } from "bun:test";
import { applyCodexAnalysis, focusSnapshot } from "../src/analysis.js";
import { parseCodexEvents } from "../src/codex.js";
import { normalizeLinearEdges, normalizeLinearIssue, summarizeTeams } from "../src/normalizer.js";
import { startGraphHub, trustedNonDestructiveMutation } from "../src/server.js";

const raw = (id: string, title = "Graph issue") => ({
  id, identifier: `USG-${id}`, title, description: "description", url: `https://linear.app/unsold/issue/USG-${id}`,
  team: { id: "team", key: "USG", name: "Unsold" }, state: { id: "state", name: "Todo", type: "unstarted" },
  priority: 2, createdAt: "2026-08-01T00:00:00.000Z", updatedAt: "2026-08-02T00:00:00.000Z", completedAt: null,
  dueDate: null, project: null, cycle: null, labels: { nodes: [] }, parent: null, relations: { nodes: [] }, inverseRelations: { nodes: [] },
});

let hub: ReturnType<typeof startGraphHub> | undefined;
afterEach(async () => { if (hub) await hub.stop(); hub = undefined; });

describe("linear graph hub", () => {
  test("allows only Tailscale-tagged non-destructive analysis and campaign starts without the action token", () => {
    const trusted = new Request("http://localhost/api/analysis-runs", { method: "POST", headers: { "X-Linear-Graph-Tailnet": "1" } });
    const campaign = new Request("http://localhost/api/campaigns", { method: "POST", headers: { "X-Linear-Graph-Tailnet": "1" } });
    const approval = new Request("http://localhost/api/bundles/x/approval", { method: "POST", headers: { "X-Linear-Graph-Tailnet": "1" } });
    expect(trustedNonDestructiveMutation(trusted, "/api/analysis-runs")).toBe(true);
    expect(trustedNonDestructiveMutation(campaign, "/api/campaigns")).toBe(true);
    expect(trustedNonDestructiveMutation(approval, "/api/bundles/x/approval")).toBe(false);
  });

  test("normalizes parent and blocking relations without inventing external nodes", () => {
    const parentRaw = raw("parent");
    const childRaw = { ...raw("child"), parent: { id: "parent" }, relations: { nodes: [{ type: "blocks", issue: { id: "missing" } }] } };
    const issues = [normalizeLinearIssue(parentRaw), normalizeLinearIssue(childRaw)];
    const edges = normalizeLinearEdges([parentRaw, childRaw], issues);
    expect(edges.map((edge) => [edge.kind, edge.sourceId, edge.targetId])).toEqual([["parent", "parent", "child"]]);
    expect(summarizeTeams(issues)).toEqual([{ id: "team", key: "USG", name: "Unsold", issueCount: 2, activeIssueCount: 2, completedIssueCount: 0, canceledIssueCount: 0 }]);
  });

  test("keeps complete team counts for every synced issue state", () => {
    const issues = [
      normalizeLinearIssue(raw("active")),
      normalizeLinearIssue({ ...raw("done"), state: { id: "state-done", name: "Done", type: "completed" } }),
      normalizeLinearIssue({ ...raw("cancelled"), state: { id: "state-cancelled", name: "Cancelled", type: "canceled" } }),
    ];
    expect(summarizeTeams(issues)).toEqual([{ id: "team", key: "USG", name: "Unsold", issueCount: 3, activeIssueCount: 1, completedIssueCount: 1, canceledIssueCount: 1 }]);
  });

  test("serves a read-only snapshot and protects mutations with the action token", async () => {
    if (process.env.LINEAR_GRAPH_HTTP_TEST !== "1") return;
    hub = startGraphHub({ actionToken: "secret", port: 0, linear: { async fetchIssues() { return { nodes: [raw("one")], pageInfo: { hasNextPage: false } }; } } });
    const unauthorized = await fetch(`${hub.url}/api/analysis-runs`, { method: "POST" });
    expect(unauthorized.status).toBe(401);
    const run = await fetch(`${hub.url}/api/analysis-runs`, { method: "POST", headers: { Authorization: "Bearer secret" } });
    expect(run.status).toBe(202);
    const body = await fetch(`${hub.url}/api/graph?view=universe`).then((response) => response.json());
    expect(body.snapshot.nodes[0].identifier).toBe("USG-one");
    expect(body.snapshot.zones.length).toBeGreaterThan(0);
    const topic = await fetch(`${hub.url}/api/issues/one/topic`, { method: "PUT", headers: { Authorization: "Bearer secret", "Content-Type": "application/json" }, body: JSON.stringify({ zone: "platform" }) });
    expect(topic.status).toBe(200);
  });

  test("accepts only validated Codex JSONL output and drops unknown suggestions", () => {
    expect(parseCodexEvents('{"type":"thread.started"}\n{"type":"item.completed","item":{"text":"{\\"assignments\\":[],\\"semanticEdges\\":[],\\"recommendations\\":[]}"}}')).toEqual({ assignments: [], semanticEdges: [], recommendations: [] });
    const issue = normalizeLinearIssue(raw("one"));
    const result = applyCodexAnalysis([issue], [], { assignments: [], semanticEdges: [{ sourceId: "one", targetId: "missing", confidence: 1, rationale: "bad" }], recommendations: [] }, issue.updatedAt, "run");
    expect(result.snapshot.edges).toEqual([]);
    expect(result.snapshot.teams).toEqual([{ id: "team", key: "USG", name: "Unsold", issueCount: 1, activeIssueCount: 1, completedIssueCount: 0, canceledIssueCount: 0 }]);
    const overridden = applyCodexAnalysis([{ ...issue, zone: "platform" }], [], { assignments: [{ issueId: "one", zone: "product", topicTags: [], confidence: 1, rationale: "different" }], semanticEdges: [], recommendations: [] }, issue.updatedAt, "run");
    expect(overridden.snapshot.nodes[0].zone).toBe("platform");
  });

  test("keeps Universe complete while Focus projects recommended neighborhoods", () => {
    const first = normalizeLinearIssue(raw("first"));
    const second = normalizeLinearIssue(raw("second"));
    const snapshot = applyCodexAnalysis([first, second], [], {
      assignments: [], semanticEdges: [], recommendations: [{
        issueId: first.id, rank: 1, score: 90, whyNow: "Ready", nextAction: "Start", evidenceIssueIds: [first.id], confidence: 0.9,
      }],
    }, first.updatedAt, "run").snapshot;
    const focus = focusSnapshot(snapshot);
    expect(snapshot.nodes.map((node) => node.id)).toEqual([first.id, second.id]);
    expect(focus.nodes.map((node) => node.id)).toEqual([first.id]);
    expect(focus.teams).toEqual(snapshot.teams);
  });

});
