import {
  claudeHookRequestSchema,
  type ClaudeHookResponse,
  type ClaudePermissionAction,
  type Decision,
} from "@commonkit/session-board-protocol";
import { createServer, type Server } from "node:http";
import { open } from "node:fs/promises";

export interface ClaudeHookServerOptions {
  machineId: string;
  port?: number;
  timeoutMs?: number;
  now?: () => Date;
  openAction: (action: ClaudePermissionAction) => void;
  closeAction: (actionId: string, outcome?: "allowed" | "denied" | "stale" | "failed") => void;
}

const truncate = (value: string, max = 200) => (value.length > max ? `${value.slice(0, max - 1)}…` : value);

export function describePermission(tool: string, input: unknown, cwd: string): string {
  const record = (input && typeof input === "object" ? input : {}) as Record<string, unknown>;
  const text = (key: string) => (typeof record[key] === "string" && record[key] ? (record[key] as string) : undefined);
  const project = cwd.split("/").filter(Boolean).pop() ?? cwd;
  const relative = (path: string) => (path.startsWith(`${cwd}/`) ? path.slice(cwd.length + 1) : path);
  const body =
    tool === "Bash" ? (text("description") ? `${text("description")} — ${text("command") ?? ""}` : text("command")) :
    tool === "Edit" || tool === "Write" || tool === "NotebookEdit" ? `${tool} ${relative(text("file_path") ?? text("notebook_path") ?? "file")}` :
    tool === "WebFetch" ? `Fetch ${text("url") ?? "URL"}` :
    tool === "WebSearch" ? `Search "${text("query") ?? ""}"` :
    tool === "Task" ? `Agent: ${text("description") ?? text("prompt") ?? "subagent"}` :
    undefined;
  return truncate(body ? `[${project}] ${body}` : `[${project}] Allow ${tool}`);
}

export async function lastAssistantText(transcriptPath: string | undefined, maxBytes = 131_072): Promise<string | undefined> {
  if (!transcriptPath) return undefined;
  try {
    const handle = await open(transcriptPath, "r");
    try {
      const { size } = await handle.stat();
      const start = Math.max(0, size - maxBytes);
      const buffer = Buffer.alloc(size - start);
      await handle.read(buffer, 0, buffer.length, start);
      const lines = buffer.toString("utf8").split("\n");
      for (let index = lines.length - 1; index >= 0; index -= 1) {
        try {
          const entry = JSON.parse(lines[index]!);
          if (entry?.type !== "assistant") continue;
          const content = entry?.message?.content;
          const blocks = Array.isArray(content) ? content : [];
          const text = blocks.filter((block: { type?: string; text?: string }) => block?.type === "text" && typeof block.text === "string")
            .map((block: { text: string }) => block.text).join(" ").trim();
          if (text) return truncate(text.replaceAll(/\s+/g, " "), 280);
        } catch { /* partial or non-JSON line */ }
      }
      return undefined;
    } finally { await handle.close(); }
  } catch { return undefined; }
}

export class ClaudeHookServer {
  #pending = new Map<string, (decision: Decision) => void>();
  #server?: Server;

  constructor(private readonly options: ClaudeHookServerOptions) {}

  start() {
    if (this.#server) return;
    this.#server = createServer((request, response) => {
      const chunks: Buffer[] = [];
      request.on("data", (chunk) => chunks.push(Buffer.from(chunk)));
      request.on("end", () => void this.#handle(request.method, request.url, Buffer.concat(chunks).toString("utf8")).then(({ status, body }) => {
        response.statusCode = status;
        if (body !== undefined) {
          response.setHeader("Content-Type", "application/json; charset=utf-8");
          response.end(JSON.stringify(body));
        } else response.end();
      }));
    });
    this.#server.listen(this.options.port ?? 47821, "127.0.0.1");
  }

  resolve(decision: Decision) {
    const resolve = this.#pending.get(decision.actionId);
    if (!resolve) return false;
    resolve(decision);
    return true;
  }

  async stop() {
    const server = this.#server;
    this.#server = undefined;
    if (server) {
      server.closeAllConnections();
      await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
    }
  }

  async #handle(method: string | undefined, path: string | undefined, rawBody: string): Promise<{ status: number; body?: unknown }> {
    if (method !== "POST" || path !== "/claude/permission") return { status: 404, body: { error: "not found" } };
    let payload;
    try { payload = claudeHookRequestSchema.parse(JSON.parse(rawBody)); }
    catch { return { status: 400, body: { error: "invalid hook request" } }; }

    const now = this.options.now ?? (() => new Date());
    const timeoutMs = Math.min(55_000, Math.max(1, this.options.timeoutMs ?? 54_000));
    const action: ClaudePermissionAction = {
      id: crypto.randomUUID(),
      kind: "claude-permission",
      sessionRef: { machineId: this.options.machineId, worktreeId: payload.session.cwd, paneKey: payload.session.id },
      summary: describePermission(payload.tool, payload.input, payload.session.cwd),
      detail: {
        tool: payload.tool,
        input: payload.input,
        ...(await lastAssistantText(payload.session.transcriptPath).then((intent) => (intent ? { intent } : {}))),
      },
      createdAt: now().toISOString(),
      expiresAt: new Date(now().getTime() + timeoutMs).toISOString(),
    };
    const decision = await new Promise<Decision | undefined>((resolve) => {
      const timer = setTimeout(() => { this.#pending.delete(action.id); resolve(undefined); }, timeoutMs);
      this.#pending.set(action.id, (value) => { clearTimeout(timer); this.#pending.delete(action.id); resolve(value); });
      this.options.openAction(action);
    });
    if (!decision || (decision.verdict !== "allow" && decision.verdict !== "deny")) {
      this.options.closeAction(action.id, "stale");
      return { status: 204 };
    }
    const body: ClaudeHookResponse = { decision: decision.verdict };
    this.options.closeAction(action.id, decision.verdict === "allow" ? "allowed" : "denied");
    return { status: 200, body };
  }
}
