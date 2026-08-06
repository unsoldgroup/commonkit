import { timingSafeEqual } from "node:crypto";
import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import { dirname, isAbsolute, join, normalize, relative, sep } from "node:path";

import {
  boardSnapshotSchema,
  decisionRecordSchema,
  decisionRequestSchema,
  decisionValidForAction,
  decisionsResponseSchema,
  reporterToHubMessageSchema,
  type BoardSnapshot,
  type DecisionRecord,
  type Machine,
  type PendingAction,
  type ReporterToHubMessage,
  type Session,
  type SseEvent,
} from "@commonkit/session-board-protocol";

type ReporterSocketData = { machineId: string };
type Socket = Bun.ServerWebSocket<ReporterSocketData>;

export interface PushSubscription {
  endpoint: string;
  expirationTime?: number | null;
  keys: { p256dh: string; auth: string };
}

export interface PushSender {
  send(subscription: PushSubscription, payload: string): Promise<void>;
}

export interface HubOptions {
  reporterTokens: Record<string, string>;
  actionToken: string;
  hostname?: string;
  port?: number;
  eventBufferSize?: number;
  layoutPath?: string;
  dataDir?: string;
  webDistPath?: string;
  now?: () => Date;
  push?: { publicKey: string; sender: PushSender };
  /** Resolves the Cloudflare Access identity on a request, or undefined when there is none. */
  access?: (request: Request) => Promise<string | undefined>;
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

async function readDecisionRecords(path: string): Promise<DecisionRecord[]> {
  try {
    const stored = JSON.parse(await readFile(path, "utf8"));
    return decisionsResponseSchema.parse(stored).records;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return [];
    throw error;
  }
}

async function writeJson(path: string, value: unknown) {
  await mkdir(dirname(path), { recursive: true });
  const temporaryPath = `${path}.${crypto.randomUUID()}.tmp`;
  await writeFile(temporaryPath, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600 });
  await rename(temporaryPath, path);
}

function parsePushSubscription(value: unknown): PushSubscription {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("invalid subscription");
  const subscription = value as Record<string, unknown>;
  const keys = subscription.keys;
  if (
    typeof subscription.endpoint !== "string"
    || !subscription.endpoint
    || !keys
    || typeof keys !== "object"
    || Array.isArray(keys)
    || typeof (keys as Record<string, unknown>).p256dh !== "string"
    || typeof (keys as Record<string, unknown>).auth !== "string"
    || (subscription.expirationTime !== undefined
      && subscription.expirationTime !== null
      && typeof subscription.expirationTime !== "number")
  ) {
    throw new Error("invalid subscription");
  }
  return subscription as unknown as PushSubscription;
}

export function startHub(options: HubOptions): HubServer {
  const now = options.now ?? (() => new Date());
  const eventBufferSize = Math.max(1, options.eventBufferSize ?? 256);
  const dataDir = options.dataDir ?? (options.layoutPath ? dirname(options.layoutPath) : join(import.meta.dir, "../data"));
  const layoutPath = options.layoutPath ?? join(dataDir, "layout.json");
  const decisionsPath = join(dataDir, "decisions.json");
  const pushSubscriptionsPath = join(dataDir, "push-subscriptions.json");
  const webDistPath = options.webDistPath ?? join(import.meta.dir, "../../web/dist");
  const machines = new Map<string, Machine>();
  const sessions = new Map<string, Session>();
  const actions = new Map<string, PendingAction>();
  const actionOwners = new Map<string, string>();
  const reporters = new Map<string, Socket>();
  const subscribers = new Set<ReadableStreamDefaultController<Uint8Array>>();
  const eventBuffer: BufferedEvent[] = [];
  const tailRequests = new Map<string, { machineId: string; resolve: (lines: string[]) => void; timer: ReturnType<typeof setTimeout> }>();
  const encoder = new TextEncoder();
  let latestEventId = 0;
  let decisionWrite = Promise.resolve();
  let subscriptionWrite = Promise.resolve();

  const appendDecision = (record: DecisionRecord) => {
    decisionWrite = decisionWrite.catch(() => {}).then(async () => {
      const cutoff = now().getTime() - 7 * 24 * 60 * 60 * 1_000;
      const records = (await readDecisionRecords(decisionsPath))
        .filter((item) => Date.parse(item.decidedAt) >= cutoff);
      records.push(decisionRecordSchema.parse(record));
      await writeJson(decisionsPath, { records });
    });
    return decisionWrite;
  };

  const readPushSubscriptions = async (): Promise<Record<string, PushSubscription>> => {
    try {
      const stored = JSON.parse(await readFile(pushSubscriptionsPath, "utf8"));
      if (!stored || typeof stored !== "object" || Array.isArray(stored)) throw new Error("invalid subscriptions");
      const parsed: Record<string, PushSubscription> = {};
      for (const [endpoint, value] of Object.entries(stored)) {
        const subscription = parsePushSubscription(value);
        if (subscription.endpoint !== endpoint) throw new Error("subscription endpoint mismatch");
        parsed[endpoint] = subscription;
      }
      return parsed;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT") return {};
      throw error;
    }
  };

  const updatePushSubscriptions = (update: (subscriptions: Record<string, PushSubscription>) => void) => {
    subscriptionWrite = subscriptionWrite.catch(() => {}).then(async () => {
      const subscriptions = await readPushSubscriptions();
      update(subscriptions);
      await writeJson(pushSubscriptionsPath, subscriptions);
    });
    return subscriptionWrite;
  };

  const notifyNewAction = (action: PendingAction) => {
    if (actionOwners.has(action.id)) return;
    actionOwners.set(action.id, action.sessionRef.machineId);
    if (!options.push) return;
    const payload = JSON.stringify({
      title: "Session Board approval needed",
      body: action.summary,
      actionId: action.id,
    });
    void (async () => {
      await subscriptionWrite;
      const subscriptions = await readPushSubscriptions();
      const expired: string[] = [];
      await Promise.all(Object.values(subscriptions).map(async (subscription) => {
        try {
          await options.push!.sender.send(subscription, payload);
        } catch (error) {
          const statusCode = (error as { statusCode?: number }).statusCode;
          if (statusCode === 404 || statusCode === 410) expired.push(subscription.endpoint);
          else console.error("Session Board push delivery failed", error);
        }
      }));
      if (expired.length > 0) {
        await updatePushSubscriptions((current) => {
          for (const endpoint of expired) delete current[endpoint];
        });
      }
    })().catch((error) => console.error("Session Board push delivery failed", error));
  };

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
    const owner = actionOwners.get(action.id);
    if (owner && owner !== machineId) throw new Error("action id was owned by another machine");
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
      for (const action of message.pendingActions) {
        actions.set(action.id, action);
        notifyNewAction(action);
      }
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
      for (const action of message.pendingActions.upsert) {
        actions.set(action.id, action);
        notifyNewAction(action);
      }
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
      notifyNewAction(message.action);
      publish({ type: "actionOpened", data: message.action });
      return;
    }

    const action = actions.get(message.actionId);
    if (action?.sessionRef.machineId === socket.data.machineId) actions.delete(message.actionId);
    if (
      action?.sessionRef.machineId === socket.data.machineId
      || (
        actionOwners.get(message.actionId) === socket.data.machineId
        && (message.outcome === "stale" || message.outcome === "failed")
      )
    ) {
      publish({ type: "actionClosed", data: { actionId: message.actionId, outcome: message.outcome } });
    }
  };

  /**
   * A browser session authenticated by Cloudflare Access carries a signed assertion instead of a
   * shared token, so it never needs to be prompted. The bearer token stays for machine callers
   * (the hook posts decisions without a browser session) and as the fallback when Access is not
   * configured. Both are checked; either one is sufficient.
   */
  const authorized = async (request: Request) => {
    const token = bearerToken(request);
    if (token && secureEqual(token, options.actionToken)) return true;
    return Boolean(options.access && (await options.access(request)));
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

      if (url.pathname === "/decisions" && request.method === "GET") {
        try {
          await decisionWrite;
          return json({ records: await readDecisionRecords(decisionsPath) });
        } catch {
          return json({ error: "stored decisions are invalid" }, 500);
        }
      }

      if (url.pathname === "/push/vapid-public-key" && request.method === "GET") {
        return json({ publicKey: options.push?.publicKey ?? null });
      }

      if (url.pathname === "/push/subscriptions" && request.method === "POST") {
        if (!(await authorized(request))) return json({ error: "unauthorized" }, 401);
        let subscription: PushSubscription;
        try {
          subscription = parsePushSubscription(await request.json());
        } catch {
          return json({ error: "invalid subscription" }, 400);
        }
        try {
          await updatePushSubscriptions((subscriptions) => {
            subscriptions[subscription.endpoint] = subscription;
          });
        } catch {
          return json({ error: "failed to store subscription" }, 500);
        }
        return json({ subscription }, 201);
      }

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
        if (!(await authorized(request))) return json({ error: "unauthorized" }, 401);
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
        if (!decisionValidForAction(action, requestBody)) return json({ error: "invalid decision for action" }, 400);
        if (actions.get(actionId) !== action || reporters.get(action.sessionRef.machineId) !== reporter) {
          return json({ error: "action closed or Target offline" }, 410);
        }
        const decision = {
          actionId,
          verdict: requestBody.verdict,
          ...(requestBody.steer === undefined ? {} : { steer: requestBody.steer }),
          decidedAt: now().toISOString(),
        };
        let sentBytes = 0;
        try {
          sentBytes = reporter.send(JSON.stringify({ type: "decision", decision }));
        } catch {
          // The close callback owns offline marking; retain the action so a reconnect can retry it.
        }
        if (sentBytes <= 0) return json({ error: "action closed or Target offline" }, 410);
        actions.delete(actionId);
        publish({ type: "decision", data: decision });
        publish({
          type: "actionClosed",
          data: { actionId, outcome: decision.verdict === "deny" ? "denied" : "allowed" },
        });
        try {
          await appendDecision({
            id: crypto.randomUUID(),
            actionId,
            summary: action.summary,
            verdict: decision.verdict,
            ...(decision.steer === undefined ? {} : { steer: decision.steer }),
            machineId: action.sessionRef.machineId,
            decidedAt: decision.decidedAt,
          });
        } catch (error) {
          console.error("Session Board failed to persist decision", error);
        }
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
        await writeJson(layoutPath, layout);
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
