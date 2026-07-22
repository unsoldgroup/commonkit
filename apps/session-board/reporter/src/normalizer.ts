import { sessionSchema, type Agent, type Session, type SessionState } from "@commonkit/session-board-protocol";

type RecordValue = Record<string, unknown>;

function record(value: unknown): RecordValue | undefined {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value as RecordValue : undefined;
}

function text(value: unknown, fallback = ""): string {
  return typeof value === "string" && value.length > 0 ? value : fallback;
}

function timestamp(value: unknown, fallback: string): string {
  if (typeof value === "number" && Number.isFinite(value)) return new Date(value).toISOString();
  if (typeof value === "string" && !Number.isNaN(Date.parse(value))) return new Date(value).toISOString();
  return fallback;
}

function agent(value: unknown): Agent {
  return value === "claude" || value === "codex" ? value : "other";
}

function state(value: unknown): SessionState {
  const values: SessionState[] = ["working", "waiting", "blocked", "done", "idle", "interrupted"];
  return values.includes(value as SessionState) ? value as SessionState : "idle";
}

export function normalizeWorktreePs(payload: unknown, machineId: string, now = new Date()): Session[] {
  const envelope = record(payload);
  if (!envelope?.ok) throw new Error("Orca worktree ps failed");
  const worktrees = record(envelope.result)?.worktrees;
  if (!Array.isArray(worktrees)) throw new Error("Orca worktree ps returned no worktrees");
  const fallbackTime = now.toISOString();
  const sessions: Session[] = [];

  for (const candidate of worktrees) {
    const worktree = record(candidate);
    if (!worktree || !Array.isArray(worktree.agents)) continue;
    const worktreeId = text(worktree.worktreeId, text(worktree.worktreeInstanceId));
    if (!worktreeId) continue;
    const nestedRepo = record(worktree.repo);
    const repo = text(nestedRepo?.name, text(worktree.repoId, "Unknown repo"));
    const project = text(worktree.projectId, repo);
    const worktreeLastOutput = timestamp(worktree.lastOutputAt, timestamp(worktree.lastActivityAt, fallbackTime));

    for (const candidateAgent of worktree.agents) {
      const rawAgent = record(candidateAgent);
      if (!rawAgent) continue;
      const paneKey = text(rawAgent.paneKey);
      if (!paneKey) continue;
      const updatedAt = timestamp(rawAgent.updatedAt, worktreeLastOutput);
      sessions.push(sessionSchema.parse({
        machineId,
        worktreeId,
        repo,
        project,
        agent: agent(rawAgent.agentType),
        paneKey,
        state: state(rawAgent.state),
        stateSince: timestamp(rawAgent.stateStartedAt, updatedAt),
        title: text(rawAgent.taskTitle, text(rawAgent.displayName, text(worktree.displayName))),
        lastOutputAt: updatedAt,
      }));
    }
  }
  return sessions.sort((left, right) => left.worktreeId.localeCompare(right.worktreeId) || left.paneKey.localeCompare(right.paneKey));
}
