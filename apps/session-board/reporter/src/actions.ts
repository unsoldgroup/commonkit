import type { Decision, OrcaGateAction, PendingAction, Verdict } from "@commonkit/session-board-protocol";
import { parseCommandJson, runCommand, type RunCommand } from "./process.js";

type GateRecord = Record<string, unknown>;

function asRecord(value: unknown): GateRecord | undefined {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value as GateRecord : undefined;
}

function strings(value: unknown): string[] {
  if (Array.isArray(value)) return value.filter((item): item is string => typeof item === "string" && item.length > 0);
  if (typeof value === "string") {
    try { return strings(JSON.parse(value)); } catch { return []; }
  }
  return [];
}

function date(value: unknown, fallback: Date): string {
  if (typeof value === "number" && Number.isFinite(value)) return new Date(value).toISOString();
  if (typeof value === "string" && !Number.isNaN(Date.parse(value))) return new Date(value).toISOString();
  return fallback.toISOString();
}

export function normalizeOpenGates(payload: unknown, machineId: string, now = new Date()): OrcaGateAction[] {
  const envelope = asRecord(payload);
  if (!envelope?.ok) throw new Error("Orca gate-list failed");
  const gates = asRecord(envelope.result)?.gates;
  if (!Array.isArray(gates)) return [];
  return gates.flatMap((candidate) => {
    const gate = asRecord(candidate);
    const id = typeof gate?.id === "string" ? gate.id : undefined;
    const status = gate?.status;
    if (!gate || !id || (status !== undefined && status !== "open" && status !== "pending")) return [];
    const question = typeof gate.question === "string" && gate.question ? gate.question : "Orca gate requires a decision";
    const createdAt = date(gate.created_at ?? gate.createdAt, now);
    const taskId = typeof gate.task_id === "string" ? gate.task_id : typeof gate.taskId === "string" ? gate.taskId : id;
    return [{
      id,
      kind: "orca-gate" as const,
      sessionRef: { machineId, worktreeId: taskId, paneKey: "orchestration" },
      summary: question,
      detail: { question, options: strings(gate.options).length ? strings(gate.options) : ["Allow", "Deny"] },
      createdAt,
      expiresAt: new Date(new Date(createdAt).getTime() + 24 * 60 * 60 * 1_000).toISOString(),
    }];
  });
}

export interface CodexKeystrokeExecutor {
  execute(action: PendingAction, verdict: Verdict): Promise<void>;
}

export class UnsupportedCodexKeystrokeExecutor implements CodexKeystrokeExecutor {
  async execute(): Promise<void> {
    throw new Error("Codex keystroke decisions are deferred to UNS-1305");
  }
}

export class DecisionExecutor {
  constructor(
    private readonly actions: () => ReadonlyMap<string, PendingAction>,
    private readonly codex: CodexKeystrokeExecutor = new UnsupportedCodexKeystrokeExecutor(),
    private readonly run: RunCommand = runCommand,
  ) {}

  async execute(decision: Decision): Promise<void> {
    const action = this.actions().get(decision.actionId);
    if (!action) throw new Error(`Decision targets unknown action ${decision.actionId}`);
    if (action.kind === "orca-gate") {
      const resolution = decision.verdict.startsWith("option:")
        ? action.detail.options[Number(decision.verdict.slice(7)) - 1]
        : decision.verdict;
      if (!resolution) throw new Error("Decision option is outside the gate option list");
      const result = await this.run(["orca", "orchestration", "gate-resolve", "--id", action.id, "--resolution", resolution, "--json"]);
      parseCommandJson(result, "orca orchestration gate-resolve");
      return;
    }
    if (action.kind === "codex-prompt") return this.codex.execute(action, decision.verdict);
    throw new Error("Claude permissions are resolved by the fail-open hook, not the Mac reporter");
  }
}

export class GatePoller {
  #timer?: ReturnType<typeof setTimeout>;
  #stopped = false;
  constructor(
    private readonly machineId: string,
    private readonly onChange: (actions: OrcaGateAction[]) => void | Promise<void>,
    private readonly run: RunCommand = runCommand,
    private readonly intervalMs = 2_000,
  ) {}
  async start(): Promise<void> {
    const poll = async () => {
      if (this.#stopped) return;
      try {
        const result = await this.run(["orca", "orchestration", "gate-list", "--status", "open", "--json"]);
        await this.onChange(normalizeOpenGates(parseCommandJson(result, "orca orchestration gate-list"), this.machineId));
      } catch (error) {
        console.warn(error instanceof Error ? error.message : error);
      } finally {
        if (!this.#stopped) this.#timer = setTimeout(poll, this.intervalMs);
      }
    };
    await poll();
  }
  stop() { this.#stopped = true; if (this.#timer) clearTimeout(this.#timer); }
}
