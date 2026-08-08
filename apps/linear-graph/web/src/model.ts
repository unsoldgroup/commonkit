import type { GraphEdge, GraphNode, GraphSnapshot, TeamMetadata, TopicZone, FocusRecommendation } from "./protocol.js";

export type ViewMode = "focus" | "universe";
export type GraphLayoutName = "fcose" | "grid";
export type Filters = { query: string; teams: Set<string>; topics: Set<string>; showSemantic: boolean; showCompleted: boolean };
export type TeamSummary = { id?: string; key?: string; name: string; count: number; activeCount?: number; completedCount?: number; canceledCount?: number; color: string };

const TEAM_COLORS = ["#80a7ff", "#56d6a0", "#f5bd5c", "#d39bff", "#ff8f82", "#6bd7e8", "#f18fc0", "#b8d879"] as const;

/** Stable color assignment: team order or issue arrival cannot change a team's color. */
export function teamColor(team: string) {
  let hash = 0;
  for (const character of team) hash = (hash * 31 + character.charCodeAt(0)) >>> 0;
  return TEAM_COLORS[hash % TEAM_COLORS.length];
}

export function teamSummaries(nodes: GraphNode[], metadata: TeamMetadata[] = []): TeamSummary[] {
  if (metadata.length) return metadata.slice().sort((left, right) => left.key.localeCompare(right.key) || left.id.localeCompare(right.id)).map((team) => ({ id: team.id, key: team.key, name: team.name, count: team.issueCount, activeCount: team.activeIssueCount, completedCount: team.completedIssueCount, canceledCount: team.canceledIssueCount, color: teamColor(team.key || team.name) }));
  const counts = new Map<string, number>();
  for (const node of nodes) counts.set(node.team, (counts.get(node.team) ?? 0) + 1);
  return [...counts.entries()].sort(([a], [b]) => a.localeCompare(b)).map(([name, count]) => ({ name, count, color: teamColor(name) }));
}

/** Focus benefits from semantic force placement; Universe needs bounded, deterministic placement. */
export const graphLayoutName = (view: ViewMode): GraphLayoutName => view === "focus" ? "fcose" : "grid";
export const deterministicGridColumns = (nodeCount: number) => Math.max(1, Math.ceil(Math.sqrt(Math.max(1, nodeCount))));

export const edgeLabel = (edge: Pick<GraphEdge, "kind" | "sourceType">) => {
  if (edge.sourceType === "codex" || edge.kind === "semantic") return "Codex link";
  return edge.kind === "blocks" ? "Blocks" : edge.kind === "parent" ? "Parent" : edge.kind === "duplicate" ? "Duplicate" : "Related";
};

export function matchesNode(node: GraphNode, filters: Filters) {
  const q = filters.query.trim().toLowerCase();
  const queryMatch = !q || [node.identifier, node.title, node.team, node.teamKey, node.project, node.repo, node.assignee, ...(node.topics ?? [])].filter(Boolean).some((value) => value!.toLowerCase().includes(q));
  const teamMatch = !filters.teams.size || filters.teams.has(node.team);
  const topicMatch = !filters.topics.size || filters.topics.has(node.topic ?? "") || (node.topics ?? []).some((topic) => filters.topics.has(topic));
  const completedMatch = filters.showCompleted || !["completed", "canceled"].includes(node.statusType ?? "");
  return queryMatch && teamMatch && topicMatch && completedMatch;
}

export function visibleGraph(snapshot: GraphSnapshot, recommendations: FocusRecommendation[], filters: Filters, view: ViewMode): { nodes: GraphNode[]; edges: GraphEdge[]; zones: TopicZone[] } {
  const focusedIds = new Set(recommendations.slice(0, 15).map((item) => item.issueId));
  const focusNeighbors = new Set(focusedIds);
  for (const edge of snapshot.edges) if (focusedIds.has(edge.source) || focusedIds.has(edge.target)) { focusNeighbors.add(edge.source); focusNeighbors.add(edge.target); }
  const nodes = snapshot.nodes.filter((node) => (view === "universe" || focusNeighbors.has(node.id)) && matchesNode(node, filters));
  const ids = new Set(nodes.map((node) => node.id));
  const edges = snapshot.edges.filter((edge) => ids.has(edge.source) && ids.has(edge.target) && (filters.showSemantic || edge.sourceType !== "codex"));
  const zoneIds = new Set(nodes.map((node) => node.topic).filter(Boolean));
  return { nodes, edges, zones: snapshot.zones.filter((zone) => zoneIds.has(zone.name)) };
}

export function recommendationFor(nodeId: string, recommendations: FocusRecommendation[]) { return recommendations.find((item) => item.issueId === nodeId); }
export function issueNeighbors(nodeId: string, edges: GraphEdge[]) { return edges.filter((edge) => edge.source === nodeId || edge.target === nodeId).map((edge) => edge.source === nodeId ? edge.target : edge.source); }
export function topicCounts(nodes: GraphNode[], zones: TopicZone[]) { return zones.map((zone) => ({ ...zone, issueCount: nodes.filter((node) => node.topic === zone.name || node.topics?.includes(zone.name)).length })); }
export const uniqueTeams = (nodes: GraphNode[]) => [...new Set(nodes.map((node) => node.team))].sort();
export const uniqueTopics = (nodes: GraphNode[]) => [...new Set(nodes.flatMap((node) => node.topics ?? (node.topic ? [node.topic] : [])))].sort();

export type EmptyStateKind = "no-data" | "no-recommendations" | "completed-only" | "filtered" | "empty";
export function emptyStateKind(snapshot: GraphSnapshot, recommendations: FocusRecommendation[], filters: Filters, view: ViewMode): EmptyStateKind {
  if (!snapshot.nodes.length) return "no-data";
  if (!filters.showCompleted && snapshot.nodes.every((node) => ["completed", "canceled"].includes(node.statusType ?? ""))) return "completed-only";
  if (view === "focus" && !recommendations.length) return "no-recommendations";
  const graph = visibleGraph(snapshot, recommendations, filters, view);
  if (!graph.nodes.length && (filters.query.trim() || filters.teams.size || filters.topics.size || !filters.showCompleted)) return "filtered";
  return "empty";
}
