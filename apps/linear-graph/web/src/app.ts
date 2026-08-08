import cytoscape from "cytoscape";
import fcose from "cytoscape-fcose";
import { demoPayload, normalizePayload, type GraphEdge, type GraphNode, type GraphPayload } from "./protocol.js";
import { deterministicGridColumns, edgeLabel, emptyStateKind, graphLayoutName, issueNeighbors, recommendationFor, teamSummaries, topicCounts, uniqueTopics, visibleGraph, type Filters, type ViewMode } from "./model.js";

cytoscape.use(fcose);
const $ = <T extends Element>(selector: string) => document.querySelector<T>(selector)!;
const app = $("#app");
let payload: GraphPayload = demoPayload;
let view: ViewMode = "focus";
let filters: Filters = { query: "", teams: new Set(), topics: new Set(), showSemantic: true, showCompleted: false };
let selectedId: string | null = null;
let cy: any;
let loading = false;
let toastTimer: ReturnType<typeof setTimeout> | undefined;
let viewport: { zoom: number; pan: { x: number; y: number } } | null = null;
let lastEmptyState = emptyStateKind(payload.snapshot, payload.recommendations, filters, view);
let graphRequestId = 0;
let graphAbort: AbortController | undefined;

function esc(value: unknown) { const el = document.createElement("span"); el.textContent = String(value ?? ""); return el.innerHTML; }
function nodeById(id: string) { return payload.snapshot.nodes.find((node) => node.id === id); }
function selectedNode() { return selectedId ? nodeById(selectedId) : undefined; }
function toast(message: string) { const root = $("#toast"); root.textContent = message; root.classList.add("show"); if (toastTimer) clearTimeout(toastTimer); toastTimer = setTimeout(() => root.classList.remove("show"), 2600); }
function formatDate(value?: string) { if (!value) return "No due date"; return new Date(value).toLocaleDateString([], { month: "short", day: "numeric" }); }
function priority(node: GraphNode) { return node.priorityLabel ?? (node.priority === 1 ? "Urgent" : node.priority === 2 ? "High" : node.priority === 3 ? "Medium" : node.priority === 4 ? "Low" : "No priority"); }
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

function renderShell() {
  const teams = teamSummaries(payload.snapshot.nodes, payload.snapshot.teams);
  const topics = uniqueTopics(payload.snapshot.nodes);
  app.innerHTML = `<header class="topbar">
    <div class="brand"><div class="brand-mark" aria-hidden="true"><span></span><span></span><span></span></div><div><p class="eyebrow">UNSOLD · WORK GRAPH</p><h1>Focus universe</h1></div></div>
    <div class="top-actions"><span class="freshness ${payload.snapshot.stale ? "stale" : ""}"><i></i>${payload.snapshot.stale ? "Snapshot stale" : `Synced ${formatRelative(payload.snapshot.generatedAt)}`}</span><button id="analyze" class="primary" type="button" ${loading ? "disabled" : ""}><span class="spark">✦</span>${loading ? "Analyzing…" : "Analyze now"}</button></div>
  </header>
  <main class="layout">
    <aside class="rail" aria-label="Focus controls">
      <section class="brief-card"><div class="section-kicker"><span class="status-dot"></span> TODAY'S BRIEF</div><p>${esc(payload.brief?.text ?? "No focus brief yet. Run an analysis to find the work that matters next.")}</p><button id="edit-brief" class="quiet-button" type="button">Edit brief <span>↗</span></button></section>
      <section class="rail-section"><div class="section-heading"><h2>Recommended</h2><span class="count">${payload.recommendations.length}</span></div><div id="recommendations" class="recommendations"></div></section>
      <section class="rail-section"><div class="section-heading"><h2>Workstreams</h2><span class="count">${payload.snapshot.nodes.length}</span></div><div id="zones" class="zones"></div></section>
      <section class="rail-section team-section"><div class="section-heading"><h2>Teams</h2><span class="count">${teams.length}</span></div><div id="teams" class="teams">${teams.map((team) => `<button type="button" class="team-row ${filters.teams.has(team.name) ? "active" : ""}" data-filter-kind="team" data-filter-value="${esc(team.name)}"><i style="--team:${team.color}"></i><span>${esc(team.name)}</span><b>${team.count}</b></button>`).join("") || `<p class="muted">No team metadata yet.</p>`}</div></section>
    </aside>
    <section class="workspace">
      <div class="workspace-head"><div class="view-tabs" role="tablist" aria-label="Graph view"><button class="${view === "focus" ? "active" : ""}" data-view="focus" role="tab" aria-selected="${view === "focus"}">Focus <span>${payload.recommendations.length}</span></button><button class="${view === "universe" ? "active" : ""}" data-view="universe" role="tab" aria-selected="${view === "universe"}">Universe <span>${payload.snapshot.nodes.length}</span></button></div><div class="workspace-tools"><label class="search"><span aria-hidden="true">⌕</span><input id="search" type="search" aria-label="Search issues" value="${esc(filters.query)}" placeholder="Search issues, teams, repos…"><kbd>/</kbd></label><button id="filters" class="icon-button" type="button" aria-label="Open filters" aria-expanded="false">☷</button><label class="toggle"><input id="semantic" type="checkbox" ${filters.showSemantic ? "checked" : ""}><span></span>Codex links</label></div></div>
      <div id="filter-panel" class="filter-panel" hidden><div><span>Teams</span>${teams.map((team) => filterChip("team", team.name, filters.teams.has(team.name), team.color, team.count)).join("")}</div><div><span>Topics</span>${topics.map((topic) => filterChip("topic", topic, filters.topics.has(topic))).join("")}</div><label class="toggle"><input id="completed" type="checkbox" ${filters.showCompleted ? "checked" : ""}><span></span>Show completed</label><button id="clear-filters" class="text-button" type="button">Clear all</button></div>
      <div class="graph-wrap"><div id="cy" role="img" aria-label="Interactive issue relationship graph"></div><div class="graph-toolbar" aria-label="Graph navigation"><button type="button" data-cy-action="zoom-in" aria-label="Zoom in" title="Zoom in (+)">+</button><button type="button" data-cy-action="zoom-out" aria-label="Zoom out" title="Zoom out (-)">−</button><button type="button" data-cy-action="fit" aria-label="Fit graph" title="Fit graph (0)">Fit</button><button type="button" data-cy-action="reset" aria-label="Reset graph view" title="Reset view (R)">Reset</button><span class="toolbar-divider"></span><button type="button" data-cy-action="pan-up" aria-label="Pan graph up" title="Pan up">↑</button><button type="button" data-cy-action="pan-left" aria-label="Pan graph left" title="Pan left">←</button><button type="button" data-cy-action="pan-down" aria-label="Pan graph down" title="Pan down">↓</button><button type="button" data-cy-action="pan-right" aria-label="Pan graph right" title="Pan right">→</button><button type="button" id="shortcuts" aria-label="Show keyboard shortcuts" title="Keyboard shortcuts">?</button></div><div class="graph-legend"><span><i class="legend-line solid"></i>Linear relation</span><span><i class="legend-line dashed"></i>Codex suggestion</span><span><i class="legend-node"></i>Ready to focus</span></div><div id="graph-empty" class="graph-empty" hidden></div><div id="shortcuts-help" class="shortcuts-help" hidden role="dialog" aria-label="Keyboard shortcuts"><button id="close-shortcuts" type="button" aria-label="Close shortcuts">×</button><strong>Keyboard shortcuts</strong><span><kbd>/</kbd> Search <kbd>+</kbd><kbd>−</kbd> Zoom <kbd>0</kbd> Fit</span><span><kbd>F</kbd> Focus <kbd>U</kbd> Universe <kbd>R</kbd> Reset</span><span><kbd>Esc</kbd> Close details</span></div></div>
      <section class="list-alternative" aria-label="Accessible issue list"><div class="list-heading"><h2>Accessible list</h2><span>Use this view if the graph is too dense.</span></div><div id="issue-list"></div></section>
    </section>
    <aside id="drawer" class="drawer" aria-label="Issue details" aria-live="polite"><div class="drawer-empty"><span class="drawer-icon">⊙</span><strong>Select an issue</strong><span>See context, relationships, and your next action.</span></div></aside>
  </main><div id="toast" role="status" aria-live="polite"></div>`;
  renderRecommendations(); renderZones(); renderList(); renderGraph();
}

function filterChip(kind: "team" | "topic", label: string, active: boolean, color?: string, count?: number) { return `<button class="filter-chip ${active ? "active" : ""}" data-filter-kind="${kind}" data-filter-value="${esc(label)}" type="button">${color ? `<i style="--team:${color}"></i>` : ""}${esc(label)}${typeof count === "number" ? `<b>${count}</b>` : ""}</button>`; }
function formatRelative(iso: string) { const minutes = Math.max(0, Math.floor((Date.now() - Date.parse(iso)) / 60000)); return minutes < 1 ? "just now" : minutes < 60 ? `${minutes}m ago` : `${Math.floor(minutes / 60)}h ago`; }
function renderRecommendations() {
  const root = $("#recommendations");
  root.innerHTML = payload.recommendations.slice(0, 5).map((rec) => { const node = nodeById(rec.issueId); if (!node) return ""; return `<button class="recommendation ${selectedId === node.id ? "selected" : ""}" type="button" data-node="${esc(node.id)}"><span class="rec-rank">${String(rec.rank).padStart(2, "0")}</span><span class="rec-copy"><strong>${esc(node.identifier)} <em>${esc(node.title)}</em></strong><small>${esc(rec.nextAction || rec.whyNow)}</small></span><span class="rec-score">${Math.round((rec.confidence ?? 0) * 100)}</span></button>`; }).join("") || `<p class="muted">No recommendations yet.</p>`;
}
function renderZones() {
  const root = $("#zones");
  root.innerHTML = topicCounts(payload.snapshot.nodes, payload.snapshot.zones).map((zone) => `<button type="button" class="zone-row ${filters.topics.has(zone.name) ? "active" : ""}" data-filter-kind="topic" data-filter-value="${esc(zone.name)}"><i style="--zone:${zone.color}"></i><span>${esc(zone.name)}</span><b>${zone.issueCount}</b></button>`).join("");
}
function renderList() {
  const graph = visibleGraph(payload.snapshot, payload.recommendations, filters, view);
  const root = $("#issue-list");
  root.innerHTML = graph.nodes.slice().sort((a, b) => (b.focusScore ?? 0) - (a.focusScore ?? 0)).map((node) => `<button class="issue-row ${selectedId === node.id ? "selected" : ""}" type="button" data-node="${esc(node.id)}"><span class="issue-id">${esc(node.identifier)}</span><span class="issue-title">${esc(node.title)}</span><span class="issue-topic">${esc(node.topic ?? "Unsorted")}</span><span class="issue-status ${node.statusType ?? "unstarted"}">${esc(node.status)}</span><span class="issue-team">${esc(node.team)}</span></button>`).join("") || `<p class="muted">No issues in this view.</p>`;
}
function renderGraph() {
  const root = $("#cy");
  const graph = visibleGraph(payload.snapshot, payload.recommendations, filters, view);
  lastEmptyState = emptyStateKind(payload.snapshot, payload.recommendations, filters, view);
  const empty = $("#graph-empty") as HTMLElement;
  empty.hidden = graph.nodes.length > 0;
  if (!graph.nodes.length) {
    empty.innerHTML = emptyStateCopy(lastEmptyState);
    if (cy) { viewport = { zoom: cy.zoom(), pan: { ...cy.pan() } }; cy.destroy(); cy = undefined; }
    return;
  }
  if (cy) { viewport = { zoom: cy.zoom(), pan: { ...cy.pan() } }; cy.destroy(); }
  const ids = new Set(graph.nodes.map((node) => node.id));
  const sortedNodes = graph.nodes.slice().sort((left, right) => left.id.localeCompare(right.id));
  const elements = [
    ...graph.zones.map((zone) => ({ data: { id: `zone:${zone.id}`, label: zone.name, zoneColor: zone.color, isZone: true } })),
    ...sortedNodes.map((node) => ({ data: { id: node.id, label: node.identifier, title: node.title, topic: node.topic ?? "Unsorted", parent: graph.zones.find((zone) => zone.name === node.topic)?.id ? `zone:${graph.zones.find((zone) => zone.name === node.topic)!.id}` : undefined, score: node.focusScore ?? 0, selected: node.id === selectedId } })),
    ...graph.edges.filter((edge) => ids.has(edge.source) && ids.has(edge.target)).map((edge) => ({ data: { id: edge.id, source: edge.source, target: edge.target, kind: edge.kind, sourceType: edge.sourceType ?? "linear", confidence: edge.confidence ?? 0 } })),
  ];
  const layoutName = graphLayoutName(view);
  const layout = layoutName === "fcose"
    ? { name: layoutName, quality: "default", animate: false, fit: !viewport, padding: 40, nodeRepulsion: 8000, idealEdgeLength: 130, nestingFactor: 0.9 }
    : { name: layoutName, animate: false, fit: !viewport, padding: 40, avoidOverlap: true, avoidOverlapPadding: 5, condense: true, rows: deterministicGridColumns(graph.nodes.length), sort: (left: any, right: any) => left.id().localeCompare(right.id()) };
  cy = cytoscape({ container: root, elements, wheelSensitivity: 0.22, minZoom: 0.25, maxZoom: 2.5, style: [
    { selector: "node", style: { "background-color": "#182130", "border-color": "#52637f", "border-width": 1, color: "#dfe9ff", label: "data(label)", "font-family": "Fira Code, monospace", "font-size": 10, "text-valign": "center", "text-halign": "center", width: 44, height: 28, shape: "round-rectangle", "overlay-opacity": 0 } },
    { selector: "node[isZone]", style: { "background-color": "data(zoneColor)", "background-opacity": 0.045, "border-color": "data(zoneColor)", "border-opacity": 0.4, "border-width": 1, label: "data(label)", color: "data(zoneColor)", "font-size": 11, "font-weight": 600, padding: 24, shape: "roundrectangle", "text-valign": "top", "text-margin-y": -10, "compound-sizing-wrt-labels": "include" } },
    { selector: "node[score > 80]", style: { "border-color": "#f5bd5c", "border-width": 2, "background-color": "#22304a" } },
    { selector: "node:selected", style: { "border-color": "#f5bd5c", "border-width": 3, "background-color": "#2d3d5e" } },
    { selector: "edge", style: { width: 1.3, "line-color": "#52637f", "target-arrow-color": "#52637f", "target-arrow-shape": "triangle", "curve-style": "bezier", opacity: 0.7 } },
    { selector: 'edge[sourceType = "codex"]', style: { "line-style": "dashed", "line-color": "#d39bff", "target-arrow-color": "#d39bff", opacity: 0.48, width: 1 } },
    { selector: 'edge[kind = "blocks"]', style: { "line-color": "#ff8f82", "target-arrow-color": "#ff8f82", width: 2 } },
    { selector: ".low-zoom", style: { "text-opacity": 0, width: 13, height: 13, "border-width": 1 } },
    { selector: "edge.low-zoom", style: { opacity: 0.16, width: 0.7 } },
  ], layout });
  const restoreViewport = () => { if (!cy || !viewport) return; cy.zoom(viewport.zoom); cy.pan(viewport.pan); viewport = null; };
  cy.one("layoutstop", restoreViewport);
  queueMicrotask(restoreViewport);
  cy.on("zoom pan", () => { viewport = { zoom: cy.zoom(), pan: { ...cy.pan() } }; });
  const updateZoomDensity = () => { const lowZoom = cy.zoom() < (view === "universe" ? 0.42 : 0.32); cy.nodes().toggleClass("low-zoom", lowZoom); cy.edges().toggleClass("low-zoom", lowZoom); };
  cy.on("zoom", updateZoomDensity);
  updateZoomDensity();
  cy.on("tap", "node", (event: any) => { if (event.target.data("isZone")) return; selectedId = event.target.id(); renderRecommendations(); renderList(); renderDrawer(); cy.nodes().unselect(); event.target.select(); });
  cy.on("tap", (event: any) => { if (event.target === cy) { selectedId = null; renderRecommendations(); renderList(); renderDrawer(); } });
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
function renderDrawer() {
  const drawer = $("#drawer"); const node = selectedNode();
  if (!node) { drawer.innerHTML = `<div class="drawer-empty"><span class="drawer-icon">⊙</span><strong>Select an issue</strong><span>See context, relationships, and your next action.</span></div>`; return; }
  const rec = recommendationFor(node.id, payload.recommendations); const neighbors = issueNeighbors(node.id, payload.snapshot.edges).map(nodeById).filter(Boolean) as GraphNode[];
  const zoneOptions = payload.snapshot.zones.map((zone) => `<option value="${esc(zone.id)}" ${zone.id === node.topicId ? "selected" : ""}>${esc(zone.name)}</option>`).join("");
  drawer.innerHTML = `<div class="drawer-top"><span class="issue-id">${esc(node.identifier)}</span><button class="icon-button" id="close-drawer" type="button" aria-label="Close details">×</button></div><h2>${esc(node.title)}</h2><div class="drawer-meta"><span class="pill status-${node.statusType ?? "unstarted"}">${esc(node.status)}</span><span class="pill">${esc(priority(node))}</span><span class="pill">${esc(node.team)}</span></div><label class="topic-editor"><span>Primary topic</span><select id="topic-select" aria-label="Primary topic">${zoneOptions}</select></label><dl class="metadata"><div><dt>Project</dt><dd>${esc(node.project ?? "No project")}</dd></div><div><dt>Repository</dt><dd>${esc(node.repo ?? "Unmapped")}</dd></div><div><dt>Due</dt><dd>${formatDate(node.dueDate)}</dd></div><div><dt>Assignee</dt><dd>${esc(node.assignee ?? "Unassigned")}</dd></div></dl>${rec ? `<section class="next-action"><div class="section-kicker">NEXT ACTION <span class="confidence">${Math.round((rec.confidence ?? 0) * 100)}% confidence</span></div><strong>${esc(rec.nextAction)}</strong><p>${esc(rec.whyNow)}</p></section>` : ""}<section class="connections"><div class="section-heading"><h3>Connections</h3><span class="count">${neighbors.length}</span></div>${neighbors.map((neighbor) => { const edge = payload.snapshot.edges.find((item) => (item.source === node.id && item.target === neighbor.id) || (item.target === node.id && item.source === neighbor.id))!; return `<button class="connection" type="button" data-node="${esc(neighbor.id)}"><span class="connection-kind ${edge.sourceType === "codex" ? "codex" : ""}">${edgeLabel(edge)}</span><span><b>${esc(neighbor.identifier)}</b> ${esc(neighbor.title)}</span><span>›</span></button>`; }).join("") || `<p class="muted">No linked issues.</p>`}</section>${node.url ? `<a class="linear-link" href="${esc(node.url)}" target="_blank" rel="noreferrer">Open in Linear ↗</a>` : ""}`;
}

async function saveTopic(issueId: string, zone: string) {
  const response = await authorizedFetch(`/api/issues/${encodeURIComponent(issueId)}/topic`, { method: "PUT", body: JSON.stringify({ zone }) });
  if (!response.ok) throw new Error("Could not save topic");
  const node = nodeById(issueId);
  if (node) { node.topicId = zone; node.topic = payload.snapshot.zones.find((item) => item.id === zone)?.name ?? zone; }
  renderShell();
  selectedId = issueId;
  renderDrawer();
  toast("Primary topic saved");
}

async function loadGraph() {
  const requestId = ++graphRequestId;
  graphAbort?.abort();
  const controller = new AbortController();
  graphAbort = controller;
  const requestedView = view;
  try {
    const response = await fetch(`/api/graph?view=${requestedView}`, { cache: "no-store", signal: controller.signal });
    if (!response.ok) throw new Error("Graph unavailable");
    const body = await response.json() as Record<string, unknown>;
    if (requestId !== graphRequestId || controller.signal.aborted || requestedView !== view) return;
    if (body.snapshot && typeof body.snapshot === "object" && "nodes" in body.snapshot) { const snapshot = body.snapshot as Parameters<typeof normalizePayload>[0]; payload = normalizePayload(snapshot, body.brief as Parameters<typeof normalizePayload>[1], body.analysis as Parameters<typeof normalizePayload>[2]); }
    else if ("nodes" in body) payload = normalizePayload(body as Parameters<typeof normalizePayload>[0]);
    renderShell();
  } catch (error) {
    if (controller.signal.aborted || requestId !== graphRequestId) return;
    if (payload === demoPayload) toast("Showing demo data · API is not connected yet");
    renderShell();
  } finally { if (graphAbort === controller) graphAbort = undefined; }
}
async function analyze() { if (loading) return; loading = true; renderShell(); try { const response = await authorizedFetch("/api/analysis-runs", { method: "POST", body: JSON.stringify({ reason: "manual" }) }); if (!response.ok) throw new Error(response.status === 401 ? "Analysis needs a graph action token" : "Analysis could not start"); toast("Codex analysis started"); await new Promise((resolve) => setTimeout(resolve, 700)); await loadGraph(); } catch (error) { toast(error instanceof Error ? error.message : "Analysis failed"); } finally { loading = false; renderShell(); } }
async function editBrief() { const current = payload.brief?.text ?? ""; const text = window.prompt("Focus brief", current); if (text === null) return; try { const response = await authorizedFetch("/api/focus-brief", { method: "PUT", body: JSON.stringify({ text: text.trim() }) }); if (!response.ok) throw new Error("Could not save brief"); payload.brief = { text: text.trim(), updatedAt: new Date().toISOString() }; renderShell(); } catch { toast("Could not save brief"); } }
function toggleFilter(kind: "team" | "topic", value: string) { const target = kind === "team" ? filters.teams : filters.topics; if (target.has(value)) target.delete(value); else target.add(value); renderShell(); }

document.addEventListener("click", (event) => { const target = event.target as Element; const button = target.closest<HTMLButtonElement>("button"); if (!button) return; if (button.dataset.cyAction) { runGraphAction(button.dataset.cyAction); return; } if (button.dataset.view) { view = button.dataset.view as ViewMode; void loadGraph(); return; } if (button.dataset.node) { selectedId = button.dataset.node; renderRecommendations(); renderList(); renderDrawer(); if (cy) { cy.nodes().unselect(); cy.getElementById(selectedId).select(); cy.center(cy.getElementById(selectedId)); } return; } if (button.dataset.filterKind) { toggleFilter(button.dataset.filterKind as "team" | "topic", button.dataset.filterValue!); return; } if (button.id === "analyze" || button.id === "analyze-empty") { void analyze(); return; } if (button.id === "edit-brief") { void editBrief(); return; } if (button.id === "filters") { const panel = $("#filter-panel") as HTMLElement; panel.hidden = !panel.hidden; button.setAttribute("aria-expanded", String(!panel.hidden)); return; } if (button.id === "clear-filters" || button.id === "clear-empty") { filters.teams.clear(); filters.topics.clear(); filters.query = ""; renderShell(); return; } if (button.id === "show-completed-empty") { filters.showCompleted = true; renderShell(); return; } if (button.id === "show-universe-empty") { view = "universe"; void loadGraph(); return; } if (button.id === "shortcuts") { toggleShortcuts(); return; } if (button.id === "close-shortcuts") { toggleShortcuts(false); return; } if (button.id === "close-drawer") { selectedId = null; renderShell(); } });
document.addEventListener("input", (event) => { const input = event.target as HTMLInputElement; if (input.id === "search") { filters.query = input.value; renderList(); renderGraph(); } if (input.id === "semantic") { filters.showSemantic = input.checked; renderGraph(); renderList(); } if (input.id === "completed") { filters.showCompleted = input.checked; renderGraph(); renderList(); } });
document.addEventListener("change", (event) => { const select = event.target as HTMLSelectElement; if (select.id === "topic-select" && selectedId) void saveTopic(selectedId, select.value).catch(() => toast("Could not save topic")); });
document.addEventListener("keydown", (event) => { const tag = document.activeElement?.tagName; if (["INPUT", "TEXTAREA", "SELECT"].includes(tag ?? "")) { if (event.key === "Escape") (document.activeElement as HTMLElement).blur(); return; } const key = event.key.toLowerCase(); if (event.key === "/") { event.preventDefault(); ($( "#search") as HTMLInputElement)?.focus(); return; } if (event.key === "+" || event.key === "=") { event.preventDefault(); runGraphAction("zoom-in"); return; } if (event.key === "-" || event.key === "_") { event.preventDefault(); runGraphAction("zoom-out"); return; } if (event.key === "0") { event.preventDefault(); runGraphAction("fit"); return; } if (key === "r") { event.preventDefault(); runGraphAction("reset"); return; } if (key === "f") { event.preventDefault(); view = "focus"; void loadGraph(); return; } if (key === "u") { event.preventDefault(); view = "universe"; void loadGraph(); return; } if (event.key === "?") { event.preventDefault(); toggleShortcuts(); return; } if (event.key === "Escape") { if (!($("#shortcuts-help") as HTMLElement).hidden) { toggleShortcuts(false); return; } if (selectedId) { selectedId = null; renderShell(); } } });

void loadGraph();
