import { z } from "zod";

const idSchema = z.string().min(1);
const timestampSchema = z.string().datetime({ offset: true });

export const agentSchema = z.enum(["claude", "codex", "other"]);
export type Agent = z.infer<typeof agentSchema>;

export const sessionStateSchema = z.enum([
  "working",
  "waiting",
  "blocked",
  "done",
  "idle",
  "interrupted",
]);
export type SessionState = z.infer<typeof sessionStateSchema>;

export const machineSchema = z
  .object({
    id: idSchema,
    name: z.string().min(1),
    lastSeenAt: timestampSchema,
    online: z.boolean(),
  })
  .strict();
export type Machine = z.infer<typeof machineSchema>;

export const sessionRefSchema = z
  .object({
    machineId: idSchema,
    worktreeId: idSchema,
    paneKey: idSchema,
  })
  .strict();
export type SessionRef = z.infer<typeof sessionRefSchema>;

export const sessionSchema = z
  .object({
    machineId: idSchema,
    worktreeId: idSchema,
    repo: z.string().min(1),
    project: z.string().min(1),
    agent: agentSchema,
    paneKey: idSchema,
    state: sessionStateSchema,
    stateSince: timestampSchema,
    title: z.string(),
    lastOutputAt: timestampSchema,
  })
  .strict();
export type Session = z.infer<typeof sessionSchema>;

const pendingActionBase = {
  id: idSchema,
  sessionRef: sessionRefSchema,
  summary: z.string().min(1),
  createdAt: timestampSchema,
  expiresAt: timestampSchema,
} as const;

export const claudePermissionActionSchema = z
  .object({
    ...pendingActionBase,
    kind: z.literal("claude-permission"),
    detail: z
      .object({
        tool: z.string().min(1),
        input: z.unknown(),
        intent: z.string().optional(),
      })
      .strict(),
  })
  .strict();
export type ClaudePermissionAction = z.infer<typeof claudePermissionActionSchema>;

export const claudeHookRequestSchema = z.object({
  tool: z.string().min(1),
  input: z.unknown(),
  session: z.object({
    id: idSchema,
    cwd: z.string().min(1),
    transcriptPath: z.string().optional(),
  }).strict(),
}).strict();
export type ClaudeHookRequest = z.infer<typeof claudeHookRequestSchema>;

export const claudeHookResponseSchema = z.object({ decision: z.enum(["allow", "deny"]) }).strict();
export type ClaudeHookResponse = z.infer<typeof claudeHookResponseSchema>;

export const codexPromptActionSchema = z
  .object({
    ...pendingActionBase,
    kind: z.literal("codex-prompt"),
    detail: z
      .object({
        prompt: z.string().min(1),
        options: z.array(z.string().min(1)).min(1),
      })
      .strict(),
  })
  .strict();
export type CodexPromptAction = z.infer<typeof codexPromptActionSchema>;

export const orcaGateActionSchema = z
  .object({
    ...pendingActionBase,
    kind: z.literal("orca-gate"),
    detail: z
      .object({
        question: z.string().min(1),
        options: z.array(z.string().min(1)).min(1),
      })
      .strict(),
  })
  .strict();
export type OrcaGateAction = z.infer<typeof orcaGateActionSchema>;

export const pendingActionSchema = z.discriminatedUnion("kind", [
  claudePermissionActionSchema,
  codexPromptActionSchema,
  orcaGateActionSchema,
]);
export type PendingAction = z.infer<typeof pendingActionSchema>;

export const verdictSchema = z.union([
  z.enum(["allow", "deny"]),
  z.string().regex(/^option:[1-9]\d*$/),
]);
export type Verdict = z.infer<typeof verdictSchema>;

export const decisionSchema = z
  .object({
    actionId: idSchema,
    verdict: verdictSchema,
    decidedAt: timestampSchema,
  })
  .strict();
export type Decision = z.infer<typeof decisionSchema>;

export const boardSnapshotSchema = z
  .object({
    machines: z.array(machineSchema),
    sessions: z.array(sessionSchema),
    pendingActions: z.array(pendingActionSchema),
  })
  .strict();
export type BoardSnapshot = z.infer<typeof boardSnapshotSchema>;

const sessionChangesSchema = z
  .object({
    upsert: z.array(sessionSchema),
    remove: z.array(sessionRefSchema),
  })
  .strict();
export type SessionChanges = z.infer<typeof sessionChangesSchema>;

const pendingActionChangesSchema = z
  .object({
    upsert: z.array(pendingActionSchema),
    remove: z.array(idSchema),
  })
  .strict();
export type PendingActionChanges = z.infer<typeof pendingActionChangesSchema>;

export const helloMessageSchema = z
  .object({ type: z.literal("hello"), machineToken: z.string().min(1) })
  .strict();
export type HelloMessage = z.infer<typeof helloMessageSchema>;

export const stateSnapshotMessageSchema = z
  .object({
    type: z.literal("stateSnapshot"),
    machine: machineSchema,
    sessions: z.array(sessionSchema),
    pendingActions: z.array(pendingActionSchema),
  })
  .strict();
export type StateSnapshotMessage = z.infer<typeof stateSnapshotMessageSchema>;

export const stateDeltaMessageSchema = z
  .object({
    type: z.literal("stateDelta"),
    machine: machineSchema,
    sessions: sessionChangesSchema,
    pendingActions: pendingActionChangesSchema,
  })
  .strict();
export type StateDeltaMessage = z.infer<typeof stateDeltaMessageSchema>;

export const actionOpenedMessageSchema = z
  .object({ type: z.literal("actionOpened"), action: pendingActionSchema })
  .strict();
export type ActionOpenedMessage = z.infer<typeof actionOpenedMessageSchema>;

export const actionClosedMessageSchema = z
  .object({
    type: z.literal("actionClosed"),
    actionId: idSchema,
    outcome: z.enum(["allowed", "denied", "stale", "failed"]).optional(),
  })
  .strict();
export type ActionClosedMessage = z.infer<typeof actionClosedMessageSchema>;

export const tailResponseMessageSchema = z
  .object({ type: z.literal("tailResponse"), requestId: idSchema, lines: z.array(z.string()) })
  .strict();
export type TailResponseMessage = z.infer<typeof tailResponseMessageSchema>;

export const reporterToHubMessageSchema = z.discriminatedUnion("type", [
  helloMessageSchema,
  stateSnapshotMessageSchema,
  stateDeltaMessageSchema,
  actionOpenedMessageSchema,
  actionClosedMessageSchema,
  tailResponseMessageSchema,
]);
export type ReporterToHubMessage = z.infer<typeof reporterToHubMessageSchema>;

export const decisionMessageSchema = z
  .object({ type: z.literal("decision"), decision: decisionSchema })
  .strict();
export type DecisionMessage = z.infer<typeof decisionMessageSchema>;

export const tailRequestMessageSchema = z
  .object({ type: z.literal("tailRequest"), requestId: idSchema, sessionRef: sessionRefSchema })
  .strict();
export type TailRequestMessage = z.infer<typeof tailRequestMessageSchema>;

export const hubToReporterMessageSchema = z.discriminatedUnion("type", [decisionMessageSchema, tailRequestMessageSchema]);
export type HubToReporterMessage = z.infer<typeof hubToReporterMessageSchema>;

export const decisionRequestSchema = z.object({ verdict: verdictSchema }).strict();
export type DecisionRequest = z.infer<typeof decisionRequestSchema>;

export const decisionResponseSchema = z.object({ decision: decisionSchema }).strict();
export type DecisionResponse = z.infer<typeof decisionResponseSchema>;

export const sseEventSchema = z.discriminatedUnion("type", [
  z.object({ type: z.literal("snapshot"), data: boardSnapshotSchema }).strict(),
  z.object({ type: z.literal("machine"), data: machineSchema }).strict(),
  z.object({ type: z.literal("session"), data: sessionSchema }).strict(),
  z.object({ type: z.literal("actionOpened"), data: pendingActionSchema }).strict(),
  z.object({
    type: z.literal("actionClosed"),
    data: z.object({
      actionId: idSchema,
      outcome: z.enum(["allowed", "denied", "stale", "failed"]).optional(),
    }).strict(),
  }).strict(),
  z.object({ type: z.literal("decision"), data: decisionSchema }).strict(),
]);
export type SseEvent = z.infer<typeof sseEventSchema>;
