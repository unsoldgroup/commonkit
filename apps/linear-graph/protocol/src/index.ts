import { z } from "zod";

const id = z.string().min(1);
const date = z.string().datetime({ offset: true });
const nullableDate = date.nullable();

export const zoneIdSchema = z.string().min(1).max(80).regex(/^[a-z0-9][a-z0-9_-]*$/);
export type ZoneId = z.infer<typeof zoneIdSchema>;

export const issueStatusSchema = z.object({
  id,
  name: z.string().min(1),
  type: z.enum(["backlog", "unstarted", "started", "completed", "canceled"]),
}).strict();
export type IssueStatus = z.infer<typeof issueStatusSchema>;

export const issueTeamSchema = z.object({ id, key: z.string().min(1), name: z.string().min(1) }).strict();
export type IssueTeam = z.infer<typeof issueTeamSchema>;

export const teamSummarySchema = z.object({
  id,
  key: z.string().min(1),
  name: z.string().min(1),
  issueCount: z.number().int().nonnegative(),
  activeIssueCount: z.number().int().nonnegative(),
  completedIssueCount: z.number().int().nonnegative(),
  canceledIssueCount: z.number().int().nonnegative(),
}).strict();
export type TeamSummary = z.infer<typeof teamSummarySchema>;

export const issueProjectSchema = z.object({ id, name: z.string().min(1), url: z.string().url().optional() }).strict();
export type IssueProject = z.infer<typeof issueProjectSchema>;

export const issueSchema = z.object({
  id,
  identifier: z.string().min(1),
  title: z.string().min(1),
  description: z.string().nullable(),
  url: z.string().url(),
  team: issueTeamSchema,
  project: issueProjectSchema.nullable(),
  status: issueStatusSchema,
  priority: z.number().int().min(0).max(4),
  labels: z.array(z.string().min(1)),
  cycle: z.object({ id, name: z.string().min(1), number: z.number().int().optional() }).strict().nullable(),
  parentId: id.nullable(),
  dueDate: z.string().date().nullable(),
  createdAt: date,
  updatedAt: date,
  completedAt: nullableDate,
  repo: z.string().min(1).nullable(),
  zone: zoneIdSchema,
  topicTags: z.array(z.string().min(1)).max(12),
}).strict();
export type Issue = z.infer<typeof issueSchema>;

export const edgeKindSchema = z.enum(["parent", "blocks", "related", "duplicate", "semantic"]);
export type EdgeKind = z.infer<typeof edgeKindSchema>;
export const edgeSourceSchema = z.enum(["linear", "codex"]);
export type EdgeSource = z.infer<typeof edgeSourceSchema>;
export const graphEdgeSchema = z.object({
  id,
  sourceId: id,
  targetId: id,
  kind: edgeKindSchema,
  source: edgeSourceSchema,
  confidence: z.number().min(0).max(1).optional(),
  rationale: z.string().max(500).optional(),
}).strict();
export type GraphEdge = z.infer<typeof graphEdgeSchema>;

export const topicZoneSchema = z.object({
  id: zoneIdSchema,
  label: z.string().min(1),
  description: z.string().min(1),
  color: z.string().regex(/^#[0-9a-f]{6}$/i),
}).strict();
export type TopicZone = z.infer<typeof topicZoneSchema>;

export const graphNodeSchema = issueSchema;
export type GraphNode = Issue;

export const focusRecommendationSchema = z.object({
  issueId: id,
  rank: z.number().int().positive(),
  score: z.number().min(0).max(100),
  whyNow: z.string().min(1).max(1000),
  nextAction: z.string().min(1).max(500),
  evidenceIssueIds: z.array(id).max(20),
  confidence: z.number().min(0).max(1),
}).strict();
export type FocusRecommendation = z.infer<typeof focusRecommendationSchema>;

export const graphSnapshotSchema = z.object({
  generatedAt: date,
  syncedAt: date,
  stale: z.boolean(),
  nodes: z.array(graphNodeSchema),
  edges: z.array(graphEdgeSchema),
  teams: z.array(teamSummarySchema),
  zones: z.array(topicZoneSchema),
  recommendations: z.array(focusRecommendationSchema),
  latestAnalysisRunId: id.nullable(),
}).strict();
export type GraphSnapshot = z.infer<typeof graphSnapshotSchema>;

export const topicAssignmentSchema = z.object({
  issueId: id,
  zone: zoneIdSchema,
  topicTags: z.array(z.string().min(1)).max(12),
  confidence: z.number().min(0).max(1),
  rationale: z.string().max(500),
}).strict();
export type TopicAssignment = z.infer<typeof topicAssignmentSchema>;

export const semanticEdgeSuggestionSchema = z.object({
  sourceId: id,
  targetId: id,
  confidence: z.number().min(0).max(1),
  rationale: z.string().min(1).max(500),
}).strict();
export type SemanticEdgeSuggestion = z.infer<typeof semanticEdgeSuggestionSchema>;

export const codexAnalysisSchema = z.object({
  assignments: z.array(topicAssignmentSchema),
  semanticEdges: z.array(semanticEdgeSuggestionSchema).max(1000),
  recommendations: z.array(focusRecommendationSchema).max(50),
}).strict();
export type CodexAnalysis = z.infer<typeof codexAnalysisSchema>;

export const analysisRunSchema = z.object({
  id,
  startedAt: date,
  completedAt: date.nullable(),
  status: z.enum(["running", "completed", "failed"]),
  issueCount: z.number().int().nonnegative(),
  error: z.string().max(1000).nullable(),
  inputHash: z.string().regex(/^[a-f0-9]{64}$/).nullable(),
}).strict();
export type AnalysisRun = z.infer<typeof analysisRunSchema>;

export const briefStatusSchema = z.enum(["ready", "running", "failed", "stale"]);
export type BriefStatus = z.infer<typeof briefStatusSchema>;
export const briefSourceSchema = z.enum(["codex", "manual", "fallback"]);
export type BriefSource = z.infer<typeof briefSourceSchema>;
export const focusBriefSchema = z.object({
  text: z.string().max(5000),
  updatedAt: date,
  status: briefStatusSchema.optional(),
  source: briefSourceSchema.optional(),
  generatedAt: date.optional(),
  snapshotAt: date.optional(),
  error: z.string().max(1000).nullable().optional(),
}).strict();
export type FocusBrief = z.infer<typeof focusBriefSchema>;

export const updateTopicRequestSchema = z.object({ zone: zoneIdSchema }).strict();
export const updateFocusBriefRequestSchema = z.object({ text: z.string().max(5000) }).strict();

export const codexAuthModeSchema = z.enum(["subscription", "api-key"]);
export type CodexAuthMode = z.infer<typeof codexAuthModeSchema>;
export const codexAuthStatusSchema = z.object({
  mode: codexAuthModeSchema,
  status: z.enum(["authenticated", "not_authenticated", "starting", "configured", "missing", "unknown", "failed"]),
  checkedAt: date,
  account: z.string().max(200).nullable(),
  detail: z.string().max(1200).nullable(),
  loginUrl: z.string().url().nullable().optional(),
  deviceCode: z.string().max(32).nullable().optional(),
}).strict();
export type CodexAuthStatus = z.infer<typeof codexAuthStatusSchema>;

export const triageDispositionSchema = z.enum(["ready", "blocked", "needs_clarification", "duplicate_stale", "bundle_candidate"]);
export type TriageDisposition = z.infer<typeof triageDispositionSchema>;
export const triageSourceSchema = z.enum(["codex", "manual"]);
export type TriageSource = z.infer<typeof triageSourceSchema>;
export const triageDecisionSchema = z.object({
  issueId: id,
  disposition: triageDispositionSchema,
  rationale: z.string().min(1).max(2000),
  confidence: z.number().min(0).max(1),
  evidenceIssueIds: z.array(id).max(50),
  nextAction: z.string().max(500).nullable(),
  source: triageSourceSchema,
  createdAt: date,
  updatedAt: date,
}).strict();
export type TriageDecision = z.infer<typeof triageDecisionSchema>;

export const workBundleStatusSchema = z.enum(["proposed", "approved", "rejected", "queued", "running", "blocked", "verified", "failed"]);
export type WorkBundleStatus = z.infer<typeof workBundleStatusSchema>;
export const workBundleIssueSchema = z.object({
  issueId: id,
  order: z.number().int().positive(),
  rationale: z.string().max(1000).optional(),
}).strict();
export type WorkBundleIssue = z.infer<typeof workBundleIssueSchema>;
export const workBundleSchema = z.object({
  id,
  campaignId: id,
  title: z.string().min(1).max(240),
  summary: z.string().min(1).max(2000),
  issueIds: z.array(id).min(1),
  issues: z.array(workBundleIssueSchema).min(1),
  dependencyIssueIds: z.array(id).max(100),
  expectedReduction: z.number().int().nonnegative(),
  status: workBundleStatusSchema,
  createdAt: date,
  updatedAt: date,
  approvedAt: date.nullable(),
  approvalNote: z.string().max(1000).nullable(),
  linearProjectId: id.nullable().optional(),
  linearProjectUrl: z.string().url().nullable().optional(),
  linearSyncError: z.string().max(1000).nullable().optional(),
}).strict();
export type WorkBundle = z.infer<typeof workBundleSchema>;

export const bulkResolutionStatusSchema = z.enum(["proposed", "approved", "rejected", "applied", "failed"]);
export type BulkResolutionStatus = z.infer<typeof bulkResolutionStatusSchema>;
export const bulkResolutionActionSchema = z.enum(["close", "cancel", "merge"]);
export type BulkResolutionAction = z.infer<typeof bulkResolutionActionSchema>;
export const bulkResolutionSetSchema = z.object({
  id,
  campaignId: id,
  issueIds: z.array(id),
  action: bulkResolutionActionSchema,
  rationale: z.string().min(1).max(2000),
  status: bulkResolutionStatusSchema,
  createdAt: date,
  updatedAt: date,
  approvedAt: date.nullable(),
}).strict();
export type BulkResolutionSet = z.infer<typeof bulkResolutionSetSchema>;

export const campaignStatusSchema = z.enum(["proposed", "processing", "ready", "approved", "running", "completed", "failed"]);
export type CampaignStatus = z.infer<typeof campaignStatusSchema>;
export const campaignSchema = z.object({
  id,
  title: z.string().min(1).max(240),
  prompt: z.string().min(1).max(1000),
  status: campaignStatusSchema,
  issueIds: z.array(id),
  processedIssueCount: z.number().int().nonnegative(),
  totalIssueCount: z.number().int().nonnegative(),
  bundles: z.array(workBundleSchema),
  resolutionSet: bulkResolutionSetSchema.nullable(),
  snapshotAt: date,
  createdAt: date,
  updatedAt: date,
  error: z.string().max(1000).nullable(),
}).strict();
export type Campaign = z.infer<typeof campaignSchema>;

export const campaignRequestSchema = z.object({
  title: z.string().min(1).max(240).optional(),
  prompt: z.string().min(1).max(1000),
  seedIssueIds: z.array(id).optional(),
  teamKeys: z.array(z.string().min(1).max(40)).optional(),
  topicIds: z.array(zoneIdSchema).optional(),
  repository: z.string().max(200).optional(),
}).strict();
export type CampaignRequest = z.infer<typeof campaignRequestSchema>;

export const bundleApprovalRequestSchema = z.object({
  decision: z.enum(["approve", "reject"]),
  note: z.string().max(1000).optional(),
}).strict();
export type BundleApprovalRequest = z.infer<typeof bundleApprovalRequestSchema>;

export const triageDecisionRequestSchema = z.object({
  issueId: id,
  disposition: triageDispositionSchema,
  rationale: z.string().min(1).max(2000),
  confidence: z.number().min(0).max(1),
  evidenceIssueIds: z.array(id).max(50).default([]),
  nextAction: z.string().max(500).nullable().default(null),
  source: triageSourceSchema.default("manual"),
}).strict();
export type TriageDecisionRequest = z.infer<typeof triageDecisionRequestSchema>;

export const drainMetricsSchema = z.object({
  snapshotAt: date,
  activeIssueCount: z.number().int().nonnegative(),
  completedIssueCount: z.number().int().nonnegative(),
  canceledIssueCount: z.number().int().nonnegative(),
  untriagedIssueCount: z.number().int().nonnegative(),
  readyIssueCount: z.number().int().nonnegative(),
  blockedIssueCount: z.number().int().nonnegative(),
  needsClarificationIssueCount: z.number().int().nonnegative(),
  duplicateStaleIssueCount: z.number().int().nonnegative(),
  bundleCandidateIssueCount: z.number().int().nonnegative(),
  approvedBundleIssueCount: z.number().int().nonnegative(),
  expectedReduction: z.number().int().nonnegative(),
  actualReduction: z.number().int().nonnegative(),
  activeCampaignCount: z.number().int().nonnegative(),
  proposedBundleCount: z.number().int().nonnegative(),
  approvedBundleCount: z.number().int().nonnegative(),
  drained: z.boolean(),
  computedAt: date,
}).strict();
export type DrainMetrics = z.infer<typeof drainMetricsSchema>;

export const drainStateSchema = z.object({
  metrics: drainMetricsSchema,
  campaigns: z.array(campaignSchema),
  decisions: z.array(triageDecisionSchema),
}).strict();
export type DrainState = z.infer<typeof drainStateSchema>;

export const executionRunStatusSchema = z.enum(["queued", "running", "completed", "failed", "timed_out"]);
export type ExecutionRunStatus = z.infer<typeof executionRunStatusSchema>;
export const executionRunSchema = z.object({
  id,
  bundleId: id,
  repository: z.string().min(1).max(200),
  branch: z.string().max(200).nullable(),
  worktreePath: z.string().max(1000).nullable(),
  status: executionRunStatusSchema,
  startedAt: date,
  completedAt: date.nullable(),
  exitCode: z.number().int().nullable(),
  stdout: z.string().max(100000).optional(),
  stderr: z.string().max(100000).optional(),
  evidence: z.array(z.object({ kind: z.string().min(1).max(80), text: z.string().max(100000) }).strict()).max(50),
  error: z.string().max(2000).nullable(),
  createdAt: date,
}).strict();
export type ExecutionRun = z.infer<typeof executionRunSchema>;

export const executionRequestSchema = z.object({
  repository: z.string().min(1).max(200),
  instruction: z.string().max(2000).optional(),
}).strict();
export type ExecutionRequest = z.infer<typeof executionRequestSchema>;

export const DEFAULT_ZONES: readonly TopicZone[] = [
  { id: "product", label: "Product", description: "User-facing product behavior and outcomes", color: "#4f8cff" },
  { id: "platform", label: "Platform", description: "Shared infrastructure and agent capabilities", color: "#8b5cf6" },
  { id: "operations", label: "Operations", description: "Delivery, hosting, observability, and support", color: "#f59e0b" },
  { id: "security", label: "Security", description: "Credentials, trust boundaries, and safety", color: "#ef4444" },
  { id: "documentation", label: "Documentation", description: "Guides, context, and knowledge management", color: "#22c55e" },
  { id: "unsorted", label: "Unsorted", description: "Issues awaiting a confident topic assignment", color: "#64748b" },
];
