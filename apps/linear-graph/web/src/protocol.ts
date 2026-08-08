import type { AnalysisRun as ProtocolAnalysisRun, FocusBrief as ProtocolFocusBrief, FocusRecommendation as ProtocolFocusRecommendation, GraphSnapshot as ProtocolGraphSnapshot, Issue, TeamSummary as ProtocolTeamSummary, TopicZone as ProtocolTopicZone } from "@commonkit/linear-graph-protocol";

/** UI-shaped adapter over the canonical @commonkit/linear-graph-protocol types. */
export type EdgeKind = "parent" | "blocks" | "related" | "duplicate" | "semantic";
export type EdgeSource = "linear" | "codex";

export type GraphNode = {
  id: string;
  identifier: string;
  title: string;
  url?: string;
  team: string;
  teamKey?: string;
  teamId?: string;
  project?: string;
  repo?: string;
  status: string;
  statusType?: "backlog" | "unstarted" | "started" | "completed" | "canceled";
  priority?: number;
  priorityLabel?: string;
  dueDate?: string;
  updatedAt?: string;
  topic?: string;
  topicId?: string;
  topics?: string[];
  assignee?: string;
  estimate?: number;
  focusScore?: number;
  whyNow?: string;
  nextAction?: string;
};

export type GraphEdge = {
  id: string;
  source: string;
  target: string;
  kind: EdgeKind;
  sourceType?: EdgeSource;
  confidence?: number;
  reason?: string;
};

export type TopicZone = { id: string; name: string; description?: string; color: string; issueCount?: number };
export type TeamMetadata = { id: string; key: string; name: string; issueCount: number; activeIssueCount: number; completedIssueCount: number; canceledIssueCount: number };
export type GraphSnapshot = {
  generatedAt: string;
  syncedAt?: string;
  stale?: boolean;
  nodes: GraphNode[];
  edges: GraphEdge[];
  teams: TeamMetadata[];
  zones: TopicZone[];
};
export type FocusRecommendation = {
  issueId: string;
  rank: number;
  whyNow: string;
  nextAction: string;
  evidenceIssueIds?: string[];
  confidence?: number;
};
export type FocusBrief = { text: string; updatedAt?: string };
export type AnalysisRun = { id: string; startedAt: string; completedAt?: string; status: "running" | "completed" | "failed"; issueCount?: number; error?: string };
export type GraphPayload = { snapshot: GraphSnapshot; recommendations: FocusRecommendation[]; brief?: FocusBrief; analysis?: AnalysisRun };

export function normalizeIssue(issue: Issue): GraphNode {
  return {
    id: issue.id, identifier: issue.identifier, title: issue.title, url: issue.url,
    team: issue.team.name, teamKey: issue.team.key, teamId: issue.team.id, project: issue.project?.name, repo: issue.repo ?? undefined,
    status: issue.status.name, statusType: issue.status.type, priority: issue.priority,
    priorityLabel: issue.priority === 1 ? "Urgent" : issue.priority === 2 ? "High" : issue.priority === 3 ? "Medium" : issue.priority === 4 ? "Low" : undefined,
    dueDate: issue.dueDate ?? undefined, updatedAt: issue.updatedAt, topic: issue.zone, topicId: issue.zone,
    topics: issue.topicTags, assignee: undefined,
  };
}

export function normalizeSnapshot(snapshot: ProtocolGraphSnapshot): GraphSnapshot {
  const zones = snapshot.zones.map((zone: ProtocolTopicZone) => ({ id: zone.id, name: zone.label, description: zone.description, color: zone.color }));
  const zoneNames = new Map(zones.map((zone) => [zone.id, zone.name]));
  const nodes = snapshot.nodes.map(normalizeIssue).map((node) => ({ ...node, topic: zoneNames.get(node.topicId ?? "") ?? node.topic, topics: (node.topics ?? []).map((topic) => zoneNames.get(topic) ?? topic) }));
  return {
    generatedAt: snapshot.generatedAt, syncedAt: snapshot.syncedAt, stale: snapshot.stale,
    nodes,
    edges: snapshot.edges.map((edge) => ({ id: edge.id, source: edge.sourceId, target: edge.targetId, kind: edge.kind, sourceType: edge.source, confidence: edge.confidence, reason: edge.rationale })),
    teams: ((snapshot as ProtocolGraphSnapshot & { teams?: ProtocolTeamSummary[] }).teams ?? []).map((team) => ({ id: team.id, key: team.key, name: team.name, issueCount: team.issueCount, activeIssueCount: team.activeIssueCount, completedIssueCount: team.completedIssueCount, canceledIssueCount: team.canceledIssueCount })),
    zones,
  };
}

export function normalizePayload(snapshot: ProtocolGraphSnapshot, brief?: ProtocolFocusBrief, analysis?: ProtocolAnalysisRun): GraphPayload {
  return {
    snapshot: normalizeSnapshot(snapshot),
    recommendations: snapshot.recommendations.map((recommendation: ProtocolFocusRecommendation) => ({ issueId: recommendation.issueId, rank: recommendation.rank, whyNow: recommendation.whyNow, nextAction: recommendation.nextAction, evidenceIssueIds: recommendation.evidenceIssueIds, confidence: recommendation.confidence })),
    brief: brief ? { text: brief.text, updatedAt: brief.updatedAt } : undefined,
    analysis: analysis ? { id: analysis.id, startedAt: analysis.startedAt, completedAt: analysis.completedAt ?? undefined, status: analysis.status, issueCount: analysis.issueCount, error: analysis.error ?? undefined } : undefined,
  };
}

export const demoPayload: GraphPayload = {
  snapshot: {
    generatedAt: "2026-08-07T07:00:00.000Z",
    teams: [
      { id: "commonkit", key: "CK", name: "CommonKit", issueCount: 2, activeIssueCount: 2, completedIssueCount: 0, canceledIssueCount: 0 },
      { id: "expedition", key: "TRV", name: "Expedition Insure", issueCount: 2, activeIssueCount: 2, completedIssueCount: 0, canceledIssueCount: 0 },
      { id: "unsold", key: "USG", name: "Unsold", issueCount: 2, activeIssueCount: 2, completedIssueCount: 0, canceledIssueCount: 0 },
    ],
    nodes: [
      { id: "demo-1", identifier: "CK-142", title: "Harden portable receipt recovery", team: "CommonKit", project: "v1 stabilization", repo: "commonkit", status: "In Progress", statusType: "started", priority: 1, priorityLabel: "Urgent", topic: "Reliability", topics: ["Reliability", "Release"], focusScore: 98, whyNow: "Blocks the release validation path.", nextAction: "Add the missing recovery fixture." },
      { id: "demo-2", identifier: "CK-138", title: "Document target mutation boundaries", team: "CommonKit", project: "v1 stabilization", repo: "commonkit", status: "Todo", statusType: "unstarted", priority: 2, priorityLabel: "High", topic: "Release", topics: ["Release"], focusScore: 72 },
      { id: "demo-3", identifier: "USG-41", title: "Map graph topic zones to repositories", team: "Unsold", project: "Work graph", repo: "linear-issues-graph", status: "In Review", statusType: "started", priority: 2, priorityLabel: "High", topic: "Product", topics: ["Product", "Automation"], focusScore: 84, whyNow: "The graph is the shared planning surface.", nextAction: "Review the zone override rules." },
      { id: "demo-4", identifier: "USG-44", title: "Schedule Codex graph refresh", team: "Unsold", project: "Work graph", repo: "linear-issues-graph", status: "Backlog", statusType: "backlog", priority: 3, priorityLabel: "Medium", topic: "Automation", topics: ["Automation"], focusScore: 51 },
      { id: "demo-5", identifier: "TRV-19", title: "Refresh expedition pricing cache", team: "Expedition Insure", project: "Pricing", repo: "expedition-insure", status: "Todo", statusType: "unstarted", priority: 2, priorityLabel: "High", topic: "Operations", topics: ["Operations"], focusScore: 66 },
      { id: "demo-6", identifier: "TRV-22", title: "Add stale-price warning to quote", team: "Expedition Insure", project: "Pricing", repo: "expedition-insure", status: "Todo", statusType: "unstarted", priority: 3, priorityLabel: "Medium", topic: "Product", topics: ["Product", "Operations"], focusScore: 48 },
    ],
    edges: [
      { id: "e1", source: "demo-1", target: "demo-2", kind: "blocks", sourceType: "linear" },
      { id: "e2", source: "demo-1", target: "demo-3", kind: "semantic", sourceType: "codex", confidence: 0.86, reason: "Both describe the planning and validation surface." },
      { id: "e3", source: "demo-3", target: "demo-4", kind: "parent", sourceType: "linear" },
      { id: "e4", source: "demo-5", target: "demo-6", kind: "related", sourceType: "linear" },
    ],
    zones: [
      { id: "reliability", name: "Reliability", color: "#56d6a0", description: "Recovery, correctness, and trust" },
      { id: "release", name: "Release", color: "#80a7ff", description: "Shipping and validation" },
      { id: "product", name: "Product", color: "#f5bd5c", description: "User-facing capability" },
      { id: "automation", name: "Automation", color: "#d39bff", description: "Agents and recurring work" },
      { id: "operations", name: "Operations", color: "#ff8f82", description: "Live systems and maintenance" },
    ],
  },
  recommendations: [
    { issueId: "demo-1", rank: 1, whyNow: "Blocks the release validation path.", nextAction: "Add the missing recovery fixture.", confidence: 0.96, evidenceIssueIds: ["demo-2"] },
    { issueId: "demo-3", rank: 2, whyNow: "The graph is the shared planning surface.", nextAction: "Review the zone override rules.", confidence: 0.89, evidenceIssueIds: ["demo-4"] },
    { issueId: "demo-5", rank: 3, whyNow: "Pricing freshness affects active quotes.", nextAction: "Run the live cache refresh check.", confidence: 0.79 },
  ],
  brief: { text: "Two connected workstreams are ready to move: stabilize recovery, then finish the shared graph taxonomy." },
};
