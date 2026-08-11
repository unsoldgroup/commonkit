import cytoscape from "cytoscape";
import fcose from "cytoscape-fcose";
import { demoPayload, normalizePayload, type GraphEdge, type GraphNode, type GraphPayload } from "./protocol.js";
import type { Campaign, WorkBundle } from "@commonkit/linear-graph-protocol";
import { deterministicGridColumns, edgeLabel, emptyStateKind, graphLayoutName, graphRenderKey, issueNeighbors, recommendationFor, selectionViewportAction, statusShape, teamColor, teamSummaries, topicColor, uniqueTopics, visibleGraph, zoneMetrics, type Filters, type ViewMode } from "./model.js";

cytoscape.use(fcose);
const $ = <T extends Element>(selector: string) => document.querySelector<T>(selector)!;
const app = $("#app");
let payload: GraphPayload = demoPayload;
let view: ViewMode = "focus";
let filters: Filters = { query: "", teams: new Set(), topics: new Set(), showSemantic: true, showCompleted: false };
let selectedId: string | null = null;
const selectedIds = new Set<string>();
let cy: any;
let loading = false;
let initialLoading = true;
let graphError: string | null = null;
let analysisError: string | null = null;
let campaignProposal: Campaign | null = null;
type CodexAuthStatus = { authenticated: boolean; state: "connected" | "disconnected" | "connecting" | "unavailable"; account?: string; plan?: string; expiresAt?: string; message?: string; loginUrl?: string; deviceCode?: string };
let codexAuth: CodexAuthStatus = { authenticated: false, state: "connecting", message: "Checking ChatGPT connection…" };
let codexMenuOpen = false;
let codexStatusTimer: ReturnType<typeof setInterval> | undefined;
let toastTimer: ReturnType<typeof setTimeout> | undefined;
let viewport: { zoom: number; pan: { x: number; y: number } } | null = null;
let fitNextGraph = true;
let graphRevision = 0;
let renderedGraphKey: string | null = null;
let lastEmptyState = emptyStateKind(payload.snapshot, payload.recommendations, filters, view);
let graphRequestId = 0;
let graphAbort: AbortController | undefined;

function esc(value: unknown) { const el = document.createElement("span"); el.textContent = String(value ?? ""); return el.innerHTML; }
function nodeById(id: string) { return payload.snapshot.nodes.find((node) => node.id === id); }
function selectedNode() { return selectedId ? nodeById(selectedId) : undefined; }
function toast(message: string) { const root = $("#toast"); root.textContent = message; root.classList.add("show"); if (toastTimer) clearTimeout(toastTimer); toastTimer = setTimeout(() => root.classList.remove("show"), 2600); }
function formatDate(value?: string) { if (!value) return "No due date"; return new Date(value).toLocaleDateString([], { month: "short", day: "numeric" }); }
function priority(node: GraphNode) { return node.priorityLabel ?? (node.priority === 1 ? "Urgent" : node.priority === 2 ? "High" : node.priority === 3 ? "Medium" : node.priority === 4 ? "Low" : "No priority"); }
function descriptionExcerpt(node: GraphNode, length = 180) { const text = (node.description ?? "").replace(/[`*_>#\[\]()]/g, "").replace(/\s+/g, " ").trim(); return text.length > length ? `${text.slice(0, length - 1)}…` : text || "No description yet."; }
function safeUrl(value: string) { try { const url = new URL(value); return url.protocol === "https:" || url.protocol === "http:" ? url.href : undefined; } catch { return undefined; } }
function renderMarkdown(markdown?: string) {
  if (!markdown?.trim()) return `<p class="ticket-description-empty">No description provided in Linear.</p>`;
  const inline = (value: string) => {
    let html = esc(value);
    html = html.replace(/`([^`]+)`/g, "<code>$1</code>").replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>").replace(/__([^_]+)__/g, "<strong>$1</strong>").replace(/\*([^*]+)\*/g, "<em>$1</em>");
    html = html.replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g, (_match, label: string, href: string) => { const url = safeUrl(href); return url ? `<a href="${esc(url)}" target="_blank" rel="noreferrer">${label}</a>` : label; });
    return html;
  };
  const blocks: string[] = [];
  let list: string[] = [];
  const flushList = () => { if (list.length) { blocks.push(`<ul>${list.join("")}</ul>`); list = []; } };
  for (const rawLine of markdown.replace(/\r\n?/g, "\n").split("\n")) {
    const line = rawLine.trim();
    if (!line) { flushList(); continue; }
    const bullet = line.match(/^[-*+]\s+(.+)$/);
    if (bullet) { list.push(`<li>${inline(bullet[1])}</li>`); continue; }
    flushList();
    const heading = line.match(/^(#{1,3})\s+(.+)$/);
    if (heading) { const level = heading[1].length + 1; blocks.push(`<h${level}>${inline(heading[2])}</h${level}>`); continue; }
    blocks.push(`<p>${inline(line)}</p>`);
  }
  flushList();
  return blocks.join("");
}
function selectedNodeIds() { return [...selectedIds]; }
function selectNode(id: string, additive = false) {
  const changedTicket = selectedId !== id;
  if (!additive) selectedIds.clear();
  if (additive && selectedIds.has(id)) selectedIds.delete(id); else selectedIds.add(id);
  selectedId = selectedIds.has(id) ? id : selectedIds.values().next().value ?? null;
  renderRecommendations(); renderList(); renderContext();
  if (cy) { cy.nodes().unselect(); selectedIds.forEach((selected) => cy.getElementById(selected).select()); if (selectionViewportAction(changedTicket, additive) === "fit") cy.fit(undefined, 40); }
}
function clearSelection() { selectedIds.clear(); selectedId = null; renderRecommendations(); renderList(); renderContext(); if (cy) cy.nodes().unselect(); }
async function authorizedFetch(path: string, init: RequestInit) {
  const token = () => localStorage.getItem("linear-graph-action-token") ?? "";
  const send = (auth: string) => fetch(path, { ...init, credentials: "same-origin", headers: { "Content-Type": "application/json", ...(auth ? { Authorization: `Bearer ${auth}` } : {}) } });
  const response = await send(token());
  if (response.status !== 401) return response;
  const entered = window.prompt("Graph action token");
  if (!entered?.trim()) return response;
  const retried = await send(entered.trim());
  if (retried.ok) localStorage.setItem("linear-graph-action-token", entered.trim());
  return retried;
}

function normalizeCodexAuth(body: unknown): CodexAuthStatus {
  const value = body && typeof body === "object" ? body as Record<string, unknown> : {};
  const authenticated = value.authenticated === true || value.connected === true || value.status === "connected" || value.status === "authenticated";
  const rawState = typeof value.status === "string" ? value.status : "";
  const state: CodexAuthStatus["state"] = authenticated ? "connected" : rawState === "connecting" || rawState === "pending" ? "connecting" : rawState === "unavailable" ? "unavailable" : "disconnected";
  const account = typeof value.account === "string" ? value.account.slice(0, 120) : typeof value.email === "string" ? value.email.slice(0, 120) : undefined;
  const plan = typeof value.plan === "string" ? value.plan.slice(0, 80) : undefined;
  const expiresAt = typeof value.expiresAt === "string" ? value.expiresAt : undefined;
  const message = typeof value.message === "string" ? value.message.slice(0, 240) : typeof value.detail === "string" ? value.detail.slice(0, 240) : typeof value.error === "string" ? value.error.slice(0, 240) : undefined;
  const loginUrl = typeof value.loginUrl === "string" && value.loginUrl.startsWith("https://auth.openai.com/") ? value.loginUrl : undefined;
  const deviceCode = typeof value.deviceCode === "string" ? value.deviceCode.slice(0, 32) : undefined;
  return { authenticated, state, account, plan, expiresAt, message, loginUrl, deviceCode };
}
async function refreshCodexStatus() {
  try {
    const response = await fetch("/api/codex/status", { cache: "no-store", credentials: "same-origin" });
    if (response.status === 404) codexAuth = { authenticated: false, state: "unavailable", message: "ChatGPT connection is not configured on this hub." };
    else if (!response.ok) codexAuth = { authenticated: false, state: "disconnected", message: "ChatGPT connection status is unavailable." };
    else codexAuth = normalizeCodexAuth(await response.json());
  } catch { codexAuth = { authenticated: false, state: "unavailable", message: "Could not reach the ChatGPT connection service." }; }
  renderShell();
  return codexAuth;
}
function startCodexStatusPolling() {
  if (codexStatusTimer) return;
  codexStatusTimer = setInterval(() => { void refreshCodexStatus(); }, 15000);
}
async function connectCodex() {
  if (codexAuth.state === "connecting") return;
  const loginWindow = window.open("about:blank", "codex-login");
  codexAuth = { authenticated: false, state: "connecting", message: "Opening ChatGPT connection…" };
  renderShell();
  try {
    const response = await authorizedFetch("/api/codex/login", { method: "POST", body: JSON.stringify({}) });
    if (!response.ok) throw new Error(response.status === 401 ? "Connection is not authorized. Refresh from the Tailscale URL and try again." : "ChatGPT connection could not start");
    const body = await response.json() as Record<string, unknown>;
    const loginUrl = typeof body.loginUrl === "string" ? body.loginUrl : typeof body.url === "string" ? body.url : undefined;
    const safeLoginUrl = loginUrl ? safeUrl(loginUrl) : undefined;
    const detail = typeof body.detail === "string" ? body.detail.slice(0, 240) : typeof body.message === "string" ? body.message.slice(0, 240) : undefined;
    if (safeLoginUrl && loginWindow) loginWindow.location.href = safeLoginUrl;
    else if (safeLoginUrl) window.open(safeLoginUrl, "codex-login");
    codexAuth = { authenticated: false, state: "connecting", loginUrl: safeLoginUrl, deviceCode: typeof body.deviceCode === "string" ? body.deviceCode : undefined, message: detail ?? (safeLoginUrl ? "Finish connecting in the ChatGPT window…" : "Waiting for ChatGPT connection…") };
    const refreshed = await refreshCodexStatus();
    if (refreshed.loginUrl && loginWindow) loginWindow.location.href = refreshed.loginUrl;
  } catch (error) { loginWindow?.close(); codexAuth = { authenticated: false, state: "disconnected", message: error instanceof Error ? error.message : "ChatGPT connection failed." }; renderShell(); }
}

function renderShell() {
  // Shell updates (auth polling, toasts, brief status) should not detach the
  // Cytoscape event surface. Preserve the whole wrapper, not only #cy, so
  // canvas gestures, hover layers, and toolbar hit-testing stay connected.
  const existingGraphWrap = cy ? document.querySelector<HTMLElement>(".graph-wrap") : null;
  existingGraphWrap?.remove();
  const teams = teamSummaries(payload.snapshot.nodes, payload.snapshot.teams);
  const topics = uniqueTopics(payload.snapshot.nodes);
  app.innerHTML = `<header class="topbar">
    <div class="brand"><div class="brand-mark" aria-hidden="true"><span></span><span></span><span></span></div><div><p class="eyebrow">UNSOLD · WORK GRAPH</p><h1>Focus universe</h1></div></div>
    <div class="top-actions"><span class="freshness ${payload.snapshot.stale ? "stale" : ""}"><i></i>${payload.snapshot.stale ? "Snapshot stale" : `Synced ${formatRelative(payload.snapshot.generatedAt)}`}</span><button id="analyze" class="primary" type="button" ${loading ? "disabled" : ""}><span class="spark">✦</span>${loading ? "Analyzing…" : "Analyze now"}</button><div class="codex-menu"><button id="codex-menu-button" class="codex-menu-button" type="button" aria-haspopup="true" aria-expanded="${codexMenuOpen}" aria-label="ChatGPT connection status"><i class="codex-status-dot ${codexAuth.state}"></i><span>Codex</span><span class="codex-menu-chevron">⌄</span></button><div id="codex-menu-panel" class="codex-menu-panel" ${codexMenuOpen ? "" : "hidden"} role="menu"><div class="codex-menu-title"><span>ChatGPT connection</span><b class="codex-state ${codexAuth.state}">${codexAuth.authenticated ? "Connected" : codexAuth.state === "connecting" ? "Connecting…" : codexAuth.state === "unavailable" ? "Unavailable" : "Not connected"}</b></div>${codexAuth.account ? `<div class="codex-account">${esc(codexAuth.account)}${codexAuth.plan ? ` · ${esc(codexAuth.plan)}` : ""}</div>` : ""}${codexAuth.expiresAt ? `<div class="codex-account">Session refresh ${esc(formatDate(codexAuth.expiresAt))}</div>` : ""}${codexAuth.message ? `<p class="codex-menu-message">${esc(codexAuth.message)}</p>` : ""}${codexAuth.loginUrl ? `<a class="codex-login-link" href="${esc(codexAuth.loginUrl)}" target="_blank" rel="noreferrer">Open ChatGPT sign-in ↗</a>` : ""}${codexAuth.deviceCode ? `<div class="codex-device-code"><span>Device code</span><code>${esc(codexAuth.deviceCode)}</code></div>` : ""}<button id="codex-connect" class="codex-connect" type="button" role="menuitem" ${codexAuth.state === "connecting" ? "disabled" : ""}>${codexAuth.authenticated ? "Reconnect ChatGPT" : "Connect ChatGPT"}</button></div></div></div>
  </header>
  <main class="layout">
    <aside class="rail" aria-label="Focus controls">
      <section class="brief-card ${graphError || analysisError || ["failed", "stale"].includes(payload.brief?.status ?? "") ? "has-error" : ""}"><div class="section-kicker"><span class="status-dot"></span> TODAY'S BRIEF</div><p>${briefCopy()}</p>${analysisError ? `<small class="brief-error">${esc(analysisError)}</small>` : payload.analysis?.status === "failed" ? `<small class="brief-error">${esc(payload.analysis.error ?? "The last brief run failed.")}</small>` : payload.brief?.error ? `<small class="brief-error">${esc(payload.brief.error)}</small>` : graphError ? `<small class="brief-error">${esc(graphError)}</small>` : ""}<button id="edit-brief" class="quiet-button" type="button">Edit brief <span>↗</span></button></section>
      <section class="rail-section"><div class="section-heading"><h2>Recommended</h2><span class="count">${payload.recommendations.length}</span></div><div id="recommendations" class="recommendations"></div></section>
      <section class="rail-section"><div class="section-heading"><h2>Workstreams</h2><span class="count">${payload.snapshot.nodes.length}</span></div><div id="zones" class="zones"></div></section>
      <section class="rail-section team-section"><div class="section-heading"><h2>Teams</h2><span class="count">${teams.length}</span></div><div id="teams" class="teams">${teams.map((team) => `<button type="button" class="team-row ${filters.teams.has(team.name) ? "active" : ""}" data-filter-kind="team" data-filter-value="${esc(team.name)}"><i style="--team:${team.color}"></i><span>${esc(team.name)}</span><b>${team.count}</b></button>`).join("") || `<p class="muted">No team metadata yet.</p>`}</div></section>
    </aside>
    <section class="workspace">
      <div class="workspace-head"><div class="view-tabs" role="tablist" aria-label="Graph view"><button class="${view === "focus" ? "active" : ""}" data-view="focus" role="tab" aria-selected="${view === "focus"}">Focus <span>${payload.recommendations.length}</span></button><button class="${view === "universe" ? "active" : ""}" data-view="universe" role="tab" aria-selected="${view === "universe"}">Universe <span>${payload.snapshot.nodes.length}</span></button></div><div class="workspace-tools"><label class="search"><span aria-hidden="true">⌕</span><input id="search" type="search" aria-label="Search issues" value="${esc(filters.query)}" placeholder="Search issues, teams, repos…"><kbd>/</kbd></label><button id="filters" class="icon-button" type="button" aria-label="Open filters" aria-expanded="false">☷</button><label class="toggle"><input id="semantic" type="checkbox" ${filters.showSemantic ? "checked" : ""}><span></span>Codex links</label><button id="campaign-selection" class="selection-action" type="button" data-campaign="selection">${selectedIds.size >= 2 ? `Bundle ${selectedIds.size}` : "Campaign"}</button></div></div>
      <div id="filter-panel" class="filter-panel" hidden><div><span>Teams</span>${teams.map((team) => filterChip("team", team.name, filters.teams.has(team.name), team.color, team.count)).join("")}</div><div><span>Topics</span>${topics.map((topic) => filterChip("topic", topic, filters.topics.has(topic))).join("")}</div><label class="toggle"><input id="completed" type="checkbox" ${filters.showCompleted ? "checked" : ""}><span></span>Show completed</label><button id="clear-filters" class="text-button" type="button">Clear all</button></div>
      <section id="ticket-context" class="ticket-context" hidden aria-live="polite"></section>
      ${campaignProposal ? renderCampaignProposal(campaignProposal) : ""}
      <div class="graph-wrap"><div id="cy" role="img" aria-label="Interactive issue relationship graph"></div><div id="node-hover-card" class="node-hover-card" role="tooltip" hidden></div><div class="graph-toolbar" aria-label="Graph navigation"><button type="button" data-cy-action="zoom-in" aria-label="Zoom in" title="Zoom in (+)">+</button><button type="button" data-cy-action="zoom-out" aria-label="Zoom out" title="Zoom out (-)">−</button><button type="button" data-cy-action="fit" aria-label="Fit graph" title="Fit graph (0)">Fit</button><button type="button" data-cy-action="reset" aria-label="Reset graph view" title="Reset view (R)">Reset</button><span class="toolbar-divider"></span><button type="button" data-cy-action="pan-up" aria-label="Pan graph up" title="Pan up">↑</button><button type="button" data-cy-action="pan-left" aria-label="Pan graph left" title="Pan left">←</button><button type="button" data-cy-action="pan-down" aria-label="Pan graph down" title="Pan down">↓</button><button type="button" data-cy-action="pan-right" aria-label="Pan graph right" title="Pan right">→</button><button type="button" id="shortcuts" aria-label="Show keyboard shortcuts" title="Keyboard shortcuts">?</button></div><div class="graph-legend"><span><i class="legend-line solid"></i>Linear relation</span><span><i class="legend-line dashed"></i>Codex suggestion</span><span class="legend-topic-key"><i class="legend-topic"></i>Topic fill</span><span class="legend-team-key"><i class="legend-team"></i>Team border</span><span><i class="legend-status"></i>Status shape</span></div><div id="graph-empty" class="graph-empty" hidden></div><div id="shortcuts-help" class="shortcuts-help" hidden role="dialog" aria-label="Keyboard shortcuts"><button id="close-shortcuts" type="button" aria-label="Close shortcuts">×</button><strong>Keyboard shortcuts</strong><span><kbd>/</kbd> Search <kbd>+</kbd><kbd>−</kbd> Zoom <kbd>0</kbd> Fit</span><span><kbd>F</kbd> Focus <kbd>U</kbd> Universe <kbd>R</kbd> Reset</span><span><kbd>Esc</kbd> Close details</span></div></div>
      <section class="list-alternative" aria-label="Accessible issue list"><div class="list-heading"><h2>Accessible list</h2><span>Use this view if the graph is too dense.</span></div><div id="issue-list"></div></section>
    </section>
  </main><div id="toast" role="status" aria-live="polite"></div>`;
  if (existingGraphWrap && cy) {
    $(".graph-wrap").replaceWith(existingGraphWrap);
    cy.resize();
  }
  renderRecommendations(); renderZones(); renderList(); renderGraph(); renderContext();
}

function renderCampaignProposal(campaign: Campaign) {
  const bundleCards = campaign.bundles.map((bundle: WorkBundle) => `<article class="bundle-card"><div><span class="section-kicker">BUNDLE</span><strong>${esc(bundle.title)}</strong><p>${esc(bundle.summary)}</p><small>${bundle.issueIds.length} tickets · expected reduction ${bundle.expectedReduction}</small>${bundle.linearProjectUrl ? `<a href="${esc(bundle.linearProjectUrl)}" target="_blank" rel="noreferrer">Linear project ↗</a>` : bundle.linearSyncError ? `<small class="bundle-warning">${esc(bundle.linearSyncError)}</small>` : ""}</div><div class="bundle-actions"><span class="campaign-status status-${bundle.status}">${esc(bundle.status)}</span>${bundle.status === "proposed" ? `<button type="button" class="selection-action" data-approve-bundle="${esc(bundle.id)}">Approve</button>` : ""}${bundle.status === "approved" ? `<button type="button" class="selection-action" data-run-bundle="${esc(bundle.id)}">Run agent</button>` : ""}</div></article>`).join("");
  const resolution = campaign.resolutionSet;
  return `<section class="campaign-summary" aria-live="polite"><div class="campaign-summary-head"><span class="section-kicker">CAMPAIGN PROPOSAL</span><strong>${esc(campaign.title)}</strong><span>${campaign.issueIds.length} tickets · ${campaign.bundles.length} bundle${campaign.bundles.length === 1 ? "" : "s"}${resolution ? ` · ${resolution.issueIds.length} duplicate review candidate${resolution.issueIds.length === 1 ? "" : "s"}` : ""}</span></div><span class="campaign-status">${esc(campaign.status)}</span><div class="bundle-list">${bundleCards || `<p class="muted">No active issues matched this campaign. Try a broader theme.</p>`}</div>${resolution ? `<div class="resolution-card"><span><b>Duplicate review set</b><small>${resolution.issueIds.length} issues · ${esc(resolution.rationale)}</small></span>${resolution.status === "proposed" ? `<button type="button" class="selection-action" data-approve-resolution="${esc(resolution.id)}">Approve review set</button>` : `<span class="campaign-status">${esc(resolution.status)}</span>`}</div>` : ""}</section>`;
}
function filterChip(kind: "team" | "topic", label: string, active: boolean, color?: string, count?: number) { return `<button class="filter-chip ${active ? "active" : ""}" data-filter-kind="${kind}" data-filter-value="${esc(label)}" type="button">${color ? `<i style="--team:${color}"></i>` : ""}${esc(label)}${typeof count === "number" ? `<b>${count}</b>` : ""}</button>`; }
function formatRelative(iso: string) { const minutes = Math.max(0, Math.floor((Date.now() - Date.parse(iso)) / 60000)); return minutes < 1 ? "just now" : minutes < 60 ? `${minutes}m ago` : `${Math.floor(minutes / 60)}h ago`; }
function briefCopy() {
  if (initialLoading && !payload.brief) return "Loading today’s brief…";
  if (payload.analysis?.status === "running" || payload.brief?.status === "running") return payload.brief?.text ?? "Codex is preparing today’s brief from the current work universe…";
  if (payload.brief?.text) return payload.brief.text;
  if (graphError) return "The brief could not be loaded. Retry the graph refresh to recover the latest prioritization.";
  return "No focus brief yet. Run an analysis to find the work that matters next.";
}
function renderRecommendations() {
  const root = $("#recommendations");
  root.innerHTML = payload.recommendations.slice(0, 5).map((rec) => { const node = nodeById(rec.issueId); if (!node) return ""; return `<button class="recommendation ${selectedId === node.id ? "selected" : ""}" type="button" data-node="${esc(node.id)}"><span class="rec-rank">${String(rec.rank).padStart(2, "0")}</span><span class="rec-copy"><strong>${esc(node.identifier)} <em>${esc(node.title)}</em></strong><small>${esc(rec.nextAction || rec.whyNow)}</small></span><span class="rec-score">${Math.round((rec.confidence ?? 0) * 100)}</span></button>`; }).join("") || `<p class="muted">No recommendations yet.</p>`;
}
function renderZones() {
  const root = $("#zones");
  const metrics = zoneMetrics(payload.snapshot.nodes, payload.snapshot.zones);
  const maxWorkload = Math.max(1, ...metrics.map((zone) => zone.weightedWorkload));
  root.innerHTML = metrics.map((zone) => { const meter = Math.round((zone.weightedWorkload / maxWorkload) * 100); return `<button type="button" class="zone-row ${filters.topics.has(zone.name) ? "active" : ""}" data-filter-kind="topic" data-filter-value="${esc(zone.name)}" aria-label="${esc(zone.name)}: ${zone.activeCount} active issues, workload ${zone.weightedWorkload}"><i style="--zone:${zone.color}"></i><span class="zone-copy"><b>${esc(zone.name)}</b><small>${zone.activeCount} active · ${zone.weightedWorkload} workload</small><span class="zone-meter" aria-hidden="true"><i style="width:${meter}%;--zone:${zone.color}"></i></span></span><b class="zone-count">${zone.issueCount}</b></button>`; }).join("") || `<p class="muted">No topical zones yet.</p>`;
}
function renderList() {
  const graph = visibleGraph(payload.snapshot, payload.recommendations, filters, view);
  const root = $("#issue-list");
  root.innerHTML = graph.nodes.slice().sort((a, b) => (b.focusScore ?? 0) - (a.focusScore ?? 0)).map((node) => `<button class="issue-row ${selectedId === node.id ? "selected" : ""}" type="button" data-node="${esc(node.id)}"><span class="issue-id">${esc(node.identifier)}</span><span class="issue-title">${esc(node.title)}</span><span class="issue-topic">${esc(node.topic ?? "Unsorted")}</span><span class="issue-status ${node.statusType ?? "unstarted"}">${esc(node.status)}</span><span class="issue-team">${esc(node.team)}</span></button>`).join("") || `<p class="muted">No issues in this view.</p>`;
}
function renderGraph() {
  const root = $("#cy");
  const graph = visibleGraph(payload.snapshot, payload.recommendations, filters, view);
  const nextGraphKey = graphRenderKey(graphRevision, view, filters);
  if (cy && renderedGraphKey === nextGraphKey) return;
  renderedGraphKey = nextGraphKey;
  lastEmptyState = emptyStateKind(payload.snapshot, payload.recommendations, filters, view);
  const empty = $("#graph-empty") as HTMLElement;
  empty.hidden = graph.nodes.length > 0;
  if (!graph.nodes.length) {
    empty.innerHTML = emptyStateCopy(lastEmptyState);
    if (cy) { viewport = { zoom: cy.zoom(), pan: { ...cy.pan() } }; cy.destroy(); cy = undefined; }
    return;
  }
  const forceFit = fitNextGraph;
  if (cy) {
    if (!forceFit) viewport = { zoom: cy.zoom(), pan: { ...cy.pan() } };
    else viewport = null;
    cy.destroy();
  }
  const ids = new Set(graph.nodes.map((node) => node.id));
  const sortedNodes = graph.nodes.slice().sort((left, right) => left.id.localeCompare(right.id));
  const graphZoneMetrics = zoneMetrics(graph.nodes, graph.zones);
  const elements = [
    ...graphZoneMetrics.map((zone) => ({ data: { id: `zone:${zone.id}`, label: `${zone.name}\n${zone.activeCount} active · ${zone.weightedWorkload} load`, zoneName: zone.name, zoneColor: zone.color, zoneIssueCount: zone.issueCount, zoneActiveCount: zone.activeCount, zoneWorkload: zone.weightedWorkload, zonePadding: Math.min(60, 22 + Math.sqrt(zone.activeCount) * 7), isZone: true } })),
    ...sortedNodes.map((node) => ({ data: { id: node.id, label: node.identifier, title: node.title, topic: node.topic ?? "Unsorted", description: descriptionExcerpt(node), topicColor: topicColor(node, graph.zones), teamColor: teamColor(node.teamKey ?? node.team), statusType: node.statusType ?? "unstarted", statusShape: statusShape(node.statusType), parent: graph.zones.find((zone) => zone.name === node.topic)?.id ? `zone:${graph.zones.find((zone) => zone.name === node.topic)!.id}` : undefined, score: node.focusScore ?? 0, selected: selectedIds.has(node.id) } })),
    ...graph.edges.filter((edge) => ids.has(edge.source) && ids.has(edge.target)).map((edge) => ({ data: { id: edge.id, source: edge.source, target: edge.target, kind: edge.kind, sourceType: edge.sourceType ?? "linear", confidence: edge.confidence ?? 0 } })),
  ];
  const layoutName = graphLayoutName(view);
  const layout = layoutName === "fcose"
    ? { name: layoutName, quality: "default", animate: false, fit: forceFit || !viewport, padding: 40, nodeRepulsion: 8000, idealEdgeLength: 130, nestingFactor: 0.9 }
    : { name: layoutName, animate: false, fit: forceFit || !viewport, padding: 40, avoidOverlap: true, avoidOverlapPadding: 5, condense: true, rows: deterministicGridColumns(graph.nodes.length), sort: (left: any, right: any) => left.id().localeCompare(right.id()) };
  cy = cytoscape({ container: root, elements, wheelSensitivity: 0.22, minZoom: 0.25, maxZoom: 2.5, style: [
    { selector: "node", style: { "background-color": "data(topicColor)", "background-opacity": 0.72, "border-color": "data(teamColor)", "border-width": 2, color: "#f5f8ff", label: "data(label)", "font-family": "Fira Code, monospace", "font-size": 10, "text-valign": "center", "text-halign": "center", width: 48, height: 29, shape: "data(statusShape)", "overlay-opacity": 0 } },
    { selector: "node[isZone]", style: { "background-color": "data(zoneColor)", "background-opacity": 0.045, "border-color": "data(zoneColor)", "border-opacity": 0.4, "border-width": 1, label: "data(label)", color: "data(zoneColor)", "font-size": 11, "font-weight": 600, padding: "data(zonePadding)", shape: "roundrectangle", "text-valign": "top", "text-margin-y": -10, "text-wrap": "wrap", "text-max-width": 180, "compound-sizing-wrt-labels": "include" } },
    { selector: "node[score > 80]", style: { "border-width": 3 } },
    { selector: 'node[statusType = "completed"]', style: { opacity: 0.62 } },
    { selector: 'node[statusType = "canceled"]', style: { opacity: 0.42, "border-style": "dashed" } },
    { selector: "node:selected", style: { "border-color": "#f5bd5c", "border-width": 4, "background-opacity": 0.95 } },
    { selector: "edge", style: { width: 1.3, "line-color": "#52637f", "target-arrow-color": "#52637f", "target-arrow-shape": "triangle", "curve-style": "bezier", opacity: 0.7 } },
    { selector: 'edge[sourceType = "codex"]', style: { "line-style": "dashed", "line-color": "#d39bff", "target-arrow-color": "#d39bff", opacity: 0.48, width: 1 } },
    { selector: 'edge[kind = "blocks"]', style: { "line-color": "#ff8f82", "target-arrow-color": "#ff8f82", width: 2 } },
    { selector: ".low-zoom", style: { "text-opacity": 0, width: 13, height: 13, "border-width": 1 } },
    { selector: "edge.low-zoom", style: { opacity: 0.16, width: 0.7 } },
  ], layout });
  const restoreViewport = () => {
    if (!cy) return;
    if (forceFit) { cy.fit(undefined, 40); fitNextGraph = false; viewport = null; return; }
    if (!viewport) return;
    cy.zoom(viewport.zoom); cy.pan(viewport.pan); viewport = null;
  };
  cy.one("layoutstop", restoreViewport);
  queueMicrotask(restoreViewport);
  cy.on("zoom pan", () => { viewport = { zoom: cy.zoom(), pan: { ...cy.pan() } }; });
  const updateZoomDensity = () => { const lowZoom = cy.zoom() < (view === "universe" ? 0.42 : 0.32); cy.nodes().toggleClass("low-zoom", lowZoom); cy.edges().toggleClass("low-zoom", lowZoom); };
  cy.on("zoom", updateZoomDensity);
  updateZoomDensity();
  const showPreview = (event: any) => {
    const target = event.target;
    if (!target || target.data("isZone")) return;
    const node = nodeById(target.id()); const preview = $("#node-hover-card") as HTMLElement;
    if (!node || !preview) return;
    preview.innerHTML = `<strong>${esc(node.identifier)} · ${esc(node.title)}</strong><span>${esc(descriptionExcerpt(node))}</span><small><i style="--team:${teamColor(node.teamKey ?? node.team)}"></i>${esc(node.team)} · ${esc(node.status)} · ${esc(priority(node))}</small>`;
    const position = event.renderedPosition ?? target.renderedPosition();
    preview.style.left = `${Math.min(Math.max(12, position.x + 14), Math.max(12, cy.width() - 280))}px`;
    preview.style.top = `${Math.min(Math.max(12, position.y + 14), Math.max(12, cy.height() - 130))}px`;
    preview.hidden = false;
  };
  const hidePreview = () => { const preview = $("#node-hover-card") as HTMLElement; if (preview) preview.hidden = true; };
  cy.on("mouseover focus", "node", showPreview);
  cy.on("mouseout blur", "node", hidePreview);
  cy.on("tap", "node", (event: any) => { if (event.target.data("isZone")) return; const original = event.originalEvent as MouseEvent | undefined; selectNode(event.target.id(), Boolean(original?.metaKey || original?.ctrlKey || original?.shiftKey)); });
  cy.on("tap", (event: any) => { if (event.target === cy) clearSelection(); });
}
function emptyStateCopy(kind: ReturnType<typeof emptyStateKind>) {
  if (kind === "no-data") return `<strong>No issues have synced yet</strong><span>Connect Linear, then run an analysis to build your work universe.</span><button class="empty-action" id="analyze-empty" type="button">Run analysis</button>`;
  if (kind === "no-recommendations") return `<strong>Focus is waiting for an analysis</strong><span>Codex has not ranked a next action for this snapshot.</span><button class="empty-action" id="analyze-empty" type="button">Analyze now</button>`;
  if (kind === "completed-only") return `<strong>All synced issues are complete</strong><span>Show completed work to inspect the full universe, or run an analysis after new work lands.</span><button class="empty-action" id="show-completed-empty" type="button">Show completed</button>`;
  if (kind === "filtered") return `<strong>No issues match these filters</strong><span>Clear a filter or broaden your search to see more of the graph.</span><button class="empty-action" id="clear-empty" type="button">Clear filters</button>`;
  return `<strong>This view is empty</strong><span>Try Universe or refresh the snapshot to see more work.</span><button class="empty-action" id="show-universe-empty" type="button">Open Universe</button>`;
}
function runGraphAction(action: string) {
  if (!cy) return;
  if (action === "zoom-in") cy.zoom({ level: cy.zoom() * 1.25, renderedPosition: { x: cy.width() / 2, y: cy.height() / 2 } });
  else if (action === "zoom-out") cy.zoom({ level: cy.zoom() / 1.25, renderedPosition: { x: cy.width() / 2, y: cy.height() / 2 } });
  else if (action === "fit") cy.fit(undefined, 40);
  else if (action === "reset") { viewport = null; cy.fit(undefined, 40); }
  else if (action.startsWith("pan-")) { const pan = cy.pan(); const amount = Math.max(45, Math.min(cy.width(), cy.height()) * 0.18); const delta = action === "pan-up" ? { x: 0, y: -amount } : action === "pan-down" ? { x: 0, y: amount } : action === "pan-left" ? { x: -amount, y: 0 } : { x: amount, y: 0 }; cy.pan({ x: pan.x + delta.x, y: pan.y + delta.y }); }
}
function toggleShortcuts(force?: boolean) { const panel = $("#shortcuts-help") as HTMLElement; panel.hidden = force === undefined ? !panel.hidden : !force; }
function renderContext() {
  const context = $("#ticket-context") as HTMLElement; const node = selectedNode();
  if (!context) return;
  if (!node) { context.hidden = true; context.innerHTML = ""; return; }
  context.hidden = false;
  const rec = recommendationFor(node.id, payload.recommendations); const neighbors = issueNeighbors(node.id, payload.snapshot.edges).map(nodeById).filter(Boolean) as GraphNode[];
  const zoneOptions = payload.snapshot.zones.map((zone) => `<option value="${esc(zone.id)}" ${zone.id === node.topicId ? "selected" : ""}>${esc(zone.name)}</option>`).join("");
  context.innerHTML = `<div class="context-heading"><div><span class="issue-id">${esc(node.identifier)}</span><span class="context-selection">${selectedIds.size > 1 ? `${selectedIds.size} selected` : "Selected ticket"}</span></div><div class="context-actions"><button class="selection-action" type="button" data-campaign="selection" ${selectedIds.size < 2 ? "disabled" : ""}>Plan bundle (${selectedIds.size})</button><button class="icon-button" id="close-context" type="button" aria-label="Close ticket context">×</button></div></div><div class="context-main"><div class="context-copy"><h2>${esc(node.title)}</h2><div class="drawer-meta"><span class="pill status-${node.statusType ?? "unstarted"}">${esc(node.status)}</span><span class="pill">${esc(priority(node))}</span><span class="pill">${esc(node.team)}</span><span class="pill topic-pill" style="--topic:${topicColor(node, payload.snapshot.zones)}">${esc(node.topic ?? "Unsorted")}</span></div><article class="ticket-description"><div class="section-kicker">DESCRIPTION</div>${renderMarkdown(node.description)}</article></div><div class="context-side"><label class="topic-editor"><span>Primary topic</span><select id="topic-select" aria-label="Primary topic">${zoneOptions}</select></label><dl class="metadata"><div><dt>Project</dt><dd>${esc(node.project ?? "No project")}</dd></div><div><dt>Repository</dt><dd>${esc(node.repo ?? "Unmapped")}</dd></div><div><dt>Due</dt><dd>${formatDate(node.dueDate)}</dd></div><div><dt>Assignee</dt><dd>${esc(node.assignee ?? "Unassigned")}</dd></div></dl>${node.labels?.length ? `<div class="context-labels">${node.labels.map((label) => `<span class="label-chip">${esc(label)}</span>`).join("")}</div>` : ""}</div></div>${rec ? `<section class="next-action"><div class="section-kicker">NEXT ACTION <span class="confidence">${Math.round((rec.confidence ?? 0) * 100)}% confidence</span></div><strong>${esc(rec.nextAction)}</strong><p>${esc(rec.whyNow)}</p></section>` : ""}<section class="connections"><div class="section-heading"><h3>Connections</h3><span class="count">${neighbors.length}</span></div>${neighbors.map((neighbor) => { const edge = payload.snapshot.edges.find((item) => (item.source === node.id && item.target === neighbor.id) || (item.target === node.id && item.source === neighbor.id))!; return `<button class="connection" type="button" data-node="${esc(neighbor.id)}"><span class="connection-kind ${edge.sourceType === "codex" ? "codex" : ""}">${edgeLabel(edge)}</span><span><b>${esc(neighbor.identifier)}</b> ${esc(neighbor.title)}</span><span>›</span></button>`; }).join("") || `<p class="muted">No linked issues.</p>`}</section>${node.url ? `<a class="linear-link" href="${esc(node.url)}" target="_blank" rel="noreferrer">Open in Linear ↗</a>` : ""}`;
}

async function saveTopic(issueId: string, zone: string) {
  const response = await authorizedFetch(`/api/issues/${encodeURIComponent(issueId)}/topic`, { method: "PUT", body: JSON.stringify({ zone }) });
  if (!response.ok) throw new Error("Could not save topic");
  const node = nodeById(issueId);
  if (node) { node.topicId = zone; node.topic = payload.snapshot.zones.find((item) => item.id === zone)?.name ?? zone; graphRevision += 1; }
  renderShell();
  selectedId = issueId;
  selectedIds.add(issueId);
  renderContext();
  toast("Primary topic saved");
}

async function loadGraph() {
  fitNextGraph = true;
  const requestId = ++graphRequestId;
  graphAbort?.abort();
  const controller = new AbortController();
  graphAbort = controller;
  const requestedView = view;
  graphError = null;
  if (initialLoading) renderShell();
  try {
    const response = await fetch(`/api/graph?view=${requestedView}`, { cache: "no-store", signal: controller.signal });
    if (!response.ok) throw new Error("Graph unavailable");
    const body = await response.json() as Record<string, unknown>;
    if (requestId !== graphRequestId || controller.signal.aborted || requestedView !== view) return;
    if (body.snapshot && typeof body.snapshot === "object" && "nodes" in body.snapshot) { const snapshot = body.snapshot as Parameters<typeof normalizePayload>[0]; payload = normalizePayload(snapshot, body.brief as Parameters<typeof normalizePayload>[1], body.analysis as Parameters<typeof normalizePayload>[2]); graphRevision += 1; fitNextGraph = true; }
    else if ("nodes" in body) { payload = normalizePayload(body as Parameters<typeof normalizePayload>[0]); graphRevision += 1; fitNextGraph = true; }
    graphError = null;
    initialLoading = false;
    renderShell();
  } catch (error) {
    if (controller.signal.aborted || requestId !== graphRequestId) return;
    initialLoading = false;
    graphError = error instanceof Error ? error.message : "Graph unavailable";
    if (payload === demoPayload) toast("Showing demo data · API is not connected yet");
    renderShell();
  } finally { if (graphAbort === controller) graphAbort = undefined; }
}
async function waitForAnalysis() {
  for (let attempt = 0; attempt < 120; attempt += 1) {
    await new Promise((resolve) => setTimeout(resolve, 1000));
    const response = await fetch("/api/analysis-runs/latest", { cache: "no-store" });
    if (!response.ok) throw new Error("Analysis status could not be loaded");
    const run = await response.json() as { status?: string; error?: string | null };
    if (run.status === "completed") return;
    if (run.status === "failed") throw new Error(run.error || "Codex analysis failed on the VPS");
    if (attempt === 29 || attempt === 59 || attempt === 89) toast("Codex is still analyzing the work universe…");
  }
  throw new Error("Analysis is still running after two minutes; check the brief again shortly");
}
async function createCampaign() {
  if (loading) return;
  const seedNodes = selectedNodeIds().map(nodeById).filter(Boolean) as GraphNode[];
  const defaultPrompt = seedNodes.length ? `Bundle related work around ${seedNodes.slice(0, 3).map((node) => node.identifier).join(", ")}` : "Find and group related open work that can be drained together";
  const prompt = window.prompt("Describe the thematic campaign Codex should propose", defaultPrompt);
  if (!prompt?.trim()) return;
  loading = true; renderShell();
  try {
    const response = await authorizedFetch("/api/campaigns", { method: "POST", body: JSON.stringify({ prompt: prompt.trim(), ...(selectedIds.size ? { seedIssueIds: selectedNodeIds() } : {}) }) });
    if (!response.ok) throw new Error(response.status === 401 ? "Campaign planning needs a graph action token" : "Campaign proposal could not be created");
    campaignProposal = await response.json() as Campaign;
    toast(`Campaign ready · ${campaignProposal.issueIds.length} tickets staged`);
    renderShell();
  } catch (error) { toast(error instanceof Error ? error.message : "Campaign proposal failed"); } finally { loading = false; renderShell(); }
}
async function approveBundle(bundleId: string) {
  const response = await authorizedFetch(`/api/bundles/${encodeURIComponent(bundleId)}/approval`, { method: "POST", body: JSON.stringify({ decision: "approve" }) });
  if (!response.ok) { toast("Bundle approval failed"); return; }
  const approved = await response.json() as WorkBundle;
  if (campaignProposal) campaignProposal = { ...campaignProposal, status: "approved", bundles: campaignProposal.bundles.map((bundle) => bundle.id === approved.id ? approved : bundle) };
  toast("Bundle approved; Linear project sync queued"); renderShell();
}
async function approveResolution(resolutionId: string) {
  const response = await authorizedFetch(`/api/resolution-sets/${encodeURIComponent(resolutionId)}/approval`, { method: "POST", body: JSON.stringify({ decision: "approve" }) });
  if (!response.ok) { toast("Review-set approval failed"); return; }
  const resolution = await response.json();
  if (campaignProposal) campaignProposal = { ...campaignProposal, resolutionSet: resolution };
  toast("Duplicate review set approved"); renderShell();
}
async function runBundle(bundleId: string) {
  const repository = window.prompt("Repository key from the VPS execution allowlist", "commonkit");
  if (!repository?.trim()) return;
  const instruction = window.prompt("Optional agent instruction", "Implement the approved bundle, run checks, and report evidence.") ?? undefined;
  const response = await authorizedFetch(`/api/bundles/${encodeURIComponent(bundleId)}/executions`, { method: "POST", body: JSON.stringify({ repository: repository.trim(), instruction }) });
  if (!response.ok) { toast("Agent execution could not start"); return; }
  const execution = await response.json() as { status?: string };
  if (campaignProposal) campaignProposal = { ...campaignProposal, status: "running", bundles: campaignProposal.bundles.map((bundle) => bundle.id === bundleId ? { ...bundle, status: execution.status === "verified" ? "verified" : "running" } : bundle) };
  toast("Headless Codex execution finished; review the evidence"); renderShell();
}
async function analyze() { if (loading) return; loading = true; analysisError = null; renderShell(); try { const response = await authorizedFetch("/api/analysis-runs", { method: "POST", body: JSON.stringify({ reason: "manual" }) }); if (!response.ok) throw new Error(response.status === 401 ? "Analysis is not authorized. Refresh the page from the Tailscale URL and try again." : "Analysis could not start"); toast("Codex analysis started"); await waitForAnalysis(); await loadGraph(); toast("Analysis complete — the brief and Focus view are updated"); } catch (error) { analysisError = error instanceof Error ? error.message : "Analysis failed"; toast(analysisError); } finally { loading = false; renderShell(); } }
async function editBrief() { const current = payload.brief?.text ?? ""; const text = window.prompt("Focus brief", current); if (text === null) return; try { const response = await authorizedFetch("/api/focus-brief", { method: "PUT", body: JSON.stringify({ text: text.trim() }) }); if (!response.ok) throw new Error("Could not save brief"); payload.brief = { text: text.trim(), updatedAt: new Date().toISOString() }; renderShell(); } catch { toast("Could not save brief"); } }
function toggleFilter(kind: "team" | "topic", value: string) { const target = kind === "team" ? filters.teams : filters.topics; if (target.has(value)) target.delete(value); else target.add(value); renderShell(); }

document.addEventListener("click", (event) => { const target = event.target as Element; const button = target.closest<HTMLButtonElement>("button"); if (!button) return; if (button.dataset.cyAction) { runGraphAction(button.dataset.cyAction); return; } if (button.id === "codex-menu-button") { codexMenuOpen = !codexMenuOpen; const panel = $("#codex-menu-panel") as HTMLElement; panel.hidden = !codexMenuOpen; button.setAttribute("aria-expanded", String(codexMenuOpen)); return; } if (button.id === "codex-connect") { codexMenuOpen = true; void connectCodex(); return; } if (button.dataset.campaign) { void createCampaign(); return; } if (button.dataset.approveBundle) { void approveBundle(button.dataset.approveBundle); return; } if (button.dataset.approveResolution) { void approveResolution(button.dataset.approveResolution); return; } if (button.dataset.runBundle) { void runBundle(button.dataset.runBundle); return; } if (button.dataset.view) { view = button.dataset.view as ViewMode; void loadGraph(); return; } if (button.dataset.node) { const mouse = event as MouseEvent; selectNode(button.dataset.node, Boolean(mouse.metaKey || mouse.ctrlKey || mouse.shiftKey)); return; } if (button.dataset.filterKind) { toggleFilter(button.dataset.filterKind as "team" | "topic", button.dataset.filterValue!); return; } if (button.id === "analyze" || button.id === "analyze-empty") { void analyze(); return; } if (button.id === "edit-brief") { void editBrief(); return; } if (button.id === "filters") { const panel = $("#filter-panel") as HTMLElement; panel.hidden = !panel.hidden; button.setAttribute("aria-expanded", String(!panel.hidden)); return; } if (button.id === "clear-filters" || button.id === "clear-empty") { filters.teams.clear(); filters.topics.clear(); filters.query = ""; renderShell(); return; } if (button.id === "show-completed-empty") { filters.showCompleted = true; renderShell(); return; } if (button.id === "show-universe-empty") { view = "universe"; void loadGraph(); return; } if (button.id === "shortcuts") { toggleShortcuts(); return; } if (button.id === "close-shortcuts") { toggleShortcuts(false); return; } if (button.id === "close-context") { clearSelection(); } });
document.addEventListener("input", (event) => { const input = event.target as HTMLInputElement; if (input.id === "search") { filters.query = input.value; renderList(); renderGraph(); } if (input.id === "semantic") { filters.showSemantic = input.checked; renderGraph(); renderList(); } if (input.id === "completed") { filters.showCompleted = input.checked; renderGraph(); renderList(); } });
document.addEventListener("change", (event) => { const select = event.target as HTMLSelectElement; if (select.id === "topic-select" && selectedId) void saveTopic(selectedId, select.value).catch(() => toast("Could not save topic")); });
document.addEventListener("keydown", (event) => { const tag = document.activeElement?.tagName; if (["INPUT", "TEXTAREA", "SELECT"].includes(tag ?? "")) { if (event.key === "Escape") (document.activeElement as HTMLElement).blur(); return; } const key = event.key.toLowerCase(); if (event.key === "/") { event.preventDefault(); ($( "#search") as HTMLInputElement)?.focus(); return; } if (event.key === "+" || event.key === "=") { event.preventDefault(); runGraphAction("zoom-in"); return; } if (event.key === "-" || event.key === "_") { event.preventDefault(); runGraphAction("zoom-out"); return; } if (event.key === "0") { event.preventDefault(); runGraphAction("fit"); return; } if (key === "r") { event.preventDefault(); runGraphAction("reset"); return; } if (key === "f") { event.preventDefault(); view = "focus"; void loadGraph(); return; } if (key === "u") { event.preventDefault(); view = "universe"; void loadGraph(); return; } if (event.key === "?") { event.preventDefault(); toggleShortcuts(); return; } if (event.key === "Escape") { if (!($("#shortcuts-help") as HTMLElement).hidden) { toggleShortcuts(false); return; } if (selectedId) { selectedId = null; renderShell(); } } });

void loadGraph();
void refreshCodexStatus();
startCodexStatusPolling();
