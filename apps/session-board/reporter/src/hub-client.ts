import {
  hubToReporterMessageSchema,
  stateDeltaMessageSchema,
  stateSnapshotMessageSchema,
  type Decision,
  type Machine,
  type PendingAction,
  type ReporterToHubMessage,
  type Session,
} from "@commonkit/session-board-protocol";

export interface HubClientOptions {
  url: string;
  machineToken: string;
  machineId: string;
  machineName: string;
  reconnectMinMs?: number;
  reconnectMaxMs?: number;
  now?: () => Date;
  onDecision: (decision: Decision) => void | Promise<void>;
  webSocketFactory?: (url: string, options?: { headers: Record<string, string> }) => WebSocket;
}

function key(session: Pick<Session, "machineId" | "worktreeId" | "paneKey">) {
  return JSON.stringify([session.machineId, session.worktreeId, session.paneKey]);
}

export class HubClient {
  #sessions = new Map<string, Session>();
  #actions = new Map<string, PendingAction>();
  #socket?: WebSocket;
  #stopped = false;
  #attempt = 0;
  #timer?: ReturnType<typeof setTimeout>;
  constructor(private readonly options: HubClientOptions) {}

  get actions(): ReadonlyMap<string, PendingAction> { return this.#actions; }

  setSessions(sessions: Session[]) {
    const previous = this.#sessions;
    this.#sessions = new Map(sessions.map((session) => [key(session), session]));
    if (!this.#connected()) return;
    const upsert = sessions.filter((session) => JSON.stringify(previous.get(key(session))) !== JSON.stringify(session));
    const remove = [...previous.values()].filter((session) => !this.#sessions.has(key(session))).map(({ machineId, worktreeId, paneKey }) => ({ machineId, worktreeId, paneKey }));
    if (upsert.length || remove.length) this.#sendDelta(upsert, remove, [], []);
  }

  setActions(actions: PendingAction[]) {
    const previous = this.#actions;
    this.#actions = new Map(actions.map((action) => [action.id, action]));
    if (!this.#connected()) return;
    for (const action of actions) if (JSON.stringify(previous.get(action.id)) !== JSON.stringify(action)) this.#send({ type: "actionOpened", action });
    for (const id of previous.keys()) if (!this.#actions.has(id)) this.#send({ type: "actionClosed", actionId: id });
  }

  start() { this.#connect(); }
  stop() { this.#stopped = true; if (this.#timer) clearTimeout(this.#timer); this.#socket?.close(); }

  #machine(): Machine {
    return { id: this.options.machineId, name: this.options.machineName, lastSeenAt: (this.options.now ?? (() => new Date()))().toISOString(), online: true };
  }
  #connected() { return this.#socket?.readyState === WebSocket.OPEN; }
  #send(message: ReporterToHubMessage) { this.#socket?.send(JSON.stringify(message)); }
  #sendSnapshot() {
    this.#send(stateSnapshotMessageSchema.parse({ type: "stateSnapshot", machine: this.#machine(), sessions: [...this.#sessions.values()], pendingActions: [...this.#actions.values()] }));
  }
  #sendDelta(upsert: Session[], remove: Array<Pick<Session, "machineId" | "worktreeId" | "paneKey">>, actionUpsert: PendingAction[], actionRemove: string[]) {
    this.#send(stateDeltaMessageSchema.parse({ type: "stateDelta", machine: this.#machine(), sessions: { upsert, remove }, pendingActions: { upsert: actionUpsert, remove: actionRemove } }));
  }
  #connect() {
    if (this.#stopped) return;
    const url = new URL(this.options.url);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    url.pathname = `${url.pathname.replace(/\/$/, "")}/reporter`;
    const createSocket = this.options.webSocketFactory ?? ((address, options) => {
      const Constructor = WebSocket as unknown as new (url: string, options?: { headers: Record<string, string> }) => WebSocket;
      return new Constructor(address, options);
    });
    const socket = createSocket(url.toString(), { headers: { Authorization: `Bearer ${this.options.machineToken}` } });
    this.#socket = socket;
    socket.addEventListener("open", () => { this.#attempt = 0; this.#send({ type: "hello", machineToken: this.options.machineToken }); this.#sendSnapshot(); });
    socket.addEventListener("message", async (event) => {
      try { await this.options.onDecision(hubToReporterMessageSchema.parse(JSON.parse(String(event.data))).decision); }
      catch (error) { console.error("Hub decision failed", error); }
    });
    socket.addEventListener("close", () => {
      if (this.#stopped || socket !== this.#socket) return;
      const min = this.options.reconnectMinMs ?? 500;
      const max = this.options.reconnectMaxMs ?? 30_000;
      const delay = Math.min(max, min * 2 ** this.#attempt++);
      this.#timer = setTimeout(() => this.#connect(), delay * (0.8 + Math.random() * 0.4));
    });
  }
}
