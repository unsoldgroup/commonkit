import { timingSafeEqual } from "node:crypto";
import { mkdirSync } from "node:fs";
import { dirname, join, resolve, sep } from "node:path";
import { z } from "zod";
import {
  DEFAULT_ZONES, bundleApprovalRequestSchema, campaignRequestSchema, focusBriefSchema, graphSnapshotSchema,
  executionRequestSchema, triageDecisionRequestSchema, updateFocusBriefRequestSchema, updateTopicRequestSchema,
  type AnalysisRun, type Campaign, type ExecutionRun, type GraphSnapshot, type TriageDecision, type WorkBundle,
} from "@commonkit/linear-graph-protocol";
import { applyCodexAnalysis, defaultRecommendations, focusSnapshot } from "./analysis.js";
import { createCodexAuthManager, type CodexAuthOptions } from "./codex-auth.js";
import { runCodexAnalysis, type CodexRunnerOptions } from "./codex.js";
import { computeDrainMetrics, proposeCampaign } from "./drain.js";
import { executeApprovedBundle, type ExecutionRunnerOptions } from "./execution.js";
import { syncLinear, type LinearSource } from "./linear.js";
import { inferTopicAssignment, summarizeTeams } from "./normalizer.js";
import { redactSensitiveText } from "./redaction.js";
import { GraphStore } from "./store.js";

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

function parseBody<T>(request: Request, schema: z.ZodType<T>): Promise<T> {
  return request.json().then((body) => schema.parse(body));
}

export function startGraphHub(options: GraphHubOptions): GraphHub {
  const now = options.now ?? (() => new Date());
  if (options.dataPath) mkdirSync(dirname(options.dataPath), { recursive: true });
  const store = options.store ?? new GraphStore(options.dataPath ?? ":memory:");
  const webDistPath = options.webDistPath ?? join(import.meta.dir, "../../web/dist");
  const codexAuth = createCodexAuthManager(options.codexAuth);
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
      try {
        if (!options.linear) throw new Error("Linear source is not configured");
        const sync = await syncLinear(options.linear, store.topicOverrides());
        const baseline: GraphSnapshot = graphSnapshotSchema.parse({ generatedAt: now().toISOString(), syncedAt: sync.syncedAt, stale: false, nodes: sync.issues, edges: sync.edges, teams: sync.teams, zones: DEFAULT_ZONES, recommendations: defaultRecommendations(sync.issues), latestAnalysisRunId: id });
        fallback = baseline;
        const result = options.codex ? await runCodexAnalysis({ issues: sync.issues, edges: sync.edges, focusBrief: store.loadBrief()?.text }, options.codex) : { assignments: [], semanticEdges: [], recommendations: baseline.recommendations, triageDecisions: [] };
        if (options.codex && !result.brief) throw new Error("Codex returned no daily brief");
        const applied = applyCodexAnalysis(sync.issues, sync.edges, result, sync.syncedAt, id);
        const snapshot = { ...applied.snapshot, zones: baseline.zones, recommendations: applied.snapshot.recommendations.length ? applied.snapshot.recommendations : baseline.recommendations };
        store.saveSnapshot(snapshot);
        current = snapshot;
        const codexTriage = result.triageDecisions ?? [];
        if (options.codex && result.brief) {
          const timestamp = now().toISOString();
          store.saveBrief({ text: result.brief, updatedAt: timestamp, generatedAt: timestamp, snapshotAt: snapshot.syncedAt, status: "ready", source: "codex", error: null });
          for (const decision of codexTriage) {
            const previous = store.loadTriageDecision(decision.issueId);
            store.saveTriageDecision({ ...decision, createdAt: previous?.createdAt ?? timestamp, updatedAt: timestamp, source: "codex" });
          }
        } else if (!existingBrief) {
          const activeCount = snapshot.nodes.filter((node) => !["completed", "canceled"].includes(node.status.type)).length;
          store.saveBrief({ text: `Universe synced: ${activeCount} active issues are ready for triage and bundle planning.`, updatedAt: now().toISOString(), generatedAt: now().toISOString(), snapshotAt: snapshot.syncedAt, status: "ready", source: "fallback", error: null });
        } else {
          store.saveBrief({ ...store.loadBrief()!, status: "ready", error: null, updatedAt: now().toISOString(), generatedAt: now().toISOString(), snapshotAt: snapshot.syncedAt });
        }
        updateDrainMetrics();
        const completed = { ...running, completedAt: now().toISOString(), status: "completed", issueCount: sync.rawCount, inputHash: applied.inputHash } as const;
        store.saveAnalysisRun(completed);
        return completed;
      } catch (caught) {
        const staleSnapshot = current ?? fallback;
        if (staleSnapshot) {
          current = graphSnapshotSchema.parse({ ...staleSnapshot, stale: true });
          store.saveSnapshot(current);
        }
        const failed = { ...running, completedAt: now().toISOString(), status: "failed", error: caught instanceof Error ? caught.message.slice(0, 1000) : "analysis failed" } as const;
        const staleBrief = store.loadBrief();
        if (staleBrief) store.saveBrief({ ...staleBrief, status: "stale", error: failed.error, updatedAt: now().toISOString() });
        store.saveAnalysisRun(failed);
        throw caught;
      } finally { analysisInFlight = undefined; }
    })();
    return analysisInFlight;
  };

  const server = Bun.serve({
    hostname: options.hostname ?? "127.0.0.1", port: options.port ?? 8790,
    async fetch(request) {
      const url = new URL(request.url);
      if (url.pathname === "/health" && request.method === "GET") return json({ ok: true, snapshotAt: current?.generatedAt ?? null, stale: current?.stale ?? true });
      if (url.pathname === "/api/graph" && request.method === "GET") {
        if (!current) return error("No snapshot available", 503);
        const snapshot = url.searchParams.get("view") === "focus" ? focusSnapshot(current) : current;
        return json({ snapshot, brief: store.loadBrief(), analysis: store.latestAnalysisRun(), drain: updateDrainMetrics() });
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
        try { const body = await parseBody(request, updateFocusBriefRequestSchema); const timestamp = now().toISOString(); const brief = focusBriefSchema.parse({ ...body, updatedAt: timestamp, generatedAt: timestamp, status: "ready", source: "manual", error: null }); store.saveBrief(brief); return json(brief); } catch { return error("Invalid focus brief", 400); }
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
  return { url: `http://${options.hostname ?? "127.0.0.1"}:${server.port}`, store, runAnalysis, async stop() { server.stop(); if (!options.store) store.close(); } };
}
