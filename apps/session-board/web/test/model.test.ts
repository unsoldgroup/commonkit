import { describe, expect, test } from "bun:test";
import type { BoardSnapshot, Session } from "@commonkit/session-board-protocol";
import { groupBoard, moveSession, pendingOldestFirst, sessionId } from "../src/model.js";

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
});
