import { afterEach, describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, readFile } from "node:fs/promises";
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

  test("persists decisions and prunes records older than seven days", async () => {
    const dataDir = await mkdtemp(join(tmpdir(), "session-board-data-"));
    await Bun.write(join(dataDir, "decisions.json"), JSON.stringify({
      records: [
        {
          id: "old-record",
          actionId: "old-action",
          summary: "Old decision",
          verdict: "deny",
          machineId: machine.id,
          decidedAt: "2026-07-20T11:59:59.000Z",
        },
      ],
    }));
    hub = startHub({
      reporterTokens: { studio: "studio-secret" },
      actionToken: "board-secret",
      dataDir,
      now: () => new Date("2026-07-27T12:00:00.000Z"),
    });
    const reporter = await openReporter();
    reporter.send(JSON.stringify({ type: "actionOpened", action }));
    await eventually(() => expect(hub!.latestEventId).toBe(1));

    const response = await fetch(`${hub.url}/actions/${action.id}/decision`, {
      method: "POST",
      headers: { Authorization: "Bearer board-secret", "Content-Type": "application/json" },
      body: JSON.stringify({ verdict: "deny", steer: "Use the safer path." }),
    });

    expect(response.status).toBe(200);
    const decisions = await fetch(`${hub.url}/decisions`).then((item) => item.json());
    expect(decisions.records).toEqual([
      {
        id: expect.any(String),
        actionId: action.id,
        summary: action.summary,
        verdict: "deny",
        steer: "Use the safer path.",
        machineId: machine.id,
        decidedAt: "2026-07-27T12:00:00.000Z",
      },
    ]);
    expect(JSON.parse(await readFile(join(dataDir, "decisions.json"), "utf8"))).toEqual(decisions);
    reporter.close();
  });

  test("broadcasts the outcome when the board applies a decision", async () => {
    hub = startHub({ reporterTokens: { studio: "studio-secret" }, actionToken: "board-secret" });
    const reporter = await openReporter();
    reporter.send(JSON.stringify({ type: "actionOpened", action }));
    await eventually(() => expect(hub!.latestEventId).toBe(1));

    const response = await fetch(`${hub.url}/actions/${action.id}/decision`, {
      method: "POST",
      headers: { Authorization: "Bearer board-secret", "Content-Type": "application/json" },
      body: JSON.stringify({ verdict: "allow" }),
    });
    expect(response.status).toBe(200);

    const events = await fetch(`${hub.url}/events`, { headers: { "Last-Event-ID": "1" } });
    const reader = events.body!.getReader();
    const decoder = new TextDecoder();
    let replay = "";
    while (!replay.includes('"type":"actionClosed"')) {
      const { value, done } = await reader.read();
      if (done) break;
      replay += decoder.decode(value, { stream: true });
    }
    await reader.cancel();
    expect(replay).toContain('"type":"decision"');
    expect(replay).toContain(`"type":"actionClosed","data":{"actionId":"${action.id}","outcome":"allowed"}`);
    reporter.close();
  });

  test("broadcasts stale or failed reporter outcomes after state removal", async () => {
    hub = startHub({ reporterTokens: { studio: "studio-secret" }, actionToken: "board-secret" });
    const reporter = await openReporter();
    reporter.send(JSON.stringify({ type: "actionOpened", action }));
    await eventually(() => expect(hub!.latestEventId).toBe(1));
    reporter.send(JSON.stringify({
      type: "stateDelta",
      machine,
      sessions: { upsert: [], remove: [] },
      pendingActions: { upsert: [], remove: [action.id] },
    }));
    await eventually(() => expect(hub!.latestEventId).toBe(2));
    reporter.send(JSON.stringify({ type: "actionClosed", actionId: action.id, outcome: "failed" }));
    await eventually(() => expect(hub!.latestEventId).toBe(3));

    const events = await fetch(`${hub.url}/events`, { headers: { "Last-Event-ID": "2" } });
    const reader = events.body!.getReader();
    const { value } = await reader.read();
    await reader.cancel();
    expect(new TextDecoder().decode(value))
      .toContain(`"type":"actionClosed","data":{"actionId":"${action.id}","outcome":"failed"}`);
    reporter.close();
  });

  test("rejects always and steer for non-Claude actions", async () => {
    hub = startHub({ reporterTokens: { studio: "studio-secret" }, actionToken: "board-secret" });
    const reporter = await openReporter();
    const codexAction = {
      ...action,
      kind: "codex-prompt",
      detail: { prompt: "Choose", options: ["Continue"] },
    } as const;
    reporter.send(JSON.stringify({ type: "actionOpened", action: codexAction }));
    await eventually(() => expect(hub!.latestEventId).toBe(1));

    for (const body of [{ verdict: "always" }, { verdict: "option:1", steer: "Try another way." }]) {
      const response = await fetch(`${hub.url}/actions/${action.id}/decision`, {
        method: "POST",
        headers: { Authorization: "Bearer board-secret", "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });
      expect(response.status).toBe(400);
    }
    expect((await fetch(`${hub.url}/state`).then((item) => item.json())).pendingActions).toEqual([codexAction]);
    reporter.close();
  });

  test("stores push subscriptions by endpoint and serves the VAPID public key", async () => {
    const dataDir = await mkdtemp(join(tmpdir(), "session-board-push-"));
    hub = startHub({
      reporterTokens: { studio: "studio-secret" },
      actionToken: "board-secret",
      dataDir,
      push: { publicKey: "public-key", sender: { async send() {} } },
    });
    const subscription = {
      endpoint: "https://push.example/device-1",
      expirationTime: null,
      keys: { p256dh: "device-key", auth: "device-auth" },
    };

    expect(await fetch(`${hub.url}/push/vapid-public-key`).then((response) => response.json()))
      .toEqual({ publicKey: "public-key" });
    expect((await fetch(`${hub.url}/push/subscriptions`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(subscription),
    })).status).toBe(401);
    expect((await fetch(`${hub.url}/push/subscriptions`, {
      method: "POST",
      headers: { Authorization: "Bearer board-secret", "Content-Type": "application/json" },
      body: JSON.stringify(subscription),
    })).status).toBe(201);
    const updated = { ...subscription, keys: { ...subscription.keys, auth: "updated-auth" } };
    expect((await fetch(`${hub.url}/push/subscriptions`, {
      method: "POST",
      headers: { Authorization: "Bearer board-secret", "Content-Type": "application/json" },
      body: JSON.stringify(updated),
    })).status).toBe(201);
    expect(JSON.parse(await readFile(join(dataDir, "push-subscriptions.json"), "utf8"))).toEqual({
      [subscription.endpoint]: updated,
    });
  });

  test("pushes each newly-seen pending action once across reporter reconnect state", async () => {
    const dataDir = await mkdtemp(join(tmpdir(), "session-board-push-"));
    const sent: Array<{ endpoint: string; payload: string }> = [];
    hub = startHub({
      reporterTokens: { studio: "studio-secret" },
      actionToken: "board-secret",
      dataDir,
      push: {
        publicKey: "public-key",
        sender: {
          async send(subscription, payload) {
            sent.push({ endpoint: subscription.endpoint, payload });
          },
        },
      },
    });
    const subscription = {
      endpoint: "https://push.example/device-1",
      keys: { p256dh: "device-key", auth: "device-auth" },
    };
    await fetch(`${hub.url}/push/subscriptions`, {
      method: "POST",
      headers: { Authorization: "Bearer board-secret", "Content-Type": "application/json" },
      body: JSON.stringify(subscription),
    });
    const reporter = await openReporter();

    reporter.send(JSON.stringify({ type: "stateSnapshot", machine, sessions: [], pendingActions: [action] }));
    await eventually(() => expect(sent).toHaveLength(1));
    reporter.send(JSON.stringify({ type: "stateSnapshot", machine, sessions: [], pendingActions: [action] }));
    await Bun.sleep(20);
    expect(sent).toHaveLength(1);

    const secondAction = { ...action, id: "action-2", summary: "Allow Edit" };
    reporter.send(JSON.stringify({
      type: "stateDelta",
      machine,
      sessions: { upsert: [], remove: [] },
      pendingActions: { upsert: [secondAction], remove: [] },
    }));
    await eventually(() => expect(sent).toHaveLength(2));
    expect(sent.map((item) => JSON.parse(item.payload))).toEqual([
      {
        title: "Session Board approval needed",
        body: action.summary,
        actionId: action.id,
      },
      {
        title: "Session Board approval needed",
        body: secondAction.summary,
        actionId: secondAction.id,
      },
    ]);
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

  test("accepts a decision from a verified Cloudflare Access identity without a bearer token", async () => {
    hub = startHub({
      reporterTokens: { studio: "studio-secret" },
      actionToken: "board-secret",
      access: async (request) =>
        request.headers.get("Cf-Access-Jwt-Assertion") === "valid-assertion"
          ? "al@unsold.group"
          : undefined,
    });
    const reporter = await openReporter();
    reporter.send(JSON.stringify({ type: "stateSnapshot", machine, sessions: [], pendingActions: [] }));
    reporter.send(JSON.stringify({ type: "actionOpened", action }));
    await eventually(() => expect(hub!.latestEventId).toBe(2));

    const response = await fetch(`${hub.url}/actions/${action.id}/decision`, {
      method: "POST",
      headers: { "Cf-Access-Jwt-Assertion": "valid-assertion", "Content-Type": "application/json" },
      body: JSON.stringify({ verdict: "allow" }),
    });

    expect(response.status).toBe(200);
    reporter.close();
  });

  test("rejects an unverified Access assertion, so a tailnet peer cannot forge the header", async () => {
    hub = startHub({
      reporterTokens: { studio: "studio-secret" },
      actionToken: "board-secret",
      access: async (request) =>
        request.headers.get("Cf-Access-Jwt-Assertion") === "valid-assertion"
          ? "al@unsold.group"
          : undefined,
    });

    const forged = await fetch(`${hub.url}/actions/action-1/decision`, {
      method: "POST",
      headers: { "Cf-Access-Jwt-Assertion": "forged", "Content-Type": "application/json" },
      body: JSON.stringify({ verdict: "allow" }),
    });
    const absent = await fetch(`${hub.url}/actions/action-1/decision`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ verdict: "allow" }),
    });

    expect(forged.status).toBe(401);
    expect(absent.status).toBe(401);
  });

  test("keeps the bearer token path working when Access is not configured", async () => {
    hub = startHub({ reporterTokens: { studio: "studio-secret" }, actionToken: "board-secret" });
    const reporter = await openReporter();
    reporter.send(JSON.stringify({ type: "stateSnapshot", machine, sessions: [], pendingActions: [] }));
    reporter.send(JSON.stringify({ type: "actionOpened", action }));
    await eventually(() => expect(hub!.latestEventId).toBe(2));

    const accepted = await fetch(`${hub.url}/actions/${action.id}/decision`, {
      method: "POST",
      headers: { Authorization: "Bearer board-secret", "Content-Type": "application/json" },
      body: JSON.stringify({ verdict: "allow" }),
    });
    const rejected = await fetch(`${hub.url}/actions/${action.id}/decision`, {
      method: "POST",
      headers: { Authorization: "Bearer wrong", "Content-Type": "application/json" },
      body: JSON.stringify({ verdict: "allow" }),
    });

    expect(accepted.status).toBe(200);
    expect(rejected.status).toBe(401);
    reporter.close();
  });
});
