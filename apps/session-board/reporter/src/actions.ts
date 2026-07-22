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

type CodexOutcome = { actionId: string; outcome: "allowed" | "denied" | "stale" | "failed" };

const CODEX_PROMPT = /Allow Codex to run|Yes,\s*proceed|don['’]t ask again for/i;

export function isCodexApprovalPrompt(text: string) {
  return CODEX_PROMPT.test(text) && !(/Goal blocked \(\/goal resume\)/i.test(text) && !/Allow Codex to run|Yes,\s*proceed/i.test(text));
}

function terminal(payload: unknown) {
  return asRecord(asRecord(asRecord(payload)?.result)?.terminal);
}

function readFrame(result: Awaited<ReturnType<RunCommand>>, label: string) {
  const value = terminal(parseCommandJson(result, label));
  const tail = Array.isArray(value?.tail) ? value.tail.filter((line): line is string => typeof line === "string") : [];
  return { text: tail.join("\n"), cursor: value?.nextCursor };
}

export class CodexTerminalExecutor implements CodexKeystrokeExecutor {
  constructor(
    private readonly run: RunCommand = runCommand,
    private readonly onClosed: (outcome: CodexOutcome) => void | Promise<void> = () => {},
  ) {}

  async execute(action: PendingAction, verdict: Verdict): Promise<void> {
    if (action.kind !== "codex-prompt") throw new Error("Codex executor requires a Codex prompt action");
    const terminalHandle = action.sessionRef.paneKey;
    let outcome: CodexOutcome["outcome"] = "failed";
    try {
      parseCommandJson(await this.run(["orca", "terminal", "wait", "--terminal", terminalHandle, "--for", "tui-idle", "--timeout-ms", "5000", "--json"]), "orca terminal wait");
      const before = readFrame(await this.run(["orca", "terminal", "read", "--terminal", terminalHandle, "--json"]), "orca terminal read");
      if (!isCodexApprovalPrompt(before.text) || !before.text.includes(action.detail.prompt)) {
        outcome = "stale";
        throw new Error("Codex approval prompt is no longer present");
      }
      const option = verdict.startsWith("option:") ? Number(verdict.slice(7)) : action.detail.options.findIndex((label) => verdict === "allow" ? /^yes\b/i.test(label) : /^no\b/i.test(label)) + 1;
      const label = action.detail.options[option - 1];
      if (!label || !new RegExp(`(?:^|\\n)\\s*${option}\\.\\s*${label.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`, "i").test(before.text)) {
        outcome = "stale";
        throw new Error("Codex approval option no longer matches the expected prompt");
      }
      parseCommandJson(await this.run(["orca", "terminal", "send", "--terminal", terminalHandle, "--text", String(option), "--enter", "--json"]), "orca terminal send");
      const afterArgs = ["orca", "terminal", "read", "--terminal", terminalHandle];
      if (typeof before.cursor === "number" || typeof before.cursor === "string") afterArgs.push("--cursor", String(before.cursor));
      afterArgs.push("--json");
      const after = readFrame(await this.run(afterArgs), "orca terminal read confirmation");
      if (isCodexApprovalPrompt(after.text)) throw new Error("Codex approval prompt was not consumed");
      outcome = verdict === "deny" || (verdict.startsWith("option:") && /^no\b/i.test(label)) ? "denied" : "allowed";
    } finally {
      await this.onClosed({ actionId: action.id, outcome });
    }
  }
}

export class DecisionExecutor {
  constructor(
    private readonly actions: () => ReadonlyMap<string, PendingAction>,
    private readonly codex: CodexKeystrokeExecutor = new CodexTerminalExecutor(),
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
        const result = await this.run(["orca", "orchestration", "gate-list", "--status", "pending", "--json"]);
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
