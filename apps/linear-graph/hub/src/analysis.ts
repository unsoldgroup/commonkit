import { graphEdgeSchema, graphSnapshotSchema, type CodexAnalysis, type GraphEdge, type Issue, type FocusRecommendation, DEFAULT_ZONES, type ZoneId } from "@commonkit/linear-graph-protocol";
import { hashAnalysisInput, inferTopicAssignment, summarizeTeams } from "./normalizer.js";

export function applyCodexAnalysis(issues: Issue[], linearEdges: GraphEdge[], analysis: CodexAnalysis, syncedAt: string, runId: string): { snapshot: ReturnType<typeof graphSnapshotSchema.parse>; inputHash: string } {
  const ids = new Set(issues.map((issue) => issue.id));
  const byId = new Map(issues.map((issue) => [issue.id, issue]));
  const zoneIds = new Set(DEFAULT_ZONES.map((zone) => zone.id));
  const assignments = new Map(analysis.assignments.filter((item) => ids.has(item.issueId) && zoneIds.has(item.zone) && item.zone !== "unsorted").map((item) => [item.issueId, item]));
  const nodes = issues.map((issue) => {
    const assignment = assignments.get(issue.id);
    if (issue.zone !== "unsorted") return issue;
    if (assignment) return { ...issue, zone: assignment.zone as ZoneId, topicTags: assignment.topicTags };
    const fallback = inferTopicAssignment(issue);
    return fallback ? { ...issue, zone: fallback.zone as ZoneId, topicTags: fallback.topicTags } : issue;
  });
  const edges = new Map(linearEdges.map((edge) => [edge.id, edge]));
  for (const suggestion of analysis.semanticEdges) {
    if (suggestion.confidence < 0.75 || !ids.has(suggestion.sourceId) || !ids.has(suggestion.targetId) || suggestion.sourceId === suggestion.targetId) continue;
    const edge = graphEdgeSchema.parse({ id: `semantic:${suggestion.sourceId}:${suggestion.targetId}`, sourceId: suggestion.sourceId, targetId: suggestion.targetId, kind: "semantic", source: "codex", confidence: suggestion.confidence, rationale: suggestion.rationale });
    edges.set(edge.id, edge);
  }
  const recommendationById = new Map<string, FocusRecommendation>();
  for (const recommendation of analysis.recommendations) {
    if (!ids.has(recommendation.issueId)) continue;
    const evidence = recommendation.evidenceIssueIds.filter((id) => ids.has(id));
    if (!recommendationById.has(recommendation.issueId)) recommendationById.set(recommendation.issueId, { ...recommendation, evidenceIssueIds: evidence });
  }
  const recommendations = [...recommendationById.values()].sort((a, b) => b.score - a.score || a.issueId.localeCompare(b.issueId)).slice(0, 50).map((item, index) => ({ ...item, rank: index + 1 }));
  const snapshot = graphSnapshotSchema.parse({ generatedAt: new Date().toISOString(), syncedAt, stale: false, nodes, edges: [...edges.values()], teams: summarizeTeams(nodes), zones: DEFAULT_ZONES, recommendations, latestAnalysisRunId: runId });
  return { snapshot, inputHash: hashAnalysisInput({ issues, linearEdges }) };
}

export function focusSnapshot(snapshot: ReturnType<typeof graphSnapshotSchema.parse>, limit = 15) {
  const recommendations = snapshot.recommendations.slice(0, limit);
  const selected = new Set(recommendations.map((item) => item.issueId));
  for (const edge of snapshot.edges) {
    if (selected.has(edge.sourceId) || selected.has(edge.targetId)) { selected.add(edge.sourceId); selected.add(edge.targetId); }
  }
  return { ...snapshot, nodes: snapshot.nodes.filter((node) => selected.has(node.id)), edges: snapshot.edges.filter((edge) => selected.has(edge.sourceId) && selected.has(edge.targetId)) };
}

export function defaultRecommendations(issues: Issue[]): FocusRecommendation[] {
  return issues.filter((issue) => !["completed", "canceled"].includes(issue.status.type)).sort((a, b) => a.priority - b.priority || a.updatedAt.localeCompare(b.updatedAt)).slice(0, 15).map((issue, index) => ({ issueId: issue.id, rank: index + 1, score: Math.max(1, 100 - index * 4), whyNow: issue.dueDate ? `Due ${issue.dueDate}` : "Open work in the current universe", nextAction: `Open ${issue.identifier} and define the next concrete step`, evidenceIssueIds: [issue.id], confidence: 0.25 }));
}
