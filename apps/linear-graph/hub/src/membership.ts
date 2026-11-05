import type { GraphSnapshot } from "@commonkit/linear-graph-protocol";
import { summarizeTeams } from "./normalizer.js";

/** Read projection only: the stored snapshot retains history and all relationships. */
export function projectMembership(snapshot: GraphSnapshot, history = false): GraphSnapshot {
  const nodes = snapshot.nodes.filter((node) => history || !["completed", "canceled"].includes(node.status.type));
  const ids = new Set(nodes.map((node) => node.id));
  return {
    ...snapshot,
    nodes,
    edges: snapshot.edges.filter((edge) => ids.has(edge.sourceId) && ids.has(edge.targetId)),
    teams: summarizeTeams(nodes),
    recommendations: snapshot.recommendations.filter((item) => ids.has(item.issueId)),
  };
}
