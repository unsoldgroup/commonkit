import { describe, expect, test } from "bun:test";
import type { BoardSnapshot, DecisionRecord, Session } from "@commonkit/session-board-protocol";
import { actionPayload, decisionsNewestFirst, groupBoard, moveSession, outcomeText, pendingOldestFirst, sessionId } from "../src/model.js";

const base: Session = { machineId: "studio", worktreeId: "one", paneKey: "p1", repo: "commonkit", project: "Board", agent: "claude", state: "idle", stateSince: "2026-07-21T12:00:00.000Z", title: "Idle", lastOutputAt: "2026-07-21T12:00:00.000Z" };
const action = (id: string, session: Session, createdAt: string) => ({ id, kind: "claude-permission" as const, sessionRef: { machineId: session.machineId, worktreeId: session.worktreeId, paneKey: session.paneKey }, summary: "Allow Bash", detail: { tool: "Bash", input: {} }, createdAt, expiresAt: "2026-07-21T13:00:00.000Z" });

describe("board view model", () => {
  test("defaults to project groups and sorts action, working, then rest", () => {
    const waiting = { ...base, worktreeId: "waiting", paneKey: "p2", state: "waiting" as const };
    const working = { ...base, worktreeId: "working", paneKey: "p3", state: "working" as const };
    const snapshot: BoardSnapshot = { machines: [], sessions: [base, working, waiting], pendingActions: [action("a", waiting, "2026-07-21T12:01:00.000Z")] };
    expect(groupBoard(snapshot, { groups: [] })[0]?.sessions.map((item) => item.worktreeId)).toEqual(["waiting", "working", "one"]);
  });

  test("lists pending actions oldest-first", () => {
    const snapshot: BoardSnapshot = { machines: [], sessions: [base], pendingActions: [action("new", base, "2026-07-21T12:02:00.000Z"), action("old", base, "2026-07-21T12:01:00.000Z")] };
    expect(pendingOldestFirst(snapshot).map((item) => item.id)).toEqual(["old", "new"]);
  });

  test("moves a session into a user group", () => {
    const groups = groupBoard({ machines: [], sessions: [base], pendingActions: [] }, { groups: [{ id: "focus", name: "Focus", sessionIds: [] }] });
    expect(moveSession({ groups: [] }, groups, sessionId(base), "focus").groups.find((group) => group.id === "focus")?.sessionIds).toEqual([sessionId(base)]);
  });

  test("renders the full Claude payload without truncation", () => {
    const content = "x".repeat(700);
    expect(actionPayload(action("bash", base, "2026-07-21T12:01:00.000Z"))).toBeUndefined();
    expect(actionPayload({
      ...action("write", base, "2026-07-21T12:01:00.000Z"),
      detail: { tool: "Write", input: { content } },
    })).toBe(content);
    expect(actionPayload({
      ...action("edit", base, "2026-07-21T12:01:00.000Z"),
      detail: { tool: "Edit", input: { old_string: "before", new_string: "after" } },
    })).toBe("- before\n+ after");
  });

  test("sorts decision records newest-first", () => {
    const record = (id: string, decidedAt: string): DecisionRecord => ({
      id, actionId: `action-${id}`, summary: id, verdict: "allow", machineId: "studio", decidedAt,
    });
    expect(decisionsNewestFirst([
      record("old", "2026-07-21T12:01:00.000Z"),
      record("new", "2026-07-21T12:02:00.000Z"),
    ]).map((item) => item.id)).toEqual(["new", "old"]);
  });

  test("maps closed outcomes to board copy", () => {
    expect(outcomeText("allowed")).toBe("Allowed");
    expect(outcomeText("denied")).toBe("Denied");
    expect(outcomeText("stale")).toBe("Expired · answered in terminal");
    expect(outcomeText("failed")).toBe("Expired · answered in terminal");
  });
});
