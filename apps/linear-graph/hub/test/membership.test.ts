import { expect, test } from "bun:test";
import { graphSnapshotSchema, DEFAULT_ZONES } from "@commonkit/linear-graph-protocol";
import { normalizeLinearIssue } from "../src/normalizer.js";
import { projectMembership } from "../src/membership.js";
import { focusSnapshot } from "../src/analysis.js";
import { GraphStore } from "../src/store.js";
import { startGraphHub } from "../src/server.js";

test("read membership prunes closed endpoints, retains history and reconciles counts without changing storage", async () => {
  const nodes = ["unstarted", "completed", "canceled"].map((type, i) => normalizeLinearIssue({
    id: String(i), identifier: `USG-${i + 1}`, title: "Issue", description: null,
    url: `https://linear.app/issue/USG-${i + 1}`, team: { id: "t", key: "USG", name: "Unsold" },
    state: { id: type, name: type, type }, priority: 2, createdAt: "2026-11-05T00:00:00Z",
    updatedAt: "2026-11-05T00:00:00Z", labels: { nodes: [] },
  }));
  const snapshot = graphSnapshotSchema.parse({ generatedAt: "2026-11-05T00:00:00Z", syncedAt: "2026-11-05T00:00:00Z", stale: false,
    nodes, zones: DEFAULT_ZONES, teams: [], recommendations: [{ issueId: "0", rank: 1, score: 100,
      whyNow: "Open work", nextAction: "Start", evidenceIssueIds: ["0"], confidence: 1 }], latestAnalysisRunId: null,
    edges: [{ id: "e", sourceId: "0", targetId: "1", kind: "parent", source: "linear", confidence: 1, rationale: "Parent" }],
  });
  expect(projectMembership(snapshot).nodes).toHaveLength(1);
  expect(projectMembership(snapshot).edges).toHaveLength(0);
  expect(projectMembership(snapshot).teams[0].issueCount).toBe(1);
  expect(projectMembership(snapshot, true).teams[0].issueCount).toBe(3);
  const store = new GraphStore(":memory:"); store.saveSnapshot(snapshot);
  const hub = startGraphHub({ store, port: 0, actionToken: "fixture" });
  try {
    for (const history of [false, true, false]) {
      const body = await (await fetch(`${hub.url}/api/graph?history=${history}`)).json();
      expect(body.snapshot.nodes).toHaveLength(history ? 3 : 1);
      expect(body.snapshot.teams[0].issueCount).toBe(body.snapshot.nodes.length);
      expect(body.snapshot.edges).toHaveLength(history ? 1 : 0);
    }
    expect(store.loadSnapshot()?.nodes).toHaveLength(3);
    expect(store.loadSnapshot()?.edges).toHaveLength(1);
    const focus = focusSnapshot(projectMembership(snapshot));
    expect(focus.nodes.map((node) => node.id)).toEqual(["0"]);
    expect(focus.edges).toEqual([]);
    expect(focus.teams[0].issueCount).toBe(1);
    const body = await (await fetch(`${hub.url}/api/graph?view=focus`)).json();
    expect(body.snapshot.nodes.map((node: { id: string }) => node.id)).toEqual(["0"]);
    expect(body.snapshot.edges).toEqual([]);
  } finally { await hub.stop(); }
});
