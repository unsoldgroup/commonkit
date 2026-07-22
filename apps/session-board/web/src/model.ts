import type { BoardSnapshot, PendingAction, Session } from "@commonkit/session-board-protocol";

export type LayoutGroup = { id: string; name: string; sessionIds: string[] };
export type BoardGroup = LayoutGroup & { sessions: Session[] };
export type Layout = { groups: LayoutGroup[] };

export const sessionId = (session: Pick<Session, "machineId" | "worktreeId" | "paneKey">) =>
  JSON.stringify([session.machineId, session.worktreeId, session.paneKey]);

const rank = (session: Session, actions: PendingAction[]) => {
  if (actions.some((action) => sessionId(action.sessionRef) === sessionId(session))) return 0;
  if (session.state === "working") return 1;
  return 2;
};

export function pendingFor(session: Session, actions: PendingAction[]) {
  return actions.filter((action) => sessionId(action.sessionRef) === sessionId(session));
}

export function groupBoard(snapshot: BoardSnapshot, layout: Layout): BoardGroup[] {
  const assigned = new Set(layout.groups.flatMap((group) => group.sessionIds));
  const groups: BoardGroup[] = layout.groups.map((group) => ({ ...group, sessions: [] }));
  const byId = new Map(groups.map((group) => [group.id, group]));
  for (const session of snapshot.sessions) {
    const id = sessionId(session);
    const custom = groups.find((group) => group.sessionIds.includes(id));
    const projectId = `project:${session.project}`;
    const group = custom ?? byId.get(projectId) ?? { id: projectId, name: session.project, sessionIds: [], sessions: [] };
    if (!byId.has(group.id)) { groups.push(group); byId.set(group.id, group); }
    group.sessions.push(session);
    if (!assigned.has(id) && !group.sessionIds.includes(id)) group.sessionIds.push(id);
  }
  return groups.filter((group) => group.sessions.length || !group.id.startsWith("project:"))
    .map((group) => ({ ...group, sessions: group.sessions.sort((a, b) => rank(a, snapshot.pendingActions) - rank(b, snapshot.pendingActions) || a.repo.localeCompare(b.repo)) }));
}

export const pendingOldestFirst = (snapshot: BoardSnapshot) =>
  [...snapshot.pendingActions].sort((a, b) => Date.parse(a.createdAt) - Date.parse(b.createdAt));

export function moveSession(layout: Layout, groups: BoardGroup[], id: string, targetId: string): Layout {
  return { groups: groups.map((group) => ({
    id: group.id,
    name: group.name,
    sessionIds: group.sessionIds.filter((item) => item !== id).concat(group.id === targetId ? [id] : []),
  })) };
}
