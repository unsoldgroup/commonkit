import { createHash } from "node:crypto";
import { graphEdgeSchema, issueSchema, teamSummarySchema, DEFAULT_ZONES, type GraphEdge, type Issue, type TeamSummary, type ZoneId } from "@commonkit/linear-graph-protocol";

export type LinearIssueInput = Record<string, unknown>;

const asRecord = (value: unknown): Record<string, unknown> => value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
const text = (value: unknown, fallback = "") => typeof value === "string" ? value : fallback;
const idOf = (value: unknown) => text(asRecord(value).id) || text(value);
const iso = (value: unknown) => {
  const parsed = new Date(text(value));
  if (Number.isNaN(parsed.getTime())) throw new Error("Linear issue has an invalid timestamp");
  return parsed.toISOString();
};
const statusType = (value: unknown): "backlog" | "unstarted" | "started" | "completed" | "canceled" => {
  const candidate = text(value);
  return ["backlog", "unstarted", "started", "completed", "canceled"].includes(candidate)
    ? candidate as "backlog" | "unstarted" | "started" | "completed" | "canceled"
    : "backlog";
};
const relationNodes = (value: unknown): unknown[] => {
  const nodes = asRecord(value).nodes;
  return Array.isArray(nodes) ? nodes : [];
};

export function normalizeLinearIssue(raw: LinearIssueInput, overrides: Map<string, ZoneId> = new Map()): Issue {
  const team = asRecord(raw.team);
  const project = raw.project ? asRecord(raw.project) : null;
  const status = asRecord(raw.state ?? raw.status);
  const labels = relationNodes(raw.labels).map((label) => text(asRecord(label).name)).filter(Boolean);
  const cycleRaw = raw.cycle ? asRecord(raw.cycle) : null;
  const cycleId = text(cycleRaw?.id);
  const cycleName = text(cycleRaw?.name);
  const issue = {
    id: text(raw.id), identifier: text(raw.identifier), title: text(raw.title),
    description: raw.description == null ? null : text(raw.description), url: text(raw.url),
    team: { id: text(team.id), key: text(team.key), name: text(team.name) },
    project: project ? { id: text(project.id), name: text(project.name), ...(project.url ? { url: text(project.url) } : {}) } : null,
    status: { id: text(status.id), name: text(status.name), type: statusType(status.type) },
    priority: typeof raw.priority === "number" ? raw.priority : 0, labels,
    cycle: cycleId && cycleName ? { id: cycleId, name: cycleName, ...(typeof cycleRaw?.number === "number" ? { number: cycleRaw.number } : {}) } : null,
    parentId: raw.parent ? idOf(raw.parent) || null : null,
    dueDate: raw.dueDate == null ? null : text(raw.dueDate),
    createdAt: iso(raw.createdAt), updatedAt: iso(raw.updatedAt), completedAt: raw.completedAt == null ? null : iso(raw.completedAt),
    repo: raw.repo == null ? null : text(raw.repo),
    zone: overrides.get(text(raw.id)) ?? "unsorted", topicTags: [],
  };
  return issueSchema.parse(issue);
}

export function normalizeLinearEdges(rawIssues: LinearIssueInput[], issues: Issue[]): GraphEdge[] {
  const valid = new Set(issues.map((issue) => issue.id));
  const byId = new Map(rawIssues.map((raw) => [text(raw.id), raw]));
  const output = new Map<string, GraphEdge>();
  const add = (sourceId: string, targetId: string, kind: GraphEdge["kind"], source: GraphEdge["source"] = "linear") => {
    if (!valid.has(sourceId) || !valid.has(targetId) || sourceId === targetId) return;
    const id = `${kind}:${sourceId}:${targetId}`;
    output.set(id, graphEdgeSchema.parse({ id, sourceId, targetId, kind, source }));
  };
  for (const issue of issues) {
    if (issue.parentId) add(issue.parentId, issue.id, "parent");
    const raw = byId.get(issue.id);
    for (const relation of [...relationNodes(raw?.relations), ...relationNodes(raw?.inverseRelations)]) {
      const relationRecord = asRecord(relation);
      const relatedId = idOf(relationRecord.issue ?? relationRecord.relatedIssue);
      const type = text(relationRecord.type, "related").toLowerCase();
      const kind: GraphEdge["kind"] = type === "blocks" ? "blocks" : type === "duplicate" ? "duplicate" : "related";
      add(issue.id, relatedId, kind);
    }
  }
  return [...output.values()].sort((a, b) => a.id.localeCompare(b.id));
}

export function summarizeTeams(issues: Issue[]): TeamSummary[] {
  const summaries = new Map<string, TeamSummary>();
  for (const issue of issues) {
    const existing = summaries.get(issue.team.id) ?? {
      id: issue.team.id, key: issue.team.key, name: issue.team.name,
      issueCount: 0, activeIssueCount: 0, completedIssueCount: 0, canceledIssueCount: 0,
    };
    existing.issueCount += 1;
    if (issue.status.type === "completed") existing.completedIssueCount += 1;
    else if (issue.status.type === "canceled") existing.canceledIssueCount += 1;
    else existing.activeIssueCount += 1;
    summaries.set(issue.team.id, existing);
  }
  return [...summaries.values()]
    .map((summary) => teamSummarySchema.parse(summary))
    .sort((left, right) => left.key.localeCompare(right.key) || left.id.localeCompare(right.id));
}

export function hashAnalysisInput(value: unknown): string {
  return createHash("sha256").update(JSON.stringify(value)).digest("hex");
}

export { DEFAULT_ZONES };
