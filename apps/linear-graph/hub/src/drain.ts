import { randomUUID } from "node:crypto";
import {
  campaignSchema, drainMetricsSchema, type Campaign, type CampaignRequest, type DrainMetrics, type GraphSnapshot,
  type TriageDecision, type WorkBundle, type BulkResolutionSet,
} from "@commonkit/linear-graph-protocol";

const CLOSED = new Set(["completed", "canceled"]);
const normalize = (value: string) => value.toLocaleLowerCase().trim();

function searchableIssue(issue: GraphSnapshot["nodes"][number]) {
  return normalize([
    issue.identifier, issue.title, issue.description ?? "", issue.team.key, issue.team.name,
    issue.project?.name ?? "", issue.repo ?? "", issue.zone, ...issue.topicTags, ...issue.labels,
  ].join(" "));
}

function relatedIds(snapshot: GraphSnapshot, seedIds: Set<string>) {
  const selected = new Set(seedIds);
  // Two hops keeps a thematic campaign connected while still allowing the full
  // synced universe to be searched when the prompt is broad.
  for (let hop = 0; hop < 2; hop += 1) {
    for (const edge of snapshot.edges) {
      if (selected.has(edge.sourceId)) selected.add(edge.targetId);
      if (selected.has(edge.targetId)) selected.add(edge.sourceId);
    }
  }
  return selected;
}

function issueMatches(issue: GraphSnapshot["nodes"][number], request: CampaignRequest) {
  const terms = normalize(request.prompt).split(/[^a-z0-9_-]+/).filter((term) => term.length >= 2);
  const haystack = searchableIssue(issue);
  const textMatch = terms.length === 0 || terms.some((term) => haystack.includes(term));
  const teamMatch = !request.teamKeys?.length || request.teamKeys.some((key) => normalize(key) === normalize(issue.team.key));
  const topicMatch = !request.topicIds?.length || request.topicIds.includes(issue.zone);
  const repoMatch = !request.repository || normalize(issue.repo ?? "") === normalize(request.repository);
  return textMatch && teamMatch && topicMatch && repoMatch;
}

function makeBundle(campaignId: string, issueIds: string[], snapshot: GraphSnapshot, order: number, now: string): WorkBundle {
  const issues = issueIds.map((issueId, index) => {
    const issue = snapshot.nodes.find((node) => node.id === issueId)!;
    return { issueId, order: index + 1, rationale: issue.parentId ? "Keeps parent/child work together." : undefined };
  });
  const topics = [...new Set(issueIds.map((id) => snapshot.nodes.find((node) => node.id === id)?.zone).filter(Boolean))];
  const title = topics.length === 1 ? `${topics[0]} work bundle ${order}` : `Cross-team work bundle ${order}`;
  const dependencyIssueIds = snapshot.edges
    .filter((edge) => issueIds.includes(edge.sourceId) && issueIds.includes(edge.targetId) && edge.kind === "blocks")
    .map((edge) => edge.targetId);
  return {
    id: `bundle:${randomUUID()}`, campaignId, title,
    summary: `Staged bundle of ${issueIds.length} related issues for one agent workstream.`,
    issueIds, issues, dependencyIssueIds: [...new Set(dependencyIssueIds)], expectedReduction: issueIds.length,
    status: "proposed", createdAt: now, updatedAt: now, approvedAt: null, approvalNote: null,
  };
}

function makeResolutionSet(campaignId: string, issueIds: string[], snapshot: GraphSnapshot, now: string): BulkResolutionSet | null {
  const duplicateIds = new Set<string>();
  for (const edge of snapshot.edges) {
    if (edge.kind !== "duplicate") continue;
    // Linear's relation direction is canonical -> duplicate. Only propose the
    // duplicate side for bulk resolution; never suggest closing the canonical.
    if (issueIds.includes(edge.sourceId)) duplicateIds.add(edge.targetId);
  }
  const valid = new Set(snapshot.nodes.filter((node) => !CLOSED.has(node.status.type)).map((node) => node.id));
  const candidates = [...duplicateIds].filter((id) => valid.has(id));
  if (!candidates.length) return null;
  return {
    id: `resolution:${randomUUID()}`, campaignId, issueIds: candidates, action: "merge",
    rationale: "These active issues are connected by a Linear duplicate relation; review the canonical issue before applying.",
    status: "proposed", createdAt: now, updatedAt: now, approvedAt: null,
  };
}

export function proposeCampaign(snapshot: GraphSnapshot, request: CampaignRequest, now = new Date(), decisions: TriageDecision[] = []): Campaign {
  const createdAt = now.toISOString();
  const active = snapshot.nodes.filter((node) => !CLOSED.has(node.status.type));
  const activeById = new Map(active.map((issue) => [issue.id, issue]));
  const decisionByIssue = new Map(decisions.map((decision) => [decision.issueId, decision]));
  const eligible = (issueId: string) => {
    if (!decisions.length) return true;
    const decision = decisionByIssue.get(issueId);
    return decision?.disposition === "ready" || decision?.disposition === "bundle_candidate";
  };
  const seedIds = new Set((request.seedIssueIds ?? []).filter((id) => activeById.has(id)));
  const matches = active.filter((issue) => eligible(issue.id) && issueMatches(issue, request));
  const selected = new Set(matches.map((issue) => issue.id));
  for (const id of relatedIds(snapshot, seedIds)) if (activeById.has(id) && eligible(id)) selected.add(id);
  // Explicit seeds are included only when triage says they are executable.
  for (const id of seedIds) if (eligible(id)) selected.add(id);
  const issueIds = active.filter((issue) => selected.has(issue.id)).sort((a, b) => a.priority - b.priority || b.updatedAt.localeCompare(a.updatedAt)).map((issue) => issue.id);
  const campaignId = `campaign:${randomUUID()}`;
  const bundles: WorkBundle[] = [];
  // Keep executable bundles in the 7–15 issue range whenever the campaign is
  // large enough; a small campaign remains one small bundle rather than being
  // padded with unrelated work.
  const bundleCount = issueIds.length <= 15 ? (issueIds.length ? 1 : 0) : Math.ceil(issueIds.length / 15);
  const bundleSize = bundleCount ? Math.ceil(issueIds.length / bundleCount) : 0;
  for (let offset = 0; offset < issueIds.length; offset += bundleSize) bundles.push(makeBundle(campaignId, issueIds.slice(offset, offset + bundleSize), snapshot, bundles.length + 1, createdAt));
  const resolutionSet = makeResolutionSet(campaignId, issueIds, snapshot, createdAt);
  return campaignSchema.parse({
    id: campaignId, title: request.title ?? `Drain: ${request.prompt.slice(0, 200)}`, prompt: request.prompt,
    status: "ready", issueIds, processedIssueCount: issueIds.length, totalIssueCount: issueIds.length,
    bundles, resolutionSet, snapshotAt: snapshot.syncedAt, createdAt, updatedAt: createdAt, error: null,
  });
}

export function computeDrainMetrics(snapshot: GraphSnapshot, decisions: TriageDecision[], campaigns: Campaign[], now = new Date()): DrainMetrics {
  const active = snapshot.nodes.filter((node) => !CLOSED.has(node.status.type));
  const decisionByIssue = new Map(decisions.map((decision) => [decision.issueId, decision]));
  const count = (disposition: TriageDecision["disposition"]) => active.filter((issue) => decisionByIssue.get(issue.id)?.disposition === disposition).length;
  const approvedBundles = campaigns.flatMap((campaign) => campaign.bundles).filter((bundle) => ["approved", "queued", "running", "blocked", "verified"].includes(bundle.status));
  const completed = snapshot.nodes.filter((node) => node.status.type === "completed").length;
  const canceled = snapshot.nodes.filter((node) => node.status.type === "canceled").length;
  const untriaged = active.filter((issue) => {
    const decision = decisionByIssue.get(issue.id);
    return !decision || (!decision.nextAction && decision.disposition !== "duplicate_stale");
  }).length;
  const uniqueApprovedIds = new Set(approvedBundles.flatMap((bundle) => bundle.issueIds));
  const approvedResolutionIds = new Set(campaigns.flatMap((campaign) => campaign.resolutionSet && ["approved", "applied"].includes(campaign.resolutionSet.status) ? campaign.resolutionSet.issueIds : []));
  const approvedOutcomeIds = new Set([...uniqueApprovedIds, ...approvedResolutionIds]);
  return drainMetricsSchema.parse({
    snapshotAt: snapshot.syncedAt, activeIssueCount: active.length, completedIssueCount: completed, canceledIssueCount: canceled,
    untriagedIssueCount: untriaged, readyIssueCount: count("ready"), blockedIssueCount: count("blocked"),
    needsClarificationIssueCount: count("needs_clarification"), duplicateStaleIssueCount: count("duplicate_stale"),
    bundleCandidateIssueCount: count("bundle_candidate"), approvedBundleIssueCount: uniqueApprovedIds.size,
    expectedReduction: uniqueApprovedIds.size,
    actualReduction: snapshot.nodes.filter((node) => approvedOutcomeIds.has(node.id) && ["completed", "canceled"].includes(node.status.type)).length,
    activeCampaignCount: campaigns.filter((campaign) => ["processing", "ready", "approved", "running"].includes(campaign.status)).length,
    proposedBundleCount: campaigns.flatMap((campaign) => campaign.bundles).filter((bundle) => bundle.status === "proposed").length,
    approvedBundleCount: approvedBundles.length, drained: untriaged === 0, computedAt: now.toISOString(),
  });
}
