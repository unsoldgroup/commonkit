import { afterEach, describe, expect, test } from "bun:test";
import { mkdir, mkdtemp } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";

import { startHub, type HubServer } from "../src/server.js";

const machine = {
  id: "studio",
  name: "Studio Target",
  lastSeenAt: "2026-07-21T12:00:00.000Z",
  online: true,
} as const;

const session = {
  machineId: machine.id,
  worktreeId: "commonkit::/worktrees/uns-1302",
  repo: "commonkit",
  project: "Session Board",
  agent: "claude",
  paneKey: "pane-1",
  state: "waiting",
  stateSince: "2026-07-21T12:00:01.000Z",
  title: "Build board hub",
  lastOutputAt: "2026-07-21T12:00:02.000Z",
} as const;

const action = {
  id: "action-1",
  kind: "claude-permission",
  sessionRef: { machineId: machine.id, worktreeId: session.worktreeId, paneKey: session.paneKey },
  summary: "Allow Bash",
  detail: { tool: "Bash", input: { command: "bun test" } },
  createdAt: "2026-07-21T12:00:03.000Z",
  expiresAt: "2026-07-21T12:05:03.000Z",
} as const;

let hub: HubServer | undefined;
const reporterSockets = new Set<WebSocket>();
const socketTimeoutMs = 500;

afterEach(async () => {
  const currentHub = hub;
  hub = undefined;
  const sockets = [...reporterSockets];
  const stopping = currentHub?.stop();
  for (const socket of sockets) socket.close();
  await Promise.race([
    Promise.all([
      ...sockets.map((socket) => closed(socket)),
      ...(stopping ? [stopping] : []),
    ]),
    Bun.sleep(socketTimeoutMs),
  ]);
  reporterSockets.clear();
});

function openReporter(token = "studio-secret") {
  const socket = new WebSocket(`${hub!.url.replace("http", "ws")}/reporter`, {
    headers: { Authorization: `Bearer ${token}` },
  });
  return new Promise<WebSocket>((resolve, reject) => {
    let opened = false;
    const timeout = setTimeout(() => {
      socket.close();
      reject(new Error("reporter connection timed out"));
    }, socketTimeoutMs);
    socket.addEventListener("open", () => {
      opened = true;
      clearTimeout(timeout);
      reporterSockets.add(socket);
      resolve(socket);
    }, { once: true });
    socket.addEventListener("close", () => {
      reporterSockets.delete(socket);
      if (!opened) {
        clearTimeout(timeout);
        reject(new Error("reporter connection closed before opening"));
      }
    }, { once: true });
    socket.addEventListener("error", () => {
      if (!opened) {
        clearTimeout(timeout);
        reject(new Error("reporter connection failed"));
      }
    }, { once: true });
  });
}

function closed(socket: WebSocket) {
  if (socket.readyState === WebSocket.CLOSED) return Promise.resolve();
  return new Promise<void>((resolve) => {
    socket.addEventListener("close", () => resolve(), { once: true });
  });
}

function nextMessage(socket: WebSocket) {
  return new Promise<unknown>((resolve) => {
    socket.addEventListener("message", (event) => resolve(JSON.parse(String(event.data))), {
      once: true,
    });
  });
}

async function eventually(check: () => void | Promise<void>) {
  let error: unknown;
  for (let attempt = 0; attempt < 50; attempt += 1) {
    try {
      await check();
      return;
    } catch (caught) {
      error = caught;
      await Bun.sleep(10);
    }
  }
  throw error;
}

describe("board hub", () => {
  test("rejects reporter and decision requests without valid bearer tokens", async () => {
    hub = startHub({ reporterTokens: { studio: "studio-secret" }, actionToken: "board-secret" });

    const upgrade = await fetch(`${hub.url}/reporter`);
    expect(upgrade.status).toBe(401);

    const response = await fetch(`${hub.url}/actions/missing/decision`, {
      method: "POST",
      headers: { Authorization: "Bearer wrong", "Content-Type": "application/json" },
      body: JSON.stringify({ verdict: "allow" }),
    });
    expect(response.status).toBe(401);
  });

  test("merges reporter snapshots and deltas into normalized state", async () => {
    hub = startHub({ reporterTokens: { studio: "studio-secret" }, actionToken: "board-secret" });
    const reporter = await openReporter();
    reporter.send(JSON.stringify({
      type: "stateSnapshot",
      machine,
      sessions: [session],
      pendingActions: [action],
    }));

    await eventually(async () => {
      const state = await fetch(`${hub!.url}/state`).then((response) => response.json());
      expect(state).toEqual({ machines: [machine], sessions: [session], pendingActions: [action] });
    });

    const updated = { ...session, state: "working", title: "Running tests" } as const;
    reporter.send(JSON.stringify({
      type: "stateDelta",
      machine: { ...machine, lastSeenAt: "2026-07-21T12:00:04.000Z" },
      sessions: { upsert: [updated], remove: [] },
      pendingActions: { upsert: [], remove: [action.id] },
    }));

    await eventually(async () => {
      const state = await fetch(`${hub!.url}/state`).then((response) => response.json());
      expect(state.sessions).toEqual([updated]);
      expect(state.pendingActions).toEqual([]);
    });
    reporter.close();
  });

  test("replays SSE changes after Last-Event-ID", async () => {
    hub = startHub({ reporterTokens: { studio: "studio-secret" }, actionToken: "board-secret" });
    const reporter = await openReporter();
    reporter.send(JSON.stringify({ type: "stateSnapshot", machine, sessions: [], pendingActions: [] }));
    await eventually(() => expect(hub!.latestEventId).toBe(1));

    reporter.send(JSON.stringify({ type: "actionOpened", action }));
    await eventually(() => expect(hub!.latestEventId).toBe(2));

    const response = await fetch(`${hub.url}/events`, { headers: { "Last-Event-ID": "1" } });
    const reader = response.body!.getReader();
    const { value } = await reader.read();
    await reader.cancel();
    const replay = new TextDecoder().decode(value);
    expect(replay).toContain("id: 2");
    expect(replay).toContain('"type":"actionOpened"');
    reporter.close();
  });

  test("starts a headerless SSE stream with a race-free current snapshot", async () => {
    hub = startHub({ reporterTokens: { studio: "studio-secret" }, actionToken: "board-secret" });
    const reporter = await openReporter();
    reporter.send(JSON.stringify({ type: "stateSnapshot", machine, sessions: [session], pendingActions: [] }));
    await eventually(() => expect(hub!.latestEventId).toBe(1));

    const response = await fetch(`${hub.url}/events`);
    const reader = response.body!.getReader();
    const { value } = await reader.read();
    await reader.cancel();
    const initial = new TextDecoder().decode(value);
    expect(initial).toContain("id: 1");
    expect(initial).toContain('"type":"snapshot"');
    expect(initial).toContain('"paneKey":"pane-1"');
    reporter.close();
  });

  test("resets SSE clients with a snapshot when their replay position has expired", async () => {
    hub = startHub({
      reporterTokens: { studio: "studio-secret" },
      actionToken: "board-secret",
      eventBufferSize: 1,
    });
    const reporter = await openReporter();
    reporter.send(JSON.stringify({ type: "stateSnapshot", machine, sessions: [], pendingActions: [] }));
    await eventually(() => expect(hub!.latestEventId).toBe(1));
    reporter.send(JSON.stringify({ type: "actionOpened", action }));
    await eventually(() => expect(hub!.latestEventId).toBe(2));

    const response = await fetch(`${hub.url}/events`, { headers: { "Last-Event-ID": "0" } });
    const reader = response.body!.getReader();
    const { value } = await reader.read();
    await reader.cancel();
    const reset = new TextDecoder().decode(value);
    expect(reset).toContain("id: 2");
    expect(reset).toContain('"type":"snapshot"');
    expect(reset).toContain('"id":"action-1"');
    reporter.close();
  });

  test("routes a human decision to the reporter and closes the action", async () => {
    hub = startHub({ reporterTokens: { studio: "studio-secret" }, actionToken: "board-secret" });
    const reporter = await openReporter();
    reporter.send(JSON.stringify({ type: "actionOpened", action }));
    await eventually(() => expect(hub!.latestEventId).toBe(1));

    const received = nextMessage(reporter);
    const response = await fetch(`${hub.url}/actions/${action.id}/decision`, {
      method: "POST",
      headers: { Authorization: "Bearer board-secret", "Content-Type": "application/json" },
      body: JSON.stringify({ verdict: "allow" }),
    });

    expect(response.status).toBe(200);
    expect(await received).toEqual({
      type: "decision",
      decision: { actionId: action.id, verdict: "allow", decidedAt: expect.any(String) },
    });
    expect((await fetch(`${hub.url}/state`).then((item) => item.json())).pendingActions).toEqual([]);

    const repeated = await fetch(`${hub.url}/actions/${action.id}/decision`, {
      method: "POST",
      headers: { Authorization: "Bearer board-secret", "Content-Type": "application/json" },
      body: JSON.stringify({ verdict: "deny" }),
    });
    expect(repeated.status).toBe(410);
    reporter.close();
  });

  test("proxies a session tail request to its Target reporter", async () => {
    hub = startHub({ reporterTokens: { studio: "studio-secret" }, actionToken: "board-secret" });
    const reporter = await openReporter();
    reporter.send(JSON.stringify({ type: "stateSnapshot", machine, sessions: [session], pendingActions: [] }));
    await eventually(() => expect(hub!.latestEventId).toBe(1));

    const sessionId = encodeURIComponent(JSON.stringify([session.machineId, session.worktreeId, session.paneKey]));
    const request = fetch(`${hub.url}/sessions/${sessionId}/tail`);
    const message = await nextMessage(reporter) as { requestId: string };
    expect(message).toEqual({
      type: "tailRequest",
      requestId: expect.any(String),
      sessionRef: { machineId: session.machineId, worktreeId: session.worktreeId, paneKey: session.paneKey },
    });
    reporter.send(JSON.stringify({ type: "tailResponse", requestId: message.requestId, lines: ["running tests", "done"] }));

    const response = await request;
    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({ lines: ["running tests", "done"] });
    reporter.close();
  });

  test("does not let one Target take over another Target's action id", async () => {
    hub = startHub({
      reporterTokens: { studio: "studio-secret", laptop: "laptop-secret" },
      actionToken: "board-secret",
    });
    const studio = await openReporter();
    studio.send(JSON.stringify({ type: "actionOpened", action }));
    await eventually(() => expect(hub!.latestEventId).toBe(1));

    const laptop = await openReporter("laptop-secret");
    laptop.send(JSON.stringify({
      type: "actionOpened",
      action: { ...action, sessionRef: { ...action.sessionRef, machineId: "laptop" } },
    }));
    // Message handling is synchronous once Bun dispatches it; allow one event-loop turn without
    // coupling the ownership assertion to Bun's server-initiated WebSocket close handshake.
    await Bun.sleep(10);

    const state = await fetch(`${hub.url}/state`).then((response) => response.json());
    expect(state.pendingActions).toEqual([action]);
    studio.close();
  });

  test("marks a disconnected machine offline and refuses its pending actions", async () => {
    hub = startHub({ reporterTokens: { studio: "studio-secret" }, actionToken: "board-secret" });
    const reporter = await openReporter();
    reporter.send(JSON.stringify({
      type: "stateSnapshot",
      machine,
      sessions: [session],
      pendingActions: [action],
    }));
    await eventually(() => expect(hub!.latestEventId).toBe(1));
    reporter.close();

    await eventually(async () => {
      const state = await fetch(`${hub!.url}/state`).then((response) => response.json());
      expect(state.machines[0].online).toBe(false);
      expect(Date.parse(state.machines[0].lastSeenAt)).toBeGreaterThan(Date.parse(machine.lastSeenAt));
      expect(state.sessions).toEqual([session]);
    });

    const response = await fetch(`${hub.url}/actions/${action.id}/decision`, {
      method: "POST",
      headers: { Authorization: "Bearer board-secret", "Content-Type": "application/json" },
      body: JSON.stringify({ verdict: "deny" }),
    });
    expect(response.status).toBe(410);
  });

  test("serves PWA files without allowing traversal outside web/dist", async () => {
    const root = await mkdtemp(join(tmpdir(), "session-board-static-"));
    const webDistPath = join(root, "web", "dist");
    await mkdir(join(webDistPath, "assets"), { recursive: true });
    await Bun.write(join(webDistPath, "index.html"), "<main>Session Board</main>");
    await Bun.write(join(webDistPath, "assets", "app.js"), "export const ready = true;");
    await Bun.write(join(root, "secret.txt"), "not public");
    hub = startHub({
      reporterTokens: { studio: "studio-secret" },
      actionToken: "board-secret",
      webDistPath,
    });

    expect(await fetch(`${hub.url}/`).then((response) => response.text())).toContain("Session Board");
    expect(await fetch(`${hub.url}/assets/app.js`).then((response) => response.text())).toContain("ready");
    expect((await fetch(`${hub.url}/%2e%2e/%2e%2e/secret.txt`)).status).toBe(404);
  });
});
