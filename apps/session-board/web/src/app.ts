import type { BoardSnapshot, DecisionRecord, PendingAction, Session, Verdict } from "@commonkit/session-board-protocol";
import { actionPayload, decisionsNewestFirst, groupBoard, moveSession, outcomeText, pendingFor, pendingOldestFirst, sessionId, type BoardGroup, type Layout } from "./model.js";

const empty: BoardSnapshot = { machines: [], sessions: [], pendingActions: [] };
let snapshot = empty;
let layout: Layout = { groups: [] };
let groups: BoardGroup[] = [];
let updatedAt = 0;
let decisions: DecisionRecord[] = [];
let activeTab: "queue" | "decisions" = "queue";
const expanded = new Set<string>();
const confirmingAlways = new Set<string>();
const denying = new Set<string>();
const tails = new Map<string, string | null>();
const knownActions = new Map<string, PendingAction>();
const closed = new Map<string, { outcome: "allowed" | "denied" | "stale" | "failed"; until: number }>();
const $ = <T extends Element>(selector: string) => document.querySelector<T>(selector)!;
const groupRoot = $("#groups");
const queueRoot = $("#queue");
const drawer = $("#drawer") as HTMLDialogElement;
const token = () => localStorage.getItem("session-board-token") ?? "";

function escape(value: string) { const node = document.createElement("span"); node.textContent = value; return node.innerHTML.replaceAll('"', "&quot;").replaceAll("'", "&#39;"); }
function elapsed(iso: string) { const seconds = Math.max(0, Math.floor((Date.now() - Date.parse(iso)) / 1000)); return seconds < 60 ? `${seconds}s` : seconds < 3600 ? `${Math.floor(seconds / 60)}m` : `${Math.floor(seconds / 3600)}h ${Math.floor(seconds % 3600 / 60)}m`; }
function outcome(action: PendingAction) {
  const value = closed.get(action.id);
  return value ? `<div class="outcome ${value.outcome}">${escape(outcomeText(value.outcome))}</div>` : "";
}
function claudeActions(action: PendingAction) {
  if (closed.has(action.id)) return outcome(action);
  if (action.kind !== "claude-permission") return "";
  if (confirmingAlways.has(action.id)) return `<div class="confirm-rule"><span>Always allow this project-local rule:</span><code>${escape(action.detail.ruleSuggestion!)}</code><div class="actions"><button class="allow" data-action="${escape(action.id)}" data-verdict="always">Confirm Always</button><button data-cancel="${escape(action.id)}">Cancel</button></div></div>`;
  if (denying.has(action.id)) return `<div class="deny-panel"><span>Optional steer</span><div class="steer-chips">${["wrong approach", "not now", "ask me in terminal"].map((value) => `<button data-action="${escape(action.id)}" data-verdict="deny" data-steer="${escape(value)}">${escape(value)}</button>`).join("")}</div><div class="steer-free"><input maxlength="280" data-steer-input="${escape(action.id)}" placeholder="Tell the agent what to do instead"><button class="deny" data-deny-send="${escape(action.id)}">Deny</button></div><button class="text-button" data-cancel="${escape(action.id)}">Cancel</button></div>`;
  return `<div class="actions"><button class="allow" data-action="${escape(action.id)}" data-verdict="allow">Allow</button>${action.detail.ruleSuggestion ? `<button data-always="${escape(action.id)}">Always</button>` : ""}<button class="deny" data-deny="${escape(action.id)}">Deny</button></div>`;
}
function actionButtons(action: PendingAction) {
  if (action.kind === "claude-permission") return claudeActions(action);
  if (closed.has(action.id)) return outcome(action);
  return `<div class="actions">${action.detail.options.map((label, index) => `<button class="${index === 0 ? "allow" : ""}" data-action="${escape(action.id)}" data-verdict="option:${index + 1}">${escape(label)}</button>`).join("")}</div>`;
}
function actionDetail(action: PendingAction, full = false) {
  if (action.kind !== "claude-permission") return "";
  const payload = actionPayload(action);
  const clip = (value: string, max = 600) => value.length > max ? `${value.slice(0, max - 1)}…` : value;
  const intent = action.detail.intent ? `<p class="intent${full ? " full" : ""}">${escape(action.detail.intent)}</p>` : "";
  return `${intent}${payload ? `<pre class="detail${full ? " full" : ""}">${escape(full ? payload : clip(payload))}</pre>` : ""}`;
}
function sessionAction(session: Session) {
  return pendingFor(session, snapshot.pendingActions)[0] ?? [...closed.keys()].map((id) => knownActions.get(id)).find((action) => action && sessionId(action.sessionRef) === sessionId(session));
}
function expandedTail(id: string) {
  if (!tails.has(id)) return `<pre class="tail inline">Loading terminal tail…</pre>`;
  const tail = tails.get(id);
  return tail ? `<pre class="tail inline">${escape(tail)}</pre>` : "";
}
function card(session: Session) {
  const id = sessionId(session);
  const action = sessionAction(session);
  const isExpanded = expanded.has(id);
  return `<article class="card${isExpanded ? " expanded" : ""}" data-session='${escape(id)}' tabindex="0"><div class="card-top"><div><div class="repo">${escape(session.repo)} · ${escape(session.agent)}</div><div class="agent">${escape(session.title || session.paneKey)}</div></div><span class="badge ${session.state}">${escape(session.state)}</span></div>${action ? `<p class="summary">${escape(action.summary)}</p>${actionDetail(action, isExpanded)}${isExpanded ? expandedTail(id) : ""}${actionButtons(action)}` : ""}<div class="elapsed" data-since="${session.stateSince}">${elapsed(session.stateSince)} in state</div></article>`;
}
function decisionsView() {
  return decisions.length ? decisionsNewestFirst(decisions).map((record) => `<div class="decision-item"><div><strong>${escape(record.summary)}</strong><span class="verdict ${record.verdict}">${escape(record.verdict)}</span></div><time>${new Date(record.decidedAt).toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" })}</time>${record.steer ? `<p>${escape(record.steer)}</p>` : ""}</div>`).join("") : `<p class="empty">No recent decisions.</p>`;
}
function render() {
  for (const action of snapshot.pendingActions) knownActions.set(action.id, action);
  for (const [id, value] of closed) if (value.until <= Date.now()) { closed.delete(id); knownActions.delete(id); }
  groups = groupBoard(snapshot, layout);
  groupRoot.innerHTML = groups.length ? groups.map((group) => `<section class="group" data-group="${escape(group.id)}"><div class="group-head"><h2>${escape(group.name)}</h2><span class="count">${group.sessions.length}</span></div>${group.sessions.map(card).join("")}</section>`).join("") : `<p class="empty">No active sessions.</p>`;
  const pending = pendingOldestFirst(snapshot);
  $("#queue-count").textContent = String(activeTab === "queue" ? pending.length : decisions.length);
  queueRoot.innerHTML = activeTab === "queue"
    ? pending.length ? pending.map((action) => `<div class="queue-item"><strong>${escape(action.summary)}</strong>${actionDetail(action)}<time>${new Date(action.createdAt).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</time>${actionButtons(action)}</div>`).join("") : `<p class="empty">No pending actions.</p>`
    : decisionsView();
  document.querySelectorAll<HTMLElement>("[data-tab]").forEach((tab) => tab.classList.toggle("active", tab.dataset.tab === activeTab));
  bindDrag();
}
async function loadState() { const response = await fetch("/state", { cache: "no-store" }); if (!response.ok) throw new Error("State unavailable"); snapshot = await response.json(); updatedAt = Date.now(); render(); }
async function loadDecisions() { const response = await fetch("/decisions", { cache: "no-store" }); if (!response.ok) throw new Error("Decisions unavailable"); decisions = (await response.json()).records; render(); }
async function decide(id: string, verdict: Verdict, steer?: string) {
  let auth = token();
  if (!auth) { auth = prompt("Board action token") ?? ""; if (auth) localStorage.setItem("session-board-token", auth); }
  if (!auth) return;
  const response = await fetch(`/actions/${encodeURIComponent(id)}/decision`, { method: "POST", headers: { Authorization: `Bearer ${auth}`, "Content-Type": "application/json" }, body: JSON.stringify({ verdict, ...(steer ? { steer } : {}) }) });
  if (!response.ok) throw new Error((await response.json()).error ?? "Decision failed");
}
async function loadTail(session: Session) {
  const id = sessionId(session);
  if (tails.has(id)) return;
  try {
    const response = await fetch(`/sessions/${encodeURIComponent(id)}/tail`);
    if (!response.ok) { tails.set(id, null); return; }
    const body = await response.json();
    tails.set(id, Array.isArray(body.lines) && body.lines.length ? body.lines.join("\n") : null);
  } catch {
    tails.set(id, null);
  } finally {
    render();
  }
}
async function openTail(session: Session) {
  drawer.showModal(); $("#drawer-content").innerHTML = `<h2>${escape(session.repo)} · ${escape(session.agent)}</h2><p>${escape(session.title)}</p><pre class="tail">Loading terminal tail…</pre>`;
  const response = await fetch(`/sessions/${encodeURIComponent(sessionId(session))}/tail`);
  const body = await response.json(); $(".tail").textContent = response.ok ? body.lines.join("\n") || "No recent output." : body.error;
}
function toast(message: string) { const item = $("#toast"); item.textContent = message; item.classList.add("show"); setTimeout(() => item.classList.remove("show"), 2200); }
function runDecision(id: string, verdict: Verdict, steer?: string) { void decide(id, verdict, steer).catch((error) => toast(error.message)); }

document.addEventListener("click", (event) => {
  const target = event.target as Element;
  const button = target.closest<HTMLButtonElement>("button");
  if (button?.dataset.action) { event.stopPropagation(); runDecision(button.dataset.action, button.dataset.verdict as Verdict, button.dataset.steer); return; }
  if (button?.dataset.always) { event.stopPropagation(); confirmingAlways.add(button.dataset.always); render(); return; }
  if (button?.dataset.deny) { event.stopPropagation(); denying.add(button.dataset.deny); render(); return; }
  if (button?.dataset.denySend) {
    event.stopPropagation();
    const input = button.closest(".deny-panel")?.querySelector<HTMLInputElement>("[data-steer-input]");
    runDecision(button.dataset.denySend, "deny", input?.value.trim() || undefined);
    return;
  }
  if (button?.dataset.cancel) { event.stopPropagation(); confirmingAlways.delete(button.dataset.cancel); denying.delete(button.dataset.cancel); render(); return; }
  if (button?.dataset.tab) { activeTab = button.dataset.tab as typeof activeTab; if (activeTab === "decisions") void loadDecisions().catch((error) => toast(error.message)); else render(); return; }
  const item = target.closest<HTMLElement>(".card");
  if (item && !target.closest("input")) {
    const session = snapshot.sessions.find((candidate) => sessionId(candidate) === item.dataset.session);
    if (!session) return;
    const action = sessionAction(session);
    if (action?.kind !== "claude-permission") { void openTail(session).catch((error) => toast(error.message)); return; }
    if (expanded.delete(item.dataset.session!)) render();
    else { expanded.add(item.dataset.session!); render(); void loadTail(session); }
  }
});
$(".close").addEventListener("click", () => drawer.close());
$("#new-group").addEventListener("click", () => {
  const name = prompt("Group name")?.trim();
  if (!name) return;
  layout = { groups: [...layout.groups, { id: `custom:${crypto.randomUUID()}`, name, sessionIds: [] }] };
  render();
  void fetch("/layout", { method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify(layout) }).catch(() => toast("Could not save layout"));
});

function bindDrag() {
  for (const item of document.querySelectorAll<HTMLElement>(".card")) {
    let moved = false;
    item.onpointerdown = (event) => { if ((event.target as Element).closest("button,input")) return; moved = false; item.setPointerCapture(event.pointerId); item.classList.add("dragging"); };
    item.onpointermove = (event) => { if (!item.hasPointerCapture(event.pointerId)) return; moved = true; document.querySelectorAll(".group").forEach((group) => group.classList.remove("over")); document.elementFromPoint(event.clientX, event.clientY)?.closest(".group")?.classList.add("over"); };
    item.onpointerup = (event) => { item.classList.remove("dragging"); const target = document.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement>(".group"); document.querySelectorAll(".group").forEach((group) => group.classList.remove("over")); if (!moved || !target) return; event.stopPropagation(); layout = moveSession(layout, groups, item.dataset.session!, target.dataset.group!); render(); void fetch("/layout", { method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify(layout) }).catch(() => toast("Could not save layout")); };
  }
}

const events = new EventSource("/events");
events.addEventListener("snapshot", (event) => { snapshot = JSON.parse((event as MessageEvent).data).data; updatedAt = Date.now(); render(); });
for (const type of ["machine", "session", "actionOpened"]) events.addEventListener(type, () => void loadState().catch(() => {}));
events.addEventListener("actionClosed", (event) => {
  const data = JSON.parse((event as MessageEvent).data).data as { actionId: string; outcome?: "allowed" | "denied" | "stale" | "failed" };
  if (data.outcome) closed.set(data.actionId, { outcome: data.outcome, until: Date.now() + 30_000 });
  void loadState().catch(() => render());
});
events.addEventListener("decision", () => { if (activeTab === "decisions") void loadDecisions().catch(() => {}); });
document.addEventListener("visibilitychange", () => { if (document.visibilityState === "visible") { void loadState().catch(() => {}); void wake(); } });
setInterval(() => { $("#updated").textContent = updatedAt ? `Updated ${elapsed(new Date(updatedAt).toISOString())} ago` : "Connecting…"; ($("#stale") as HTMLElement).hidden = !updatedAt || Date.now() - updatedAt <= 10_000; document.querySelectorAll<HTMLElement>("[data-since]").forEach((node) => node.textContent = `${elapsed(node.dataset.since!)} in state`); if ([...closed.values()].some((value) => value.until <= Date.now())) render(); }, 1_000);
async function wake() { try { if ("wakeLock" in navigator) await navigator.wakeLock.request("screen"); } catch {} }

void Promise.all([loadState(), fetch("/layout").then((response) => response.json()).then((value) => { layout = value; render(); })]).catch((error) => toast(error.message));
void wake(); if ("serviceWorker" in navigator) void navigator.serviceWorker.register("/sw.js");
