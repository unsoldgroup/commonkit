import { readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";
import type { Session } from "@commonkit/session-board-protocol";
import { normalizeWorktreePs } from "./normalizer.js";
import { parseCommandJson, runCommand, type RunCommand } from "./process.js";

export type StateListener = (sessions: Session[]) => void | Promise<void>;

export interface StateSource {
  readonly name: string;
  start(listener: StateListener): Promise<void>;
  stop(): Promise<void>;
}

interface RuntimeDescriptor {
  authToken: string;
  transports: Array<{ kind: string; endpoint: string }>;
}

export interface WebSocketStateSourceOptions {
  machineId: string;
  runtimePath?: string;
  probeTimeoutMs?: number;
  webSocketFactory?: (url: string, options?: { headers: Record<string, string> }) => WebSocket;
}

export class OrcaWebSocketStateSource implements StateSource {
  readonly name = "orca-json-rpc-websocket";
  #socket?: WebSocket;
  #stopped = false;
  #timer?: ReturnType<typeof setInterval>;

  constructor(private readonly options: WebSocketStateSourceOptions) {}

  async start(listener: StateListener): Promise<void> {
    const runtimePath = this.options.runtimePath ?? join(homedir(), "Library/Application Support/Orca/orca-runtime.json");
    const descriptor = JSON.parse(await readFile(runtimePath, "utf8")) as RuntimeDescriptor;
    const endpoint = descriptor.transports?.find((transport) => transport.kind === "websocket")?.endpoint;
    if (!endpoint || !descriptor.authToken) throw new Error("Orca runtime descriptor has no WebSocket transport or auth token");
    const url = new URL(endpoint.replace("0.0.0.0", "127.0.0.1"));
    url.searchParams.set("authToken", descriptor.authToken);
    const createSocket = this.options.webSocketFactory ?? ((address, options) => {
      const Constructor = WebSocket as unknown as new (url: string, options?: { headers: Record<string, string> }) => WebSocket;
      return new Constructor(address, options);
    });
    const socket = createSocket(url.toString(), { headers: { Authorization: `Bearer ${descriptor.authToken}` } });
    this.#socket = socket;
    const id = crypto.randomUUID();
    const timeoutMs = this.options.probeTimeoutMs ?? 2_000;

    await new Promise<void>((resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error("Orca JSON-RPC framing probe timed out")), timeoutMs);
      const fail = (error: unknown) => { clearTimeout(timeout); reject(error instanceof Error ? error : new Error("Orca WebSocket failed")); };
      socket.addEventListener("open", () => {
        // Candidate framing: the bundle confirms JSON-RPC 2.0, while the command method remains undocumented.
        socket.send(JSON.stringify({
          jsonrpc: "2.0",
          id,
          method: "command.execute",
          params: { authToken: descriptor.authToken, command: "worktree ps", argv: ["worktree", "ps"], json: true },
        }));
      }, { once: true });
      socket.addEventListener("error", () => fail(new Error("Orca WebSocket connection failed")), { once: true });
      socket.addEventListener("close", () => { if (!this.#stopped) fail(new Error("Orca WebSocket closed during framing probe")); }, { once: true });
      socket.addEventListener("message", async (event) => {
        try {
          const message = JSON.parse(String(event.data)) as Record<string, unknown>;
          if (message.id !== id) return;
          if (message.error) throw new Error("Orca rejected JSON-RPC command framing");
          const payload = message.result;
          await listener(normalizeWorktreePs(payload, this.options.machineId));
          clearTimeout(timeout);
          if (!this.#timer && !this.#stopped) {
            this.#timer = setInterval(() => {
              if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify({
                jsonrpc: "2.0", id, method: "command.execute",
                params: { authToken: descriptor.authToken, command: "worktree ps", argv: ["worktree", "ps"], json: true },
              }));
            }, 2_000);
          }
          resolve();
        } catch (error) {
          fail(error);
        }
      });
    });
  }

  async stop(): Promise<void> {
    this.#stopped = true;
    if (this.#timer) clearInterval(this.#timer);
    this.#socket?.close();
  }
}

export interface PollingStateSourceOptions {
  machineId: string;
  intervalMs?: number;
  run?: RunCommand;
}

export class OrcaCliPollingStateSource implements StateSource {
  readonly name = "orca-cli-poll";
  #timer?: ReturnType<typeof setTimeout>;
  #stopped = false;

  constructor(private readonly options: PollingStateSourceOptions) {}

  async start(listener: StateListener): Promise<void> {
    const poll = async () => {
      if (this.#stopped) return;
      try {
        const result = await (this.options.run ?? runCommand)(["orca", "worktree", "ps", "--json"]);
        await listener(normalizeWorktreePs(parseCommandJson(result, "orca worktree ps"), this.options.machineId));
      } catch (error) {
        console.warn(error instanceof Error ? error.message : error);
      } finally {
        if (!this.#stopped) this.#timer = setTimeout(poll, this.options.intervalMs ?? 2_000);
      }
    };
    await poll();
  }

  async stop(): Promise<void> {
    this.#stopped = true;
    if (this.#timer) clearTimeout(this.#timer);
  }
}

export class FallbackStateSource implements StateSource {
  readonly name = "orca-auto";
  #active?: StateSource;
  constructor(private readonly primary: StateSource, private readonly fallback: StateSource, private readonly warn = console.warn) {}

  async start(listener: StateListener): Promise<void> {
    try {
      this.#active = this.primary;
      await this.primary.start(listener);
    } catch (error) {
      await this.primary.stop();
      this.warn(`Primary Orca state source unavailable; using CLI polling: ${error instanceof Error ? error.message : String(error)}`);
      this.#active = this.fallback;
      await this.fallback.start(listener);
    }
  }

  async stop(): Promise<void> { await this.#active?.stop(); }
}
