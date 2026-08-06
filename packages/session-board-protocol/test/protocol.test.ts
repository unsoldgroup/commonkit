import { describe, expect, test } from "bun:test";

import {
  boardSnapshotSchema,
  claudeHookResponseSchema,
  decisionRecordSchema,
  decisionValidForAction,
  decisionSchema,
  decisionsResponseSchema,
  decisionRequestSchema,
  hubToReporterMessageSchema,
  reporterToHubMessageSchema,
  sseEventSchema,
} from "../src/index.js";

const machine = {
  id: "mac-studio",
  name: "Studio Loadout",
  lastSeenAt: "2026-07-21T12:00:00.000Z",
  online: true,
} as const;

const session = {
  machineId: machine.id,
  worktreeId: "commonkit::/worktrees/uns-1301",
  repo: "commonkit",
  project: "Session Board",
  agent: "claude",
  paneKey: "pane-1",
  state: "waiting",
  stateSince: "2026-07-21T12:00:01.000Z",
  title: "Define protocol",
  lastOutputAt: "2026-07-21T12:00:02.000Z",
} as const;

const action = {
  id: "action-1",
  kind: "claude-permission",
  sessionRef: {
    machineId: session.machineId,
    worktreeId: session.worktreeId,
    paneKey: session.paneKey,
  },
  summary: "Allow Bash",
  detail: { tool: "Bash", input: { command: "pnpm test" } },
  createdAt: "2026-07-21T12:00:03.000Z",
  expiresAt: "2026-07-21T12:05:03.000Z",
} as const;

const codexAction = {
  ...action,
  id: "action-2",
  kind: "codex-prompt",
  summary: "Choose a retry",
  detail: { prompt: "How should Codex continue?", options: ["Retry", "Stop"] },
} as const;

const gateAction = {
  ...action,
  id: "action-3",
  kind: "orca-gate",
  summary: "Select a Target",
  detail: { question: "Which Target should receive the Loadout?", options: ["Studio", "Laptop"] },
} as const;

const decision = {
  actionId: action.id,
  verdict: "allow",
  decidedAt: "2026-07-21T12:00:04.000Z",
} as const;

function expectRoundTrip<T>(schema: { parse(value: unknown): T }, fixture: T) {
  expect(schema.parse(JSON.parse(JSON.stringify(fixture)))).toEqual(fixture);
}

describe("session board protocol fixtures", () => {
  test("round-trips reporter-to-hub messages", () => {
    const fixtures = [
      { type: "hello", machineToken: "machine-secret" },
      { type: "stateSnapshot", machine, sessions: [session], pendingActions: [action] },
      {
        type: "stateDelta",
        machine,
        sessions: { upsert: [session], remove: [] },
        pendingActions: { upsert: [action], remove: [] },
      },
      { type: "actionOpened", action },
      { type: "actionOpened", action: codexAction },
      { type: "actionOpened", action: gateAction },
      { type: "actionClosed", actionId: action.id, outcome: "allowed" },
      { type: "tailResponse", requestId: "tail-1", lines: ["one", "two"] },
    ] as const;

    for (const fixture of fixtures) expectRoundTrip(reporterToHubMessageSchema, fixture);
  });

  test("round-trips hub decisions", () => {
    expectRoundTrip(hubToReporterMessageSchema, { type: "decision", decision });
    expectRoundTrip(hubToReporterMessageSchema, {
      type: "tailRequest",
      requestId: "tail-1",
      sessionRef: action.sessionRef,
    });
  });

  test("round-trips REST and SSE payloads", () => {
    const snapshot = { machines: [machine], sessions: [session], pendingActions: [action] };
    expectRoundTrip(boardSnapshotSchema, snapshot);
    expectRoundTrip(decisionRequestSchema, { verdict: "allow" });
    expectRoundTrip(decisionRequestSchema, { verdict: "option:2" });
    expectRoundTrip(decisionRequestSchema, { verdict: "always", steer: "Keep this project-local." });
    expectRoundTrip(decisionSchema, { ...decision, verdict: "deny", steer: "Use the read-only command." });
    expectRoundTrip(sseEventSchema, { type: "snapshot", data: snapshot });
    expectRoundTrip(sseEventSchema, { type: "actionClosed", data: { actionId: action.id, outcome: "allowed" } });
  });

  test("rejects invalid action details and option verdicts", () => {
    expect(() =>
      reporterToHubMessageSchema.parse({
        type: "actionOpened",
        action: { ...action, detail: { prompt: "Wrong detail", options: [] } },
      }),
    ).toThrow();
    expect(() => decisionRequestSchema.parse({ verdict: "option:0" })).toThrow();
    expect(() => decisionRequestSchema.parse({ verdict: "deny", steer: "x".repeat(281) })).toThrow();
  });

  test("round-trips a Claude permission rule suggestion", () => {
    expectRoundTrip(reporterToHubMessageSchema, {
      type: "actionOpened",
      action: {
        ...action,
        detail: { ...action.detail, ruleSuggestion: "Bash(pnpm test:*)" },
      },
    });
  });

  test("restricts always and steer to Claude permission actions", () => {
    expect(decisionValidForAction(action, { verdict: "always" })).toBe(true);
    expect(decisionValidForAction(action, { verdict: "deny", steer: "Use a safer command." })).toBe(true);
    expect(decisionValidForAction(codexAction, { verdict: "always" })).toBe(false);
    expect(decisionValidForAction(codexAction, { verdict: "option:1", steer: "Retry later." })).toBe(false);
    expect(decisionValidForAction(gateAction, { verdict: "always" })).toBe(false);
    expect(decisionValidForAction(gateAction, { verdict: "option:1" })).toBe(true);
  });

  test("round-trips Claude hook permission updates", () => {
    expectRoundTrip(claudeHookResponseSchema, {
      decision: "allow",
      updatedPermissions: [{ rule: "Bash(git push:*)", destination: "localSettings" }],
    });
    expectRoundTrip(claudeHookResponseSchema, { decision: "deny" });
    expect(() =>
      claudeHookResponseSchema.parse({
        decision: "allow",
        updatedPermissions: [{ rule: "Bash(git push:*)", destination: "userSettings" }],
      }),
    ).toThrow();
  });

  test("round-trips decision log records and REST payloads", () => {
    const record = {
      id: "decision-1",
      actionId: action.id,
      summary: action.summary,
      verdict: "deny",
      steer: "Use a read-only command.",
      machineId: machine.id,
      decidedAt: decision.decidedAt,
    } as const;

    expectRoundTrip(decisionRecordSchema, record);
    expectRoundTrip(decisionsResponseSchema, { records: [record] });
  });
});
