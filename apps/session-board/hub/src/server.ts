import { timingSafeEqual } from "node:crypto";
import { mkdir, rename, writeFile } from "node:fs/promises";
import { dirname, isAbsolute, join, normalize, relative, sep } from "node:path";

import {
  boardSnapshotSchema,
  decisionRequestSchema,
  reporterToHubMessageSchema,
  type BoardSnapshot,
  type Machine,
  type PendingAction,
  type ReporterToHubMessage,
  type Session,
  type SseEvent,
} from "@commonkit/session-board-protocol";

type ReporterSocketData = { machineId: string };
type Socket = Bun.ServerWebSocket<ReporterSocketData>;

export interface HubOptions {
  reporterTokens: Record<string, string>;
  actionToken: string;
  hostname?: string;
  port?: number;
  eventBufferSize?: number;
  layoutPath?: string;
  webDistPath?: string;
  now?: () => Date;
}

export interface HubServer {
  url: string;
  readonly latestEventId: number;
  stop(): Promise<void>;
}

type BufferedEvent = { id: number; event: SseEvent };

const jsonHeaders = { "Content-Type": "application/json; charset=utf-8" };

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), { status, headers: jsonHeaders });
}

function secureEqual(left: string, right: string) {
  const leftBytes = Buffer.from(left);
  const rightBytes = Buffer.from(right);
  return leftBytes.length === rightBytes.length && timingSafeEqual(leftBytes, rightBytes);
}

function bearerToken(request: Request) {
  const match = /^Bearer\s+(.+)$/i.exec(request.headers.get("Authorization") ?? "");
  return match?.[1];
}

function sessionKey(session: Pick<Session, "machineId" | "worktreeId" | "paneKey">) {
  return JSON.stringify([session.machineId, session.worktreeId, session.paneKey]);
}

function encodeEvent(item: BufferedEvent) {
  return `id: ${item.id}\nevent: ${item.event.type}\ndata: ${JSON.stringify(item.event)}\n\n`;
}

export function startHub(options: HubOptions): HubServer {
  const now = options.now ?? (() => new Date());
  const eventBufferSize = Math.max(1, options.eventBufferSize ?? 256);
  const layoutPath = options.layoutPath ?? join(import.meta.dir, "../data/layout.json");
  const webDistPath = options.webDistPath ?? join(import.meta.dir, "../../web/dist");
  const machines = new Map<string, Machine>();
  const sessions = new Map<string, Session>();
  const actions = new Map<string, PendingAction>();
  const reporters = new Map<string, Socket>();
  const subscribers = new Set<ReadableStreamDefaultController<Uint8Array>>();
  const eventBuffer: BufferedEvent[] = [];
  const tailRequests = new Map<string, { machineId: string; resolve: (lines: string[]) => void; timer: ReturnType<typeof setTimeout> }>();
  const encoder = new TextEncoder();
  let latestEventId = 0;

  const snapshot = (): BoardSnapshot =>
    boardSnapshotSchema.parse({
      machines: [...machines.values()].sort((a, b) => a.id.localeCompare(b.id)),
      sessions: [...sessions.values()].sort((a, b) => sessionKey(a).localeCompare(sessionKey(b))),
      pendingActions: [...actions.values()].sort((a, b) => a.id.localeCompare(b.id)),
    });

  const publish = (event: SseEvent) => {
    const item = { id: ++latestEventId, event };
    eventBuffer.push(item);
    while (eventBuffer.length > eventBufferSize) eventBuffer.shift();
    const bytes = encoder.encode(encodeEvent(item));
    for (const subscriber of subscribers) {
      try {
        subscriber.enqueue(bytes);
      } catch {
        subscribers.delete(subscriber);
      }
    }
  };

  const assertMachineOwnership = (message: Exclude<ReporterToHubMessage, { type: "hello" }>, machineId: string) => {
    if (message.type === "actionOpened" && message.action.sessionRef.machineId !== machineId) {
      throw new Error("action does not belong to authenticated machine");
    }
    if ((message.type === "stateSnapshot" || message.type === "stateDelta") && message.machine.id !== machineId) {
      throw new Error("state does not belong to authenticated machine");
    }
  };

  const assertActionOwnership = (action: PendingAction, machineId: string) => {
    if (action.sessionRef.machineId !== machineId) throw new Error("action ownership mismatch");
    const existing = actions.get(action.id);
    if (existing && existing.sessionRef.machineId !== machineId) {
      throw new Error("action id is already owned by another machine");
    }
  };

  const handleReporterMessage = (socket: Socket, raw: string | Buffer) => {
    const message = reporterToHubMessageSchema.parse(JSON.parse(raw.toString()));
    if (message.type === "hello") return;
    if (message.type === "tailResponse") {
      const pending = tailRequests.get(message.requestId);
      if (pending?.machineId === socket.data.machineId) {
        clearTimeout(pending.timer);
        tailRequests.delete(message.requestId);
        pending.resolve(message.lines);
      }
      return;
    }
    assertMachineOwnership(message, socket.data.machineId);

    if (message.type === "stateSnapshot") {
      for (const session of message.sessions) {
        if (session.machineId !== socket.data.machineId) throw new Error("session ownership mismatch");
      }
      for (const action of message.pendingActions) assertActionOwnership(action, socket.data.machineId);
      for (const [key, session] of sessions) {
        if (session.machineId === socket.data.machineId) sessions.delete(key);
      }
      for (const [id, action] of actions) {
        if (action.sessionRef.machineId === socket.data.machineId) actions.delete(id);
      }
      machines.set(socket.data.machineId, { ...message.machine, online: true });
      for (const session of message.sessions) sessions.set(sessionKey(session), session);
      for (const action of message.pendingActions) actions.set(action.id, action);
      publish({ type: "snapshot", data: snapshot() });
      return;
    }

    if (message.type === "stateDelta") {
      for (const session of message.sessions.upsert) {
        if (session.machineId !== socket.data.machineId) throw new Error("session ownership mismatch");
      }
      for (const ref of message.sessions.remove) {
        if (ref.machineId !== socket.data.machineId) throw new Error("session ownership mismatch");
      }
      for (const action of message.pendingActions.upsert) assertActionOwnership(action, socket.data.machineId);
      machines.set(socket.data.machineId, { ...message.machine, online: true });
      for (const session of message.sessions.upsert) sessions.set(sessionKey(session), session);
      for (const ref of message.sessions.remove) sessions.delete(sessionKey(ref));
      for (const action of message.pendingActions.upsert) actions.set(action.id, action);
      for (const id of message.pendingActions.remove) {
        const action = actions.get(id);
        if (action?.sessionRef.machineId === socket.data.machineId) actions.delete(id);
      }
      publish({ type: "snapshot", data: snapshot() });
      return;
    }

    if (message.type === "actionOpened") {
      assertActionOwnership(message.action, socket.data.machineId);
      actions.set(message.action.id, message.action);
      publish({ type: "actionOpened", data: message.action });
      return;
    }

    const action = actions.get(message.actionId);
    if (action?.sessionRef.machineId === socket.data.machineId) {
      actions.delete(message.actionId);
      publish({ type: "actionClosed", data: { actionId: message.actionId } });
    }
  };

  const server = Bun.serve<ReporterSocketData>({
    hostname: options.hostname ?? "127.0.0.1",
    port: options.port ?? 0,
    async fetch(request, server) {
      const url = new URL(request.url);

      if (url.pathname === "/reporter") {
        const token = bearerToken(request);
        const machineId = token
          ? Object.entries(options.reporterTokens).find(([, candidate]) => secureEqual(token, candidate))?.[0]
          : undefined;
        if (!machineId) return json({ error: "unauthorized" }, 401);
        if (!server.upgrade(request, { data: { machineId } })) return json({ error: "upgrade required" }, 426);
        return undefined;
      }

      if (url.pathname === "/state" && request.method === "GET") return json(snapshot());

      if (url.pathname === "/events" && request.method === "GET") {
        const lastId = Number.parseInt(request.headers.get("Last-Event-ID") ?? "", 10);
        let controller: ReadableStreamDefaultController<Uint8Array> | undefined;
        const stream = new ReadableStream<Uint8Array>({
          start(current) {
            controller = current;
            subscribers.add(current);
            if (!Number.isFinite(lastId)) {
              current.enqueue(encoder.encode(encodeEvent({
                id: latestEventId,
                event: { type: "snapshot", data: snapshot() },
              })));
            } else {
              const firstBufferedId = eventBuffer[0]?.id ?? latestEventId + 1;
              if (lastId < firstBufferedId - 1) {
                current.enqueue(encoder.encode(encodeEvent({
                  id: latestEventId,
                  event: { type: "snapshot", data: snapshot() },
                })));
              } else {
                for (const item of eventBuffer) if (item.id > lastId) current.enqueue(encoder.encode(encodeEvent(item)));
              }
            }
          },
          cancel() {
            if (controller) subscribers.delete(controller);
          },
        });
        return new Response(stream, {
          headers: {
            "Content-Type": "text/event-stream",
            "Cache-Control": "no-cache, no-transform",
            Connection: "keep-alive",
          },
        });
      }

      const decisionMatch = /^\/actions\/([^/]+)\/decision$/.exec(url.pathname);
      if (decisionMatch && request.method === "POST") {
        const token = bearerToken(request);
        if (!token || !secureEqual(token, options.actionToken)) return json({ error: "unauthorized" }, 401);
        const actionId = decodeURIComponent(decisionMatch[1]);
        const action = actions.get(actionId);
        const reporter = action ? reporters.get(action.sessionRef.machineId) : undefined;
        if (!action || !reporter) {
          return json({ error: "action closed or Target offline" }, 410);
        }
        let requestBody;
        try {
          requestBody = decisionRequestSchema.parse(await request.json());
        } catch {
          return json({ error: "invalid decision" }, 400);
        }
        if (actions.get(actionId) !== action || reporters.get(action.sessionRef.machineId) !== reporter) {
          return json({ error: "action closed or Target offline" }, 410);
        }
        const decision = { actionId, verdict: requestBody.verdict, decidedAt: now().toISOString() };
        let sentBytes = 0;
        try {
          sentBytes = reporter.send(JSON.stringify({ type: "decision", decision }));
        } catch {
          // The close callback owns offline marking; retain the action so a reconnect can retry it.
        }
        if (sentBytes <= 0) return json({ error: "action closed or Target offline" }, 410);
        actions.delete(actionId);
        publish({ type: "decision", data: decision });
        return json({ decision });
      }

      const tailMatch = /^\/sessions\/([^/]+)\/tail$/.exec(url.pathname);
      if (tailMatch && request.method === "GET") {
        let ref: Pick<Session, "machineId" | "worktreeId" | "paneKey">;
        try {
          const value = JSON.parse(decodeURIComponent(tailMatch[1]));
          if (!Array.isArray(value) || value.length !== 3 || value.some((item) => typeof item !== "string")) throw new Error();
          ref = { machineId: value[0], worktreeId: value[1], paneKey: value[2] };
        } catch {
          return json({ error: "invalid session id" }, 400);
        }
        if (!sessions.has(sessionKey(ref))) return json({ error: "session not found" }, 404);
        const reporter = reporters.get(ref.machineId);
        if (!reporter) return json({ error: "Target offline" }, 410);
        const requestId = crypto.randomUUID();
        const lines = await new Promise<string[] | undefined>((resolve) => {
          const timer = setTimeout(() => { tailRequests.delete(requestId); resolve(undefined); }, 5_000);
          tailRequests.set(requestId, { machineId: ref.machineId, resolve, timer });
          if (reporter.send(JSON.stringify({ type: "tailRequest", requestId, sessionRef: ref })) <= 0) {
            clearTimeout(timer);
            tailRequests.delete(requestId);
            resolve(undefined);
          }
        });
        return lines ? json({ lines }) : json({ error: "tail request timed out" }, 504);
      }

      if (url.pathname === "/layout" && request.method === "GET") {
        const file = Bun.file(layoutPath);
        if (!(await file.exists())) return json({ groups: [] });
        try {
          return json(await file.json());
        } catch {
          return json({ error: "stored layout is invalid" }, 500);
        }
      }

      if (url.pathname === "/layout" && request.method === "PUT") {
        let layout: unknown;
        try {
          layout = await request.json();
          JSON.stringify(layout);
        } catch {
          return json({ error: "invalid JSON" }, 400);
        }
        await mkdir(dirname(layoutPath), { recursive: true });
        const temporaryPath = `${layoutPath}.${crypto.randomUUID()}.tmp`;
        await writeFile(temporaryPath, `${JSON.stringify(layout, null, 2)}\n`, { mode: 0o600 });
        await rename(temporaryPath, layoutPath);
        return json(layout);
      }

      if (request.method === "GET") {
        const relativePath = url.pathname === "/" ? "index.html" : decodeURIComponent(url.pathname.slice(1));
        const candidate = normalize(join(webDistPath, relativePath));
        const assetPath = relative(webDistPath, candidate);
        if (assetPath !== ".." && !assetPath.startsWith(`..${sep}`) && !isAbsolute(assetPath)) {
          const file = Bun.file(candidate);
          if (await file.exists()) return new Response(file);
        }
      }

      return json({ error: "not found" }, 404);
    },
    websocket: {
      open(socket) {
        const previous = reporters.get(socket.data.machineId);
        if (previous && previous !== socket) previous.close(4001, "replaced by newer reporter");
        reporters.set(socket.data.machineId, socket);
      },
      message(socket, message) {
        try {
          handleReporterMessage(socket, message);
        } catch {
          socket.close(1008, "invalid reporter message");
        }
      },
      close(socket) {
        if (reporters.get(socket.data.machineId) !== socket) return;
        reporters.delete(socket.data.machineId);
        const machine = machines.get(socket.data.machineId);
        if (!machine) return;
        const offline = { ...machine, online: false, lastSeenAt: now().toISOString() };
        machines.set(socket.data.machineId, offline);
        publish({ type: "machine", data: offline });
      },
    },
  });

  return {
    url: server.url.origin,
    get latestEventId() {
      return latestEventId;
    },
    async stop() {
      for (const pending of tailRequests.values()) { clearTimeout(pending.timer); pending.resolve([]); }
      tailRequests.clear();
      for (const subscriber of subscribers) subscriber.close();
      subscribers.clear();
      await server.stop(true);
    },
  };
}
