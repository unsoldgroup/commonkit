import { timingSafeEqual } from "node:crypto";
import { mkdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve, sep } from "node:path";
import { z } from "zod";
import {
  DEFAULT_ZONES, bundleApprovalRequestSchema, campaignRequestSchema, focusBriefSchema, graphSnapshotSchema,
  executionRequestSchema, triageDecisionRequestSchema, updateFocusBriefRequestSchema, updateTopicRequestSchema,
  type AnalysisRun, type Campaign, type ExecutionRun, type GraphSnapshot, type TriageDecision, type WorkBundle, type EnrichmentProgress,
} from "@commonkit/linear-graph-protocol";
import { applyCodexAnalysis, focusSnapshot } from "./analysis.js";
import { createCodexAuthManager, type CodexAuthOptions } from "./codex-auth.js";
import { runCodexAnalysis, type CodexRunnerOptions } from "./codex.js";
import { computeDrainMetrics, proposeCampaign } from "./drain.js";
import { buildFocusModel, renderFocusModelHtml, type FocusMetrics, type FocusModel } from "./focusView.js";
import { executeApprovedBundle, type ExecutionRunnerOptions } from "./execution.js";
import { syncLinear, type LinearSource } from "./linear.js";
import { assignRelationZones, freshness, nextUp, rankedRecommendations } from "./nextup.js";
import { inferTopicAssignment, summarizeTeams } from "./normalizer.js";
import { redactSensitiveText } from "./redaction.js";
import { GraphStore } from "./store.js";
import { projectMembership } from "./membership.js";
import { classifyTheme, themeInputHash } from "./themes.js";

export interface GraphHubOptions {
  port?: number;
  hostname?: string;
  actionToken: string;
  store?: GraphStore;
  dataPath?: string;
  linear?: LinearSource;
  codex?: CodexRunnerOptions;
  codexAuth?: CodexAuthOptions;
  execution?: Omit<ExecutionRunnerOptions, "repositoryAllowlist"> & { repositoryAllowlist: Readonly<Record<string, string>> };
  webDistPath?: string;
  now?: () => Date;
  themes?: { gatewayUrl: string; token: string; gatewayToken?: string; fetch?: typeof fetch };
}
export interface GraphHub { url: string; store: GraphStore; runAnalysis(): Promise<AnalysisRun>; stop(): Promise<void> }

const jsonHeaders = { "Content-Type": "application/json; charset=utf-8", "Cache-Control": "no-store" };
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status, headers: jsonHeaders });
const error = (message: string, status: number) => json({ error: message }, status);

function validBearer(request: Request, expected: string) {
  const actual = request.headers.get("Authorization")?.replace(/^Bearer\s+/i, "");
  if (!actual) return false;
  const left = Buffer.from(actual); const right = Buffer.from(expected);
  return left.length === right.length && timingSafeEqual(left, right);
}

export function trustedNonDestructiveMutation(request: Request, pathname: string) {
  if (request.method !== "POST" || !["/api/analysis-runs", "/api/campaigns", "/api/codex/login"].includes(pathname)) return false;
  if (request.headers.get("X-Linear-Graph-Tailnet") === "1") return true;
  // graph.unsold.cloud is Tailscale-only; embedded browsers can strip the
  // proxy marker, so accept the exact same-origin browser request as the
  // equivalent trusted path. Approvals and execution remain token-gated.
  return request.headers.get("Origin") === "https://graph.unsold.cloud" || request.headers.get("Sec-Fetch-Site") === "same-origin";
}

const focusMetricsSchema = z.object({
  generatedAt: z.string(),
  tiles: z.array(z.object({ key: z.string(), label: z.string(), value: z.string(), detail: z.string() })),
});
const FOCUS_CACHE_MS = 300_000;

/** metrics.json sits beside graph.sqlite; absent or malformed means "awaiting events". */
function readFocusMetrics(dataPath: string | undefined): FocusMetrics | null {
  if (!dataPath || dataPath === ":memory:") return null;
  try { return focusMetricsSchema.parse(JSON.parse(readFileSync(join(dirname(dataPath), "metrics.json"), "utf8"))); } catch { return null; }
}

function parseBody<T>(request: Request, schema: z.ZodType<T>): Promise<T> {
  return request.json().then((body) => schema.parse(body));
}

export function startGraphHub(options: GraphHubOptions): GraphHub {
  const now = options.now ?? (() => new Date());
  if (options.dataPath) mkdirSync(dirname(options.dataPath), { recursive: true });
  const store = options.store ?? new GraphStore(options.dataPath ?? ":memory:");
  const webDistPath = options.webDistPath ?? join(import.meta.dir, "../../web/dist");
  const codexAuth = createCodexAuthManager(options.codexAuth);
  const codexAuthMode = options.codexAuth?.mode ?? "subscription";
  const persisted = store.loadSnapshot();
  const grouped = persisted ? graphSnapshotSchema.parse({
    ...persisted,
    nodes: persisted.nodes.map((issue) => {
      const fallback = inferTopicAssignment(issue);
      return fallback ? { ...issue, zone: fallback.zone, topicTags: fallback.topicTags } : issue;
    }),
  }) : null;
  if (persisted && grouped && JSON.stringify(persisted.nodes.map((node) => node.zone)) !== JSON.stringify(grouped.nodes.map((node) => node.zone))) {
    store.saveSnapshot({ ...grouped, teams: summarizeTeams(grouped.nodes) });
  }
  let current = grouped;
  let themeJob: Promise<void> | undefined;
  let themeError: string | null = null;
  let stopping = false;
  const runThemes = () => {
    if (!options.themes || !current || themeJob || stopping) return;
    const nodes = current.nodes.filter((n) => !["completed", "canceled"].includes(n.status.type)).map((node) => {
      const hash = themeInputHash(node.title);
      return { node, hash, retryAfter: store.themeRetryAfter(node.id, hash) };
    }).sort((a, b) => Number(a.retryAfter !== null) - Number(b.retryAfter !== null));
    themeJob = (async () => {
      let failures = 0;
      let consecutiveFailures = 0;
      for (const { node, hash, retryAfter } of nodes) {
        if (stopping) break;
        if (store.getTheme(node.id, hash)) continue;
        if ((retryAfter ?? 0) > now().getTime()) continue;
        try {
          const result = await classifyTheme(node.title, options.themes!);
          store.saveTheme(node.id, hash, result.theme, result.suggested, result.confidence);
          consecutiveFailures = 0;
        } catch {
          // Skip a single bad issue; stop repeated outages before making thousands of calls.
          failures++;
          store.deferTheme(node.id, hash, now().getTime() + 60 * 60_000);
          if (++consecutiveFailures >= 3) break;
        }
      }
      themeError = failures ? "Theme grouping failed; retrying later." : null;
    })().finally(() => { themeJob = undefined; });
  };
  const themeTimer = options.themes ? setInterval(runThemes, 15 * 60_000) : undefined;
  if (themeTimer) setTimeout(runThemes, 0);
  const interruptedBrief = store.loadBrief();
  if (interruptedBrief?.status === "running" && store.latestAnalysisRun()?.status === "failed") {
    store.saveBrief({ ...interruptedBrief, status: "stale", error: store.latestAnalysisRun()!.error, updatedAt: now().toISOString() });
  }
  if (current && !store.loadBrief()) {
    const activeCount = current.nodes.filter((node) => !["completed", "canceled"].includes(node.status.type)).length;
    const timestamp = now().toISOString();
    store.saveBrief({ text: `Universe synced: ${activeCount} active issues are ready for triage and bundle planning.`, updatedAt: timestamp, generatedAt: timestamp, snapshotAt: current.syncedAt, status: "ready", source: "fallback", error: null });
  }
  let analysisInFlight: Promise<AnalysisRun> | undefined;
  const updateDrainMetrics = () => {
    if (!current) return null;
    const metrics = computeDrainMetrics(current, store.loadTriageDecisions(), store.loadCampaigns(), now());
    store.saveDrainMetrics(metrics);
    return metrics;
  };

  const runAnalysis = async (): Promise<AnalysisRun> => {
    if (analysisInFlight) return analysisInFlight;
    analysisInFlight = (async () => {
      const startedAt = now().toISOString();
      const id = crypto.randomUUID();
      const running = { id, startedAt, completedAt: null, status: "running", issueCount: 0, error: null, inputHash: null } as const;
      store.saveAnalysisRun(running);
      const existingBrief = store.loadBrief();
      if (existingBrief) store.saveBrief({ ...existingBrief, status: "running", error: null, updatedAt: now().toISOString() });
      let fallback: GraphSnapshot | null = null;
      let enrichment: EnrichmentProgress | undefined;
      try {
        if (!options.linear) throw new Error("Linear source is not configured");
        const sync = await syncLinear(options.linear, store.topicOverrides());
        // USG-383: Linear is the primary structure. Zones come from parent/blocks/related
        // links and ranking from priority + relations, so the snapshot is published before
        // (and independent of) Codex. Codex failures no longer freeze the graph.
        const issues = assignRelationZones(sync.issues, sync.edges);
        const baseline: GraphSnapshot = graphSnapshotSchema.parse({ generatedAt: now().toISOString(), syncedAt: sync.syncedAt, stale: false, nodes: issues, edges: sync.edges, teams: sync.teams, zones: DEFAULT_ZONES, recommendations: rankedRecommendations(issues, sync.edges, existingBrief), latestAnalysisRunId: id });
        fallback = baseline;
        store.saveSnapshot(baseline);
        current = baseline;
        runThemes();
        // Refresh fallback counts on every sync, even while Codex is running.
        // A previous Codex brief keeps its original provenance until replaced.
        const previousBrief = store.loadBrief();
        if (!previousBrief || previousBrief.source === "fallback") {
          const activeCount = issues.filter((issue) => !["completed", "canceled"].includes(issue.status.type)).length;
          const timestamp = now().toISOString();
          store.saveBrief({ text: `Universe synced: ${activeCount} active issues are ready for triage and bundle planning.`, updatedAt: timestamp, generatedAt: timestamp, snapshotAt: baseline.syncedAt, status: options.codex ? "running" : "ready", source: "fallback", error: null });
        }
        let enrichmentError: string | null = null;
        let inputHash: string | null = null;
        if (options.codex) {
          try {
            const result = await runCodexAnalysis({ issues, edges: sync.edges, focusBrief: existingBrief?.source === "manual" ? existingBrief.text : undefined }, options.codex, (progress) => {
              enrichment = progress;
              store.saveAnalysisRun({ ...running, issueCount: sync.rawCount, enrichment });
            });
            if (!result.brief) throw new Error("Codex returned no daily brief");
            const applied = applyCodexAnalysis(issues, sync.edges, result, sync.syncedAt, id);
            const snapshot = { ...applied.snapshot, zones: baseline.zones, recommendations: baseline.recommendations };
            store.saveSnapshot(snapshot);
            current = snapshot;
            inputHash = applied.inputHash;
            const timestamp = now().toISOString();
            // Al's manual brief is the ranking input; never overwrite it with a generated one.
            if (store.loadBrief()?.source !== "manual") store.saveBrief({ text: result.brief, updatedAt: timestamp, generatedAt: timestamp, snapshotAt: snapshot.syncedAt, status: "ready", source: "codex", error: null });
            for (const decision of result.triageDecisions ?? []) {
              const previous = store.loadTriageDecision(decision.issueId);
              store.saveTriageDecision({ ...decision, createdAt: previous?.createdAt ?? timestamp, updatedAt: timestamp, source: "codex" });
            }
          } catch (caught) {
            if (enrichment) enrichment = { ...enrichment, status: "failed" };
            enrichmentError = `Codex enrichment skipped: ${caught instanceof Error ? caught.message : "failed"}`.slice(0, 1000);
            console.error("Linear graph Codex enrichment failed", enrichmentError);
          }
        }
        const brief = store.loadBrief();
        if (brief) {
          const stale = brief.source !== "manual" && brief.snapshotAt !== baseline.syncedAt;
          const status = brief.source === "manual" ? "ready" : stale ? "stale" : enrichmentError ? "failed" : "ready";
          store.saveBrief({ ...brief, status, error: enrichmentError, updatedAt: now().toISOString() });
        }
        updateDrainMetrics();
        const completed = { ...running, completedAt: now().toISOString(), status: "completed", issueCount: sync.rawCount, error: enrichmentError, inputHash, ...(enrichment ? { enrichment } : {}) } as const;
        store.saveAnalysisRun(completed);
        return completed;
      } catch (caught) {
        const staleSnapshot = current ?? fallback;
        if (staleSnapshot) {
          current = graphSnapshotSchema.parse({ ...staleSnapshot, stale: true });
          store.saveSnapshot(current);
        }
        const failed = { ...running, completedAt: now().toISOString(), status: "failed", error: caught instanceof Error ? caught.message.slice(0, 1000) : "analysis failed", ...(enrichment ? { enrichment: { ...enrichment, status: "failed" as const } } : {}) } as const;
        const staleBrief = store.loadBrief();
        if (staleBrief) store.saveBrief({ ...staleBrief, status: "stale", error: failed.error, updatedAt: now().toISOString() });
        store.saveAnalysisRun(failed);
        throw caught;
      } finally { analysisInFlight = undefined; }
    })();
    return analysisInFlight;
  };

  // One cached model per epic feeds both the TV HTML (/focus/:ref) and the frame JSON (/api/focus/:ref).
  const focusCache = new Map<string, { at: number; model: FocusModel }>();
  const renderFocus = async (ref: string, render: (model: FocusModel) => Response): Promise<Response> => {
    const cached = focusCache.get(ref);
    if (cached && now().getTime() - cached.at < FOCUS_CACHE_MS) return render(cached.model);
    const text = (body: string, status: number) => new Response(body, { status, headers: { "Content-Type": "text/plain; charset=utf-8" } });
    if (!options.linear?.fetchEpic) return text("Linear source is not configured", 503);
    try {
      const epic = await options.linear.fetchEpic(ref);
      if (!epic) return text(`Unknown issue ${ref}`, 404);
      const model = buildFocusModel(epic, readFocusMetrics(options.dataPath), now());
      focusCache.set(ref, { at: now().getTime(), model });
      return render(model);
    } catch (caught) {
      console.error("Linear graph focus view failed", caught);
      return text(`Linear request failed: ${redactSensitiveText(caught instanceof Error ? caught.message : "unknown error").slice(0, 300)}`, 502);
    }
  };
  const focusHtml = (pageUrl: string) => (model: FocusModel) => new Response(renderFocusModelHtml(model, pageUrl), { headers: { "Content-Type": "text/html; charset=utf-8", "Cache-Control": "no-store" } });

  const server = Bun.serve({
    hostname: options.hostname ?? "127.0.0.1", port: options.port ?? 8790,
    async fetch(request) {
      const url = new URL(request.url);
      if (url.pathname === "/health" && request.method === "GET") {
        const fresh = freshness(current, store.latestAnalysisRun(), now());
        return json({ ok: true, snapshotAt: current?.generatedAt ?? null, syncedAt: fresh.syncedAt, ageSeconds: fresh.ageSeconds, stale: fresh.stale });
      }
      if (url.pathname === "/api/freshness" && request.method === "GET") {
        const lastRun = store.latestAnalysisRun();
        const fresh = freshness(current, lastRun, now());
        return json({ ...fresh, lastRun: fresh.lastRun ? { ...fresh.lastRun, ...(lastRun?.enrichment ? { enrichment: lastRun.enrichment } : {}) } : null });
      }
      if (url.pathname === "/api/next-up" && request.method === "GET") {
        if (!current) return error("No snapshot available", 503);
        const limit = Math.min(50, Math.max(1, Number(url.searchParams.get("limit")) || 10));
        return json(nextUp(current, store.loadBrief(), store.latestAnalysisRun(), now(), limit));
      }
      if (url.pathname === "/api/graph" && request.method === "GET") {
        if (!current) return error("No snapshot available", 503);
        const membership = projectMembership(current, url.searchParams.get("history") === "true");
        const selected = url.searchParams.get("view") === "focus" ? focusSnapshot(membership) : membership;
        const snapshot = { ...selected, nodes: selected.nodes.map((node) => { const assignment = store.getTheme(node.id, themeInputHash(node.title)); return { ...node, theme: assignment?.theme ?? "pending", ...(assignment ? { themeSuggestion: assignment.suggested, themeConfidence: assignment.confidence } : {}) }; }) };
        return json({ snapshot, brief: store.loadBrief(), analysis: store.latestAnalysisRun(), drain: updateDrainMetrics(), themes: { enabled: Boolean(options.themes), running: Boolean(themeJob), error: themeError } });
      }
      if (url.pathname === "/api/focus-brief" && request.method === "GET") return json(store.loadBrief() ?? { text: "", updatedAt: now().toISOString(), status: "stale", source: "fallback", error: "No brief has been generated yet" });
      if (url.pathname === "/api/analysis-runs/latest" && request.method === "GET") return json(store.latestAnalysisRun());
      if (url.pathname === "/api/codex/status" && request.method === "GET") return json(await codexAuth.status());
      if (url.pathname === "/api/work-drain" && request.method === "GET") {
        if (!current) return error("No snapshot available", 503);
        return json({ metrics: updateDrainMetrics(), campaigns: store.loadCampaigns(), decisions: store.loadTriageDecisions() });
      }
      if (url.pathname === "/api/campaigns" && request.method === "GET") return json(store.loadCampaigns());
      if (url.pathname === "/api/triage-decisions" && request.method === "GET") return json(store.loadTriageDecisions());
      if (url.pathname === "/api/executions" && request.method === "GET") return json(store.loadExecutionRuns());
      const executionMatch = /^\/api\/bundles\/([^/]+)\/executions$/.exec(url.pathname);
      if (executionMatch && request.method === "GET") {
        return json(store.loadExecutionRuns().filter((run) => run.bundleId === executionMatch[1]));
      }
      const campaignMatch = /^\/api\/campaigns\/([^/]+)$/.exec(url.pathname);
      if (campaignMatch && request.method === "GET") return json(store.loadCampaign(campaignMatch[1]) ?? { error: "Campaign not found" }, store.loadCampaign(campaignMatch[1]) ? 200 : 404);
      const focusMatch = /^\/focus\/([A-Z]+-\d+)$/.exec(url.pathname);
      if (focusMatch && request.method === "GET") return renderFocus(focusMatch[1], focusHtml(`${url.host}${url.pathname}${url.search}`));
      const focusApiMatch = /^\/api\/focus\/([A-Z]+-\d+)$/.exec(url.pathname);
      if (focusApiMatch && request.method === "GET") return renderFocus(focusApiMatch[1], (model) => json(model));
      if (request.method === "GET") {
        const relativePath = url.pathname === "/" ? "index.html" : url.pathname.replace(/^\/+/, "");
        const root = resolve(webDistPath);
        const filePath = resolve(root, relativePath);
        if (filePath === root || filePath.startsWith(`${root}${sep}`)) {
          const file = Bun.file(filePath);
          if (await file.exists()) return new Response(file);
        }
      }
      if (!validBearer(request, options.actionToken) && !trustedNonDestructiveMutation(request, url.pathname)) return error("Unauthorized", 401);
      if (url.pathname === "/api/analysis-runs" && request.method === "POST") {
        try {
          void runAnalysis().catch((caught) => console.error("Linear graph analysis failed", caught));
          return json(store.latestAnalysisRun() ?? { status: "running" }, 202);
        } catch (caught) { return error(caught instanceof Error ? caught.message : "analysis failed", 502); }
      }
      if (url.pathname === "/api/codex/login" && request.method === "POST") {
        const status = await codexAuth.startDeviceLogin();
        return json(status, status.mode === "api-key" ? 409 : status.status === "failed" ? 502 : 202);
      }
      if (url.pathname === "/api/campaigns" && request.method === "POST") {
        try {
          if (!current) return error("No snapshot available", 503);
          const body = await parseBody(request, campaignRequestSchema);
          const campaign = proposeCampaign(current, body, now(), store.loadTriageDecisions());
          store.saveCampaign(campaign);
          updateDrainMetrics();
          return json(campaign, 201);
        } catch { return error("Invalid campaign request", 400); }
      }
      if (url.pathname === "/api/triage-decisions" && request.method === "POST") {
        try {
          if (!current) return error("No snapshot available", 503);
          const body = await parseBody(request, triageDecisionRequestSchema);
          if (!current.nodes.some((node) => node.id === body.issueId)) return error("Unknown issue", 404);
          const timestamp = now().toISOString();
          const decision: TriageDecision = { ...body, createdAt: store.loadTriageDecision(body.issueId)?.createdAt ?? timestamp, updatedAt: timestamp };
          store.saveTriageDecision(decision);
          updateDrainMetrics();
          return json(decision, 201);
        } catch { return error("Invalid triage decision", 400); }
      }
      const bundleMatch = /^\/api\/bundles\/([^/]+)\/approval$/.exec(url.pathname);
      if (bundleMatch && request.method === "POST") {
        try {
          const body = await parseBody(request, bundleApprovalRequestSchema);
          const timestamp = now().toISOString();
          const result = store.approveBundle(bundleMatch[1], body.decision, body.note, timestamp);
          if (!result.ok) return error(result.reason === "not_found" ? "Bundle not found" : "Bundle changed; refresh before approving", result.reason === "not_found" ? 404 : 409);
          updateDrainMetrics();
          return json(result.bundle);
        } catch { return error("Invalid bundle approval", 400); }
      }
      const resolutionMatch = /^\/api\/resolution-sets\/([^/]+)\/approval$/.exec(url.pathname);
      if (resolutionMatch && request.method === "POST") {
        try {
          const body = await parseBody(request, bundleApprovalRequestSchema);
          const timestamp = now().toISOString();
          const result = store.approveResolution(resolutionMatch[1], body.decision, timestamp);
          if (!result.ok) return error(result.reason === "not_found" ? "Resolution set not found" : "Resolution set changed; refresh before approving", result.reason === "not_found" ? 404 : 409);
          updateDrainMetrics();
          return json(result.resolution);
        } catch { return error("Invalid resolution approval", 400); }
      }
      if (executionMatch && request.method === "POST") {
        if (codexAuthMode === "api-key") return error("Autonomous workspace-write execution requires subscription authentication; API-key mode is analysis-only", 409);
        if (!options.execution) return error("Headless execution is not configured on this VPS", 503);
        try {
          if (!current) return error("No snapshot available", 503);
          const body = await parseBody(request, executionRequestSchema);
          const campaign = store.loadCampaigns().find((candidate) => candidate.bundles.some((bundle) => bundle.id === executionMatch[1]));
          const bundle = campaign?.bundles.find((candidate) => candidate.id === executionMatch[1]);
          if (!campaign || !bundle) return error("Bundle not found", 404);
          if (bundle.status !== "approved" || !bundle.approvedAt) return error("Bundle must be approved before execution", 409);
          if (!options.execution.repositoryAllowlist[body.repository]) return error("Repository is not in the execution allowlist", 400);
          const timestamp = now().toISOString();
          const instruction = body.instruction ? redactSensitiveText(body.instruction.slice(0, 2_000)) : undefined;
          const executionId = `execution:${crypto.randomUUID()}`;
          const intent: ExecutionRun = { id: executionId, bundleId: bundle.id, repository: body.repository, branch: null, worktreePath: null, status: "queued", startedAt: timestamp, completedAt: null, exitCode: null, stdout: "", stderr: "", evidence: [{ kind: "runner", text: "Execution intent persisted before Codex start." }], error: null, instruction, issueIds: bundle.issueIds, createdAt: timestamp };
          const claimed = store.claimBundleExecution(bundle.id, intent);
          if (!claimed.ok) return error(claimed.reason === "not_found" ? "Bundle not found" : "Bundle is already running or changed; refresh before retrying", claimed.reason === "not_found" ? 404 : 409);
          const issueContext = current.nodes.filter((issue) => claimed.bundle.issueIds.includes(issue.id)).map((issue) => ({ identifier: issue.identifier, title: issue.title, description: issue.description }));
          const result = await executeApprovedBundle({ executionId, bundle: { ...claimed.bundle, status: "approved", approvedAt: claimed.bundle.approvedAt }, repository: body.repository, issueContext, instruction }, options.execution);
          const execution: ExecutionRun = {
            id: result.id, bundleId: result.bundleId, repository: result.repository, branch: result.branch, worktreePath: result.worktreePath,
            status: result.status, startedAt: result.startedAt, completedAt: result.completedAt, exitCode: result.exitCode,
            stdout: result.stdout, stderr: result.stderr, evidence: [{ kind: "runner", text: "Execution intent persisted before Codex start." }, ...result.evidence], error: result.error, instruction, issueIds: bundle.issueIds, createdAt: timestamp,
          };
          const finalStatus: WorkBundle["status"] = result.status === "completed" ? "verified" : result.status === "timed_out" ? "blocked" : "failed";
          const campaignStatus: Campaign["status"] = result.status === "completed" && claimed.campaign.bundles.every((candidate) => candidate.id === bundle.id || ["verified", "rejected"].includes(candidate.status)) ? "completed" : result.status === "failed" ? "failed" : "running";
          store.completeBundleExecution(claimed.campaign.id, bundle.id, execution, finalStatus, campaignStatus);
          updateDrainMetrics();
          return json({ execution, campaignId: campaign.id }, result.status === "completed" ? 202 : 502);
        } catch (caught) { return error(caught instanceof Error ? caught.message : "Execution failed", 502); }
      }
      if (url.pathname === "/api/focus-brief" && request.method === "PUT") {
        try { const body = await parseBody(request, updateFocusBriefRequestSchema); const timestamp = now().toISOString(); const brief = focusBriefSchema.parse({ ...body, updatedAt: timestamp, generatedAt: timestamp, status: "ready", source: "manual", error: null }); store.saveBrief(brief); if (current) { current = graphSnapshotSchema.parse({ ...current, recommendations: rankedRecommendations(current.nodes, current.edges, brief) }); store.saveSnapshot(current); } return json(brief); } catch { return error("Invalid focus brief", 400); }
      }
      const topicMatch = /^\/api\/issues\/([^/]+)\/topic$/.exec(url.pathname);
      if (topicMatch && request.method === "PUT") {
        try {
          const body = await parseBody(request, updateTopicRequestSchema);
          if (!DEFAULT_ZONES.some((zone) => zone.id === body.zone)) return error("Unknown topic", 400);
          if (!current?.nodes.some((node) => node.id === topicMatch[1])) return error("Unknown issue", 404);
          store.setTopicOverride(topicMatch[1], body.zone);
          if (current) { current = graphSnapshotSchema.parse({ ...current, nodes: current.nodes.map((node) => node.id === topicMatch[1] ? { ...node, zone: body.zone } : node) }); store.saveSnapshot(current); }
          return json({ issueId: topicMatch[1], zone: body.zone });
        } catch { return error("Invalid topic", 400); }
      }
      return error("Not found", 404);
    },
  });
  return { url: `http://${options.hostname ?? "127.0.0.1"}:${server.port}`, store, runAnalysis, async stop() { stopping = true; if (themeTimer) clearInterval(themeTimer); server.stop(); await themeJob; if (!options.store) store.close(); } };
}
