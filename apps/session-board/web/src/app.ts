import type { BoardSnapshot, PendingAction, Session, Verdict } from "@commonkit/session-board-protocol";
import { groupBoard, moveSession, pendingFor, pendingOldestFirst, sessionId, type BoardGroup, type Layout } from "./model.js";

const empty: BoardSnapshot = { machines: [], sessions: [], pendingActions: [] };
let snapshot = empty;
let layout: Layout = { groups: [] };
let groups: BoardGroup[] = [];
let updatedAt = 0;
const $ = <T extends Element>(selector: string) => document.querySelector<T>(selector)!;
const groupRoot = $("#groups");
const queueRoot = $("#queue");
const drawer = $("#drawer") as HTMLDialogElement;
const token = () => localStorage.getItem("session-board-token") ?? "";

function escape(value: string) { const node = document.createElement("span"); node.textContent = value; return node.innerHTML.replaceAll('"', "&quot;").replaceAll("'", "&#39;"); }
function elapsed(iso: string) { const seconds = Math.max(0, Math.floor((Date.now() - Date.parse(iso)) / 1000)); return seconds < 60 ? `${seconds}s` : seconds < 3600 ? `${Math.floor(seconds / 60)}m` : `${Math.floor(seconds / 3600)}h ${Math.floor(seconds % 3600 / 60)}m`; }
function actionButtons(action: PendingAction) {
  const options: Array<[string, Verdict, string]> = action.kind === "claude-permission"
    ? [["Approve", "allow", "allow"], ["Decline", "deny", "deny"]]
    : action.detail.options.map((label, index) => [label, `option:${index + 1}`, index === 0 ? "allow" : ""]);
  return `<div class="actions">${options.map(([label, verdict, style]) => `<button class="${style}" data-action="${escape(action.id)}" data-verdict="${verdict}">${escape(label!)}</button>`).join("")}</div>`;
}
function actionDetail(action: PendingAction) {
  if (action.kind !== "claude-permission") return "";
  const input = (action.detail.input && typeof action.detail.input === "object" ? action.detail.input : {}) as Record<string, unknown>;
  const str = (key: string) => (typeof input[key] === "string" && input[key] ? (input[key] as string) : undefined);
  const clip = (value: string, max = 600) => (value.length > max ? `${value.slice(0, max - 1)}…` : value);
  const tool = action.detail.tool;
  const body =
    tool === "Bash" ? str("command") :
    tool === "Write" ? str("content") :
    tool === "Edit" ? (str("old_string") || str("new_string")
      ? `${(str("old_string") ?? "").split("\n").map((line) => `- ${line}`).join("\n")}\n${(str("new_string") ?? "").split("\n").map((line) => `+ ${line}`).join("\n")}`
      : undefined) :
    tool === "NotebookEdit" ? str("new_source") :
    tool === "WebFetch" ? [str("url"), str("prompt")].filter(Boolean).join("\n") :
    tool === "WebSearch" ? str("query") :
    tool === "Task" ? str("prompt") :
    str("command") ?? str("content") ?? str("prompt");
  const intent = typeof action.detail.intent === "string" && action.detail.intent ? `<p class="intent">${escape(action.detail.intent)}</p>` : "";
  return `${intent}${body ? `<pre class="detail">${escape(clip(body))}</pre>` : ""}`;
}
function card(session: Session) {
  const action = pendingFor(session, snapshot.pendingActions)[0];
  return `<article class="card" data-session='${escape(sessionId(session))}' tabindex="0"><div class="card-top"><div><div class="repo">${escape(session.repo)} · ${escape(session.agent)}</div><div class="agent">${escape(session.title || session.paneKey)}</div></div><span class="badge ${session.state}">${escape(session.state)}</span></div>${action ? `<p class="summary">${escape(action.summary)}</p>${actionDetail(action)}${actionButtons(action)}` : ""}<div class="elapsed" data-since="${session.stateSince}">${elapsed(session.stateSince)} in state</div></article>`;
}
function render() {
  groups = groupBoard(snapshot, layout);
  groupRoot.innerHTML = groups.length ? groups.map((group) => `<section class="group" data-group="${escape(group.id)}"><div class="group-head"><h2>${escape(group.name)}</h2><span class="count">${group.sessions.length}</span></div>${group.sessions.map(card).join("")}</section>`).join("") : `<p class="empty">No active sessions.</p>`;
  const pending = pendingOldestFirst(snapshot);
  $("#queue-count").textContent = String(pending.length);
  queueRoot.innerHTML = pending.length ? pending.map((action) => `<div class="queue-item"><strong>${escape(action.summary)}</strong>${actionDetail(action)}<time>${new Date(action.createdAt).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</time>${actionButtons(action)}</div>`).join("") : `<p class="empty">No pending actions.</p>`;
  bindDrag();
}
async function loadState() { const response = await fetch("/state", { cache: "no-store" }); if (!response.ok) throw new Error("State unavailable"); snapshot = await response.json(); updatedAt = Date.now(); render(); }
async function decide(id: string, verdict: Verdict) {
  let auth = token();
  if (!auth) { auth = prompt("Board action token") ?? ""; if (auth) localStorage.setItem("session-board-token", auth); }
  if (!auth) return;
  const response = await fetch(`/actions/${encodeURIComponent(id)}/decision`, { method: "POST", headers: { Authorization: `Bearer ${auth}`, "Content-Type": "application/json" }, body: JSON.stringify({ verdict }) });
  if (!response.ok) throw new Error((await response.json()).error ?? "Decision failed");
  await loadState();
}
async function openTail(session: Session) {
  drawer.showModal(); $("#drawer-content").innerHTML = `<h2>${escape(session.repo)} · ${escape(session.agent)}</h2><p>${escape(session.title)}</p><pre class="tail">Loading terminal tail…</pre>`;
  const response = await fetch(`/sessions/${encodeURIComponent(sessionId(session))}/tail`);
  const body = await response.json(); $(".tail").textContent = response.ok ? body.lines.join("\n") || "No recent output." : body.error;
}
function toast(message: string) { const item = $("#toast"); item.textContent = message; item.classList.add("show"); setTimeout(() => item.classList.remove("show"), 2200); }

document.addEventListener("click", (event) => {
  const button = (event.target as Element).closest<HTMLButtonElement>("[data-action]");
  if (button) { event.stopPropagation(); void decide(button.dataset.action!, button.dataset.verdict as Verdict).catch((error) => toast(error.message)); return; }
  const item = (event.target as Element).closest<HTMLElement>(".card");
  if (item) { const session = snapshot.sessions.find((candidate) => sessionId(candidate) === item.dataset.session); if (session) void openTail(session).catch((error) => toast(error.message)); }
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
    item.onpointerdown = (event) => { if ((event.target as Element).closest("button")) return; moved = false; item.setPointerCapture(event.pointerId); item.classList.add("dragging"); };
    item.onpointermove = (event) => { if (!item.hasPointerCapture(event.pointerId)) return; moved = true; document.querySelectorAll(".group").forEach((group) => group.classList.remove("over")); document.elementFromPoint(event.clientX, event.clientY)?.closest(".group")?.classList.add("over"); };
    item.onpointerup = (event) => { item.classList.remove("dragging"); const target = document.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement>(".group"); document.querySelectorAll(".group").forEach((group) => group.classList.remove("over")); if (!moved || !target) return; event.stopPropagation(); layout = moveSession(layout, groups, item.dataset.session!, target.dataset.group!); render(); void fetch("/layout", { method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify(layout) }).catch(() => toast("Could not save layout")); };
  }
}

const events = new EventSource("/events");
events.addEventListener("snapshot", (event) => { snapshot = JSON.parse((event as MessageEvent).data).data; updatedAt = Date.now(); render(); });
for (const type of ["machine", "session", "actionOpened", "actionClosed", "decision"]) events.addEventListener(type, () => void loadState().catch(() => {}));
document.addEventListener("visibilitychange", () => { if (document.visibilityState === "visible") { void loadState().catch(() => {}); void wake(); } });
setInterval(() => { $("#updated").textContent = updatedAt ? `Updated ${elapsed(new Date(updatedAt).toISOString())} ago` : "Connecting…"; ($("#stale") as HTMLElement).hidden = !updatedAt || Date.now() - updatedAt <= 10_000; document.querySelectorAll<HTMLElement>("[data-since]").forEach((node) => node.textContent = `${elapsed(node.dataset.since!)} in state`); }, 1_000);
async function wake() { try { if ("wakeLock" in navigator) await navigator.wakeLock.request("screen"); } catch {} }

void Promise.all([loadState(), fetch("/layout").then((response) => response.json()).then((value) => { layout = value; render(); })]).catch((error) => toast(error.message));
void wake(); if ("serviceWorker" in navigator) void navigator.serviceWorker.register("/sw.js");
