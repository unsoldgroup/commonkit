import { resolve } from "node:path";
import { parseCommandJson, runCommand, type RunCommand } from "./process.js";

type RecordValue = Record<string, unknown>;

const record = (value: unknown): RecordValue | undefined =>
  value !== null && typeof value === "object" && !Array.isArray(value) ? value as RecordValue : undefined;

export function findClaudePane(payload: unknown, cwd: string): string | undefined {
  const worktrees = record(record(payload)?.result)?.worktrees;
  if (!record(payload)?.ok || !Array.isArray(worktrees)) return undefined;
  const wanted = resolve(cwd);
  for (const candidate of worktrees) {
    const worktree = record(candidate);
    if (typeof worktree?.path !== "string" || resolve(worktree.path) !== wanted || !Array.isArray(worktree.agents)) continue;
    for (const candidateAgent of worktree.agents) {
      const agent = record(candidateAgent);
      if (agent?.agentType === "claude" && typeof agent.paneKey === "string" && agent.paneKey) return agent.paneKey;
    }
  }
}

// `orchestration send --to` addresses a terminal handle, never a paneKey; paneKey is `<tabId>:<leafId>`.
export function findTerminalHandle(payload: unknown, paneKey: string): string | undefined {
  const terminals = record(record(payload)?.result)?.terminals;
  if (!record(payload)?.ok || !Array.isArray(terminals)) return undefined;
  for (const candidate of terminals) {
    const terminal = record(candidate);
    if (typeof terminal?.handle !== "string" || !terminal.handle) continue;
    if (`${terminal.tabId}:${terminal.leafId}` === paneKey) return terminal.handle;
  }
}

export class OrcaSteerer {
  constructor(private readonly run: RunCommand = runCommand) {}

  async send(cwd: string, text: string): Promise<void> {
    try {
      const worktrees = await this.run(["orca", "worktree", "ps", "--json"]);
      const pane = findClaudePane(parseCommandJson(worktrees, "orca worktree ps"), cwd);
      if (!pane) return;
      const terminals = await this.run(["orca", "terminal", "list", "--json"]);
      const handle = findTerminalHandle(parseCommandJson(terminals, "orca terminal list"), pane);
      if (!handle) return;
      await this.run([
        "orca", "orchestration", "send",
        "--to", handle,
        "--subject", "Permission denied — user guidance",
        "--body", text,
        "--type", "status",
        "--json",
      ]);
    } catch { /* fail open */ }
  }
}
