import type { AnalysisRun, FocusBrief, FocusRecommendation, GraphEdge, GraphSnapshot, Issue, ZoneId } from "@commonkit/linear-graph-protocol";
import { inferTopicAssignment } from "./normalizer.js";
import { redactSensitiveText } from "./redaction.js";

/** A snapshot older than this is stale: the frame falls back to plain Linear priority. */
export const STALE_AFTER_HOURS = 36;

const closed = (issue: Issue) => issue.status.type === "completed" || issue.status.type === "canceled";
const STRUCTURAL: ReadonlySet<GraphEdge["kind"]> = new Set(["parent", "blocks", "related", "duplicate"]);
// ponytail: a component above this size is a "related" hairball, not a topic; its members keep their own inferred zone.
const MAX_ZONE_COMPONENT = 40;

/**
 * Zones follow Linear's own structure: issues joined by parent, blocks or related
 * links share one zone, chosen by vote (manual overrides weigh 3, keyword inference 1).
 */
export function assignRelationZones(issues: Issue[], edges: GraphEdge[]): Issue[] {
  const parent = new Map(issues.map((issue) => [issue.id, issue.id]));
  const find = (id: string): string => { let root = id; while (parent.get(root) !== root) root = parent.get(root)!; parent.set(id, root); return root; };
  for (const edge of edges) {
    if (!STRUCTURAL.has(edge.kind) || !parent.has(edge.sourceId) || !parent.has(edge.targetId)) continue;
    parent.set(find(edge.sourceId), find(edge.targetId));
  }
  const inferred = new Map(issues.map((issue) => [issue.id, issue.zone !== "unsorted" ? null : inferTopicAssignment(issue)]));
  const members = new Map<string, Issue[]>();
  for (const issue of issues) members.set(find(issue.id), [...(members.get(find(issue.id)) ?? []), issue]);
  const winner = new Map<string, ZoneId>();
  for (const [root, group] of members) {
    if (group.length < 2 || group.length > MAX_ZONE_COMPONENT) continue;
    const votes = new Map<ZoneId, number>();
    for (const issue of group) {
      const zone = issue.zone !== "unsorted" ? issue.zone : inferred.get(issue.id)?.zone;
      if (zone && zone !== "unsorted") votes.set(zone, (votes.get(zone) ?? 0) + (issue.zone !== "unsorted" ? 3 : 1));
    }
    const best = [...votes.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))[0];
    if (best) winner.set(root, best[0]);
  }
  return issues.map((issue) => {
    if (issue.zone !== "unsorted") return issue; // manual topic override wins
    const own = inferred.get(issue.id);
    const zone = winner.get(find(issue.id)) ?? own?.zone;
    return zone ? { ...issue, zone, topicTags: own?.topicTags ?? issue.topicTags } : issue;
  });
}

/** Issue identifiers Al named in a manual focus brief, in the order he wrote them. */
export function pinnedRefs(brief: FocusBrief | null | undefined): string[] {
  if (!brief || brief.source !== "manual") return [];
  return [...new Set(brief.text.toUpperCase().match(/\b[A-Z][A-Z0-9]{1,9}-\d+\b/g) ?? [])];
}

type RelationKind = "blocked_by" | "blocks" | "parent" | "child" | "related" | "duplicate";
const RELATION_ORDER: readonly RelationKind[] = ["blocked_by", "blocks", "parent", "child", "related", "duplicate"];

function relationIndex(edges: GraphEdge[]) {
  const index = new Map<string, Array<{ id: string; kind: RelationKind }>>();
  const push = (from: string, id: string, kind: RelationKind) => index.set(from, [...(index.get(from) ?? []), { id, kind }]);
  for (const edge of edges) {
    if (edge.kind === "parent") { push(edge.targetId, edge.sourceId, "parent"); push(edge.sourceId, edge.targetId, "child"); }
    else if (edge.kind === "blocks") { push(edge.sourceId, edge.targetId, "blocks"); push(edge.targetId, edge.sourceId, "blocked_by"); }
    else if (edge.kind === "related" || edge.kind === "duplicate") { push(edge.sourceId, edge.targetId, edge.kind); push(edge.targetId, edge.sourceId, edge.kind); }
  }
  return index;
}

export interface RankedIssue {
  issue: Issue;
  pinned: boolean;
  relations: Array<{ issue: Issue; kind: RelationKind }>;
  whyNow: string;
  nextAction: string;
}

const STATUS_RANK = { started: 0, unstarted: 1, backlog: 2, completed: 3, canceled: 3 } as const;
const PRIORITY_LABEL = ["No priority", "Urgent", "High", "Medium", "Low"];

/**
 * Deterministic next-up order: focus-brief pins, then unblocked before blocked,
 * Linear priority (urgent first, none last), how many open issues it unblocks,
 * in-progress before todo before backlog, due date, most recently updated.
 */
export function rankIssues(issues: Issue[], edges: GraphEdge[], brief?: FocusBrief | null): RankedIssue[] {
  const byId = new Map(issues.map((issue) => [issue.id, issue]));
  const pins = pinnedRefs(brief);
  const relations = relationIndex(edges);
  const ranked = issues.filter((issue) => !closed(issue)).map((issue) => {
    const related = (relations.get(issue.id) ?? [])
      .map(({ id, kind }) => ({ issue: byId.get(id)!, kind }))
      .filter((item) => item.issue && !closed(item.issue))
      .sort((a, b) => RELATION_ORDER.indexOf(a.kind) - RELATION_ORDER.indexOf(b.kind) || a.issue.identifier.localeCompare(b.issue.identifier));
    const blockers = related.filter((item) => item.kind === "blocked_by");
    const unblocks = related.filter((item) => item.kind === "blocks");
    const pinIndex = pins.indexOf(issue.identifier.toUpperCase());
    const why = [
      pinIndex >= 0 ? "Pinned in focus brief" : null,
      issue.priority > 0 ? `${PRIORITY_LABEL[issue.priority]} priority` : null,
      unblocks.length ? `unblocks ${unblocks.slice(0, 3).map((item) => item.issue.identifier).join(", ")}${unblocks.length > 3 ? ` +${unblocks.length - 3}` : ""}` : null,
      issue.status.type === "started" ? "in progress" : null,
      issue.dueDate ? `due ${issue.dueDate}` : null,
      blockers.length ? `blocked by ${blockers.map((item) => item.issue.identifier).join(", ")}` : null,
    ].filter(Boolean).join("; ");
    const nextAction = blockers.length ? `Clear blocker ${blockers[0]!.issue.identifier} first`
      : issue.status.type === "started" ? `Continue ${issue.identifier}` : `Start work on ${issue.identifier}`;
    return {
      ranked: { issue, pinned: pinIndex >= 0, relations: related, whyNow: why || "Open in Linear", nextAction },
      key: [pinIndex >= 0 ? pinIndex : Number.MAX_SAFE_INTEGER, blockers.length ? 1 : 0, issue.priority === 0 ? 5 : issue.priority, -unblocks.length, STATUS_RANK[issue.status.type]] as number[],
    };
  });
  ranked.sort((a, b) => {
    for (let i = 0; i < a.key.length; i += 1) if (a.key[i] !== b.key[i]) return a.key[i]! - b.key[i]!;
    const due = (a.ranked.issue.dueDate ?? "9999").localeCompare(b.ranked.issue.dueDate ?? "9999");
    return due || b.ranked.issue.updatedAt.localeCompare(a.ranked.issue.updatedAt) || a.ranked.issue.id.localeCompare(b.ranked.issue.id);
  });
  return ranked.map((item) => item.ranked);
}

export function rankedRecommendations(issues: Issue[], edges: GraphEdge[], brief?: FocusBrief | null, limit = 50): FocusRecommendation[] {
  return rankIssues(issues, edges, brief).slice(0, limit).map((item, index) => ({
    issueId: item.issue.id, rank: index + 1, score: Math.max(1, 100 - index * 2),
    whyNow: item.whyNow.slice(0, 1000), nextAction: item.nextAction.slice(0, 500),
    evidenceIssueIds: [item.issue.id, ...item.relations.filter((rel) => rel.kind === "blocks" || rel.kind === "blocked_by").map((rel) => rel.issue.id)].slice(0, 20),
    confidence: 1,
  }));
}

/** Titles can carry customer data on EXP tickets; the office frame must never show emails or quote ids. */
export function shortDescription(title: string, max = 90): string {
  const clean = redactSensitiveText(title)
    .replace(/[\w.+-]+@[\w-]+(?:\.[\w-]+)+/g, "[email]")
    .replace(/\b[A-Z]{2,5}-(?=[A-Z0-9]*[A-Z])(?=[A-Z0-9]*\d)[A-Z0-9]{6,}\b/g, "[quote]")
    .replace(/\s+/g, " ").trim();
  return clean.length <= max ? clean : `${clean.slice(0, max - 1).trimEnd()}…`;
}

export function freshness(snapshot: GraphSnapshot | null, lastRun: AnalysisRun | null, now: Date) {
  const ageSeconds = snapshot ? Math.max(0, Math.round((now.getTime() - Date.parse(snapshot.syncedAt)) / 1000)) : null;
  return {
    syncedAt: snapshot?.syncedAt ?? null,
    generatedAt: snapshot?.generatedAt ?? null,
    ageSeconds,
    staleAfterHours: STALE_AFTER_HOURS,
    stale: ageSeconds === null || ageSeconds > STALE_AFTER_HOURS * 3600,
    lastRun: lastRun ? { status: lastRun.status, startedAt: lastRun.startedAt, completedAt: lastRun.completedAt, error: lastRun.error } : null,
  };
}

/** Compact feed for the office frame (FR frame-push). Field names are a cross-repo contract. */
export function nextUp(snapshot: GraphSnapshot, brief: FocusBrief | null, lastRun: AnalysisRun | null, now: Date, limit = 10) {
  const items = rankIssues(snapshot.nodes, snapshot.edges, brief).slice(0, limit).map((item) => ({
    ref: item.issue.identifier,
    short_description: shortDescription(item.issue.title),
    why_now: shortDescription(item.whyNow, 160),
    next_action: item.nextAction,
    priority: item.issue.priority,
    pinned: item.pinned,
    zone: item.issue.zone,
    relations: item.relations.slice(0, 6).map((rel) => ({ ref: rel.issue.identifier, kind: rel.kind, short_description: shortDescription(rel.issue.title, 60) })),
  }));
  return { ...freshness(snapshot, lastRun, now), items };
}
