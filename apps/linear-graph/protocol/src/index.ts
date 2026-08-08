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

export const focusBriefSchema = z.object({
  text: z.string().max(5000),
  updatedAt: date,
}).strict();
export type FocusBrief = z.infer<typeof focusBriefSchema>;

export const updateTopicRequestSchema = z.object({ zone: zoneIdSchema }).strict();
export const updateFocusBriefRequestSchema = z.object({ text: z.string().max(5000) }).strict();

export const DEFAULT_ZONES: readonly TopicZone[] = [
  { id: "product", label: "Product", description: "User-facing product behavior and outcomes", color: "#4f8cff" },
  { id: "platform", label: "Platform", description: "Shared infrastructure and agent capabilities", color: "#8b5cf6" },
  { id: "operations", label: "Operations", description: "Delivery, hosting, observability, and support", color: "#f59e0b" },
  { id: "security", label: "Security", description: "Credentials, trust boundaries, and safety", color: "#ef4444" },
  { id: "documentation", label: "Documentation", description: "Guides, context, and knowledge management", color: "#22c55e" },
  { id: "unsorted", label: "Unsorted", description: "Issues awaiting a confident topic assignment", color: "#64748b" },
];
