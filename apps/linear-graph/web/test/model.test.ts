import { describe, expect, test } from "bun:test";
import { deterministicGridColumns, edgeLabel, emptyStateKind, graphLayoutName, issueNeighbors, matchesNode, selectionViewportAction, statusShape, teamColor, teamSummaries, topicColor, topicCounts, visibleGraph, zoneMetrics, type Filters } from "../src/model.js";
import { demoPayload } from "../src/protocol.js";

const base: Filters = { query: "", teams: new Set(), topics: new Set(), showSemantic: true, showCompleted: false };

describe("linear graph view model", () => {
  test("focus view keeps recommendations and one-hop context", () => {
    const graph = visibleGraph(demoPayload.snapshot, demoPayload.recommendations, base, "focus");
    expect(graph.nodes.map((node) => node.identifier)).toContain("CK-142");
    expect(graph.nodes.map((node) => node.identifier)).toContain("CK-138");
    expect(graph.edges.some((edge) => edge.kind === "blocks")).toBe(true);
  });

  test("universe filters by search, team, and topic", () => {
    const graph = visibleGraph(demoPayload.snapshot, demoPayload.recommendations, { ...base, query: "pricing", teams: new Set(["Expedition Insure"]), topics: new Set(["Operations"]) }, "universe");
    expect(graph.nodes.map((node) => node.identifier)).toEqual(["TRV-19", "TRV-22"]);
  });

  test("issue search matches the Linear team key", () => {
    const graph = visibleGraph({ ...demoPayload.snapshot, nodes: demoPayload.snapshot.nodes.map((node) => node.team === "Expedition Insure" ? { ...node, teamKey: "EXP" } : node) }, demoPayload.recommendations, { ...base, query: "EXP" }, "universe");
    expect(graph.nodes.map((node) => node.identifier)).toEqual(["TRV-19", "TRV-22"]);
  });

  test("Codex links can be hidden without removing Linear relations", () => {
    const graph = visibleGraph(demoPayload.snapshot, demoPayload.recommendations, { ...base, showSemantic: false }, "universe");
    expect(graph.edges.some((edge) => edge.sourceType === "codex")).toBe(false);
    expect(graph.edges.some((edge) => edge.sourceType === "linear")).toBe(true);
  });

  test("labels and neighbors preserve relationship semantics", () => {
    expect(edgeLabel({ kind: "semantic", sourceType: "codex" })).toBe("Codex link");
    expect(edgeLabel({ kind: "blocks", sourceType: "linear" })).toBe("Blocks");
    expect(issueNeighbors("demo-1", demoPayload.snapshot.edges)).toEqual(["demo-2", "demo-3"]);
  });

  test("zone counts include secondary topic tags", () => {
    const counts = topicCounts(demoPayload.snapshot.nodes, demoPayload.snapshot.zones);
    expect(counts.find((zone) => zone.name === "Product")?.issueCount).toBe(2);
    expect(counts.find((zone) => zone.name === "Operations")?.issueCount).toBe(2);
  });

  test("completed issues stay hidden by default", () => {
    const completed = { ...demoPayload.snapshot, nodes: [...demoPayload.snapshot.nodes, { ...demoPayload.snapshot.nodes[0], id: "done", identifier: "DONE-1", statusType: "completed" as const }] };
    expect(visibleGraph(completed, demoPayload.recommendations, base, "universe").nodes.some((node) => node.id === "done")).toBe(false);
    expect(visibleGraph(completed, demoPayload.recommendations, { ...base, showCompleted: true }, "universe").nodes.some((node) => node.id === "done")).toBe(true);
  });

  test("team metadata has deterministic colors and counts", () => {
    const summaries = teamSummaries(demoPayload.snapshot.nodes);
    expect(summaries.map((team) => [team.name, team.count])).toEqual([["CommonKit", 2], ["Expedition Insure", 2], ["Unsold", 2]]);
    expect(teamColor("Unsold")).toBe(teamColor("Unsold"));
    expect(teamColor("Unsold")).not.toBe(teamColor("CommonKit"));
    expect(teamSummaries([], demoPayload.snapshot.teams).map((team) => team.name)).toEqual(["CommonKit", "Expedition Insure", "Unsold"]);
  });

  test("empty-state kind distinguishes unavailable, filtered, and unprioritized work", () => {
    expect(emptyStateKind({ ...demoPayload.snapshot, nodes: [] }, [], base, "universe")).toBe("no-data");
    expect(emptyStateKind({ ...demoPayload.snapshot, nodes: [] }, [], base, "focus")).toBe("no-data");
    expect(emptyStateKind(demoPayload.snapshot, [], base, "focus")).toBe("no-recommendations");
    expect(emptyStateKind(demoPayload.snapshot, demoPayload.recommendations, { ...base, query: "does-not-exist" }, "universe")).toBe("filtered");
    const completed = { ...demoPayload.snapshot, nodes: demoPayload.snapshot.nodes.map((node) => ({ ...node, statusType: "completed" as const })) };
    expect(emptyStateKind(completed, demoPayload.recommendations, base, "universe")).toBe("completed-only");
  });

  test("layout policy stays fast and deterministic at universe scale", () => {
    expect(graphLayoutName("focus")).toBe("fcose");
    expect(graphLayoutName("universe")).toBe("grid");
    expect(deterministicGridColumns(0)).toBe(1);
    expect(deterministicGridColumns(4096)).toBe(64);
  });

  test("selection fits a changed ticket without recentering away from the fitted graph", () => {
    expect(selectionViewportAction(true, false)).toBe("fit");
    expect(selectionViewportAction(false, false)).toBe("preserve");
    expect(selectionViewportAction(true, true)).toBe("preserve");
  });

  test("topic fills and status shapes are deterministic and redundant", () => {
    const node = demoPayload.snapshot.nodes[0];
    expect(topicColor(node, demoPayload.snapshot.zones)).toBe("#56d6a0");
    expect(topicColor({ topic: "Missing" }, demoPayload.snapshot.zones)).toBe("#64748b");
    expect(statusShape("started")).toBe("round-rectangle");
    expect(statusShape("completed")).toBe("ellipse");
    expect(statusShape("canceled")).toBe("diamond");
  });

  test("long searches include descriptions while short team keys stay exact", () => {
    const node = demoPayload.snapshot.nodes[0];
    expect(matchesNode(node, { ...base, query: "interrupted target mutation" })).toBe(true);
    expect(matchesNode({ ...node, teamKey: "EXP", description: "An explicit unrelated detail" }, { ...base, query: "EXP" })).toBe(true);
    expect(matchesNode({ ...node, teamKey: "CK", description: "An explicit unrelated detail" }, { ...base, query: "EXP" })).toBe(false);
  });

  test("zone metrics include active counts and priority-weighted workload", () => {
    const metrics = zoneMetrics(demoPayload.snapshot.nodes, demoPayload.snapshot.zones);
    expect(metrics.find((zone) => zone.name === "Reliability")).toMatchObject({ issueCount: 1, activeCount: 1, weightedWorkload: 4 });
    expect(metrics.find((zone) => zone.name === "Release")).toMatchObject({ issueCount: 2, activeCount: 2, weightedWorkload: 7 });
    expect(metrics.every((zone) => zone.weightedWorkload >= zone.activeCount)).toBe(true);
  });

  test("Universe keeps explicit zone containers while filters still narrow issues", () => {
    const universe = visibleGraph(demoPayload.snapshot, demoPayload.recommendations, base, "universe");
    expect(universe.zones.map((zone) => zone.id)).toEqual(demoPayload.snapshot.zones.map((zone) => zone.id));
    const filtered = visibleGraph(demoPayload.snapshot, demoPayload.recommendations, { ...base, topics: new Set(["Product"]) }, "universe");
    expect(filtered.nodes.every((node) => node.topic === "Product" || node.topics?.includes("Product"))).toBe(true);
    expect(filtered.zones.map((zone) => zone.id)).toEqual(demoPayload.snapshot.zones.map((zone) => zone.id));
  });
});
