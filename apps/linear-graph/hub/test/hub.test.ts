import { afterEach, describe, expect, test } from "bun:test";
import { applyCodexAnalysis, focusSnapshot } from "../src/analysis.js";
import { parseCodexEvents, runCodexAnalysis, sanitizeAnalysis } from "../src/codex.js";
import { inferTopicAssignment, normalizeLinearEdges, normalizeLinearIssue, summarizeTeams } from "../src/normalizer.js";
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
    const sameOrigin = new Request("https://graph.unsold.cloud/api/analysis-runs", { method: "POST", headers: { Origin: "https://graph.unsold.cloud" } });
    const login = new Request("https://graph.unsold.cloud/api/codex/login", { method: "POST", headers: { Origin: "https://graph.unsold.cloud" } });
    const approval = new Request("http://localhost/api/bundles/x/approval", { method: "POST", headers: { "X-Linear-Graph-Tailnet": "1" } });
    expect(trustedNonDestructiveMutation(trusted, "/api/analysis-runs")).toBe(true);
    expect(trustedNonDestructiveMutation(campaign, "/api/campaigns")).toBe(true);
    expect(trustedNonDestructiveMutation(sameOrigin, "/api/analysis-runs")).toBe(true);
    expect(trustedNonDestructiveMutation(login, "/api/codex/login")).toBe(true);
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

  test("names the timeout instead of blaming the output when the child is killed", async () => {
    // A killed child still reports exitCode 0, so the truncated stream used to
    // surface as "Codex returned no structured analysis".
    let resolveExit: (code: number) => void = () => {};
    const child = {
      stdin: { write: () => {}, end: () => {} },
      stdout: undefined, stderr: undefined,
      exited: new Promise<number>((resolve) => { resolveExit = resolve; }),
      kill: () => resolveExit(0),
    };
    await expect(
      runCodexAnalysis({ issues: [], edges: [] }, { timeoutMs: 10, spawn: () => child as never }),
    ).rejects.toThrow(/timed out after 10ms/);
  });

  test("sends the complete active universe and all connected edges to Codex", async () => {
    const issues = Array.from({ length: 701 }, (_, index) => normalizeLinearIssue(raw(`full-${index}`)));
    const edges = Array.from({ length: 3001 }, (_, index) => ({ id: `edge-${index}`, sourceId: issues[0]!.id, targetId: issues[1]!.id, kind: "related", source: "linear" }));
    let prompt = "";
    const stream = (text: string) => new ReadableStream<Uint8Array>({ start(controller) { controller.enqueue(new TextEncoder().encode(text)); controller.close(); } });
    await runCodexAnalysis({ issues, edges }, {
      spawn: (_command, options) => ({
        stdin: { write(value: string) { prompt = value; }, end() {} },
        stdout: stream(JSON.stringify({ brief: "Full universe reviewed.", assignments: [], semanticEdges: [], recommendations: [], triageDecisions: [] })),
        stderr: stream(""), exited: Promise.resolve(0), kill() {},
      } as never),
    });
    const input = JSON.parse(prompt.split("\n").at(-1)!);
    expect(input.issues).toHaveLength(701);
    expect(input.edges).toHaveLength(3001);
  });

  test("keeps a Codex run alive when individual rows are malformed", () => {
    const assignment = (issueId: string, zone: string) => ({ issueId, zone, topicTags: ["quote"], confidence: 0.8, rationale: "why" });
    const result = sanitizeAnalysis({
      assignments: [
        assignment("issue-1", "Product"),          // prose case, coerced
        assignment("issue-2", "Quote pipeline"),    // not a real zone, becomes unsorted
        assignment("issue-3", "platform"),          // already valid
        { issueId: "issue-4" },                     // malformed, dropped
      ],
      semanticEdges: [
        { sourceId: "issue-1", targetId: "issue-3", confidence: 0.9, rationale: "shared carrier path" },
        { sourceId: "issue-1" },                    // malformed, dropped
      ],
      recommendations: [{ issueId: "issue-1", rank: 1, score: 90, whyNow: "now", nextAction: "do it", evidenceIssueIds: [], confidence: 0.9 }],
    });
    expect(result.assignments.map((item) => [item.issueId, item.zone])).toEqual([
      ["issue-1", "product"], ["issue-2", "unsorted"], ["issue-3", "platform"],
    ]);
    expect(result.semanticEdges).toHaveLength(1);
    expect(result.recommendations).toHaveLength(1);
  });

  test("assigns unsorted issues to deterministic topic zones from metadata", () => {
    const security = normalizeLinearIssue({ ...raw("security", "Rotate OAuth token"), labels: { nodes: [{ name: "credentials" }] }, project: { id: "p", name: "Access control" } });
    const docs = normalizeLinearIssue({ ...raw("docs", "Write migration guide"), labels: { nodes: [{ name: "documentation" }] }, project: { id: "p", name: "Knowledge base" } });
    expect(inferTopicAssignment(security)?.zone).toBe("security");
    expect(inferTopicAssignment(docs)?.zone).toBe("documentation");
    expect(inferTopicAssignment({ ...security, zone: "product" })).toBeNull();
  });

  test("uses fallback zones only when Codex and manual assignment are absent", () => {
    const inferred = normalizeLinearIssue({ ...raw("inferred", "Deploy API worker") });
    const manual = normalizeLinearIssue({ ...raw("manual", "Deploy API worker") }, new Map([["manual", "product"]]));
    const snapshot = applyCodexAnalysis([inferred, manual], [], { assignments: [{ issueId: inferred.id, zone: "security", topicTags: ["reviewed"], confidence: 1, rationale: "review" }], semanticEdges: [], recommendations: [] }, inferred.updatedAt, "run").snapshot;
    expect(snapshot.nodes.find((node) => node.id === inferred.id)?.zone).toBe("security");
    expect(snapshot.nodes.find((node) => node.id === manual.id)?.zone).toBe("product");
    const fallback = applyCodexAnalysis([normalizeLinearIssue({ ...raw("fallback", "Deploy API worker") })], [], { assignments: [], semanticEdges: [], recommendations: [] }, inferred.updatedAt, "run").snapshot;
    expect(fallback.nodes[0]?.zone).toBe("platform");
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

  test("retains a Codex-produced brief and triage decisions", () => {
    const result = sanitizeAnalysis({
      brief: "Codex says to ship the ready recovery path first.",
      assignments: [], semanticEdges: [], recommendations: [],
      triageDecisions: [{ issueId: "one", disposition: "ready", rationale: "Concrete next step", confidence: 0.9, evidenceIssueIds: [], nextAction: "Implement" }],
    });
    expect(result.brief).toBe("Codex says to ship the ready recovery path first.");
    expect(result.triageDecisions?.[0]?.disposition).toBe("ready");
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
