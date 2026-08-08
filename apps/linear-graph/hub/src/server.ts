import { timingSafeEqual } from "node:crypto";
import { mkdirSync } from "node:fs";
import { dirname, join, resolve, sep } from "node:path";
import { z } from "zod";
import { DEFAULT_ZONES, focusBriefSchema, graphSnapshotSchema, updateFocusBriefRequestSchema, updateTopicRequestSchema, type AnalysisRun, type GraphSnapshot } from "@commonkit/linear-graph-protocol";
import { applyCodexAnalysis, defaultRecommendations, focusSnapshot } from "./analysis.js";
import { runCodexAnalysis, type CodexRunnerOptions } from "./codex.js";
import { syncLinear, type LinearSource } from "./linear.js";
import { GraphStore } from "./store.js";

export interface GraphHubOptions {
  port?: number;
  hostname?: string;
  actionToken: string;
  store?: GraphStore;
  dataPath?: string;
  linear?: LinearSource;
  codex?: CodexRunnerOptions;
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

function parseBody<T>(request: Request, schema: z.ZodType<T>): Promise<T> {
  return request.json().then((body) => schema.parse(body));
}

export function startGraphHub(options: GraphHubOptions): GraphHub {
  const now = options.now ?? (() => new Date());
  if (options.dataPath) mkdirSync(dirname(options.dataPath), { recursive: true });
  const store = options.store ?? new GraphStore(options.dataPath ?? ":memory:");
  const webDistPath = options.webDistPath ?? join(import.meta.dir, "../../web/dist");
  let current = store.loadSnapshot();
  let analysisInFlight: Promise<AnalysisRun> | undefined;

  const runAnalysis = async (): Promise<AnalysisRun> => {
    if (analysisInFlight) return analysisInFlight;
    analysisInFlight = (async () => {
      const startedAt = now().toISOString();
      const id = crypto.randomUUID();
      const running = { id, startedAt, completedAt: null, status: "running", issueCount: 0, error: null, inputHash: null } as const;
      store.saveAnalysisRun(running);
      let fallback: GraphSnapshot | null = null;
      try {
        if (!options.linear) throw new Error("Linear source is not configured");
        const sync = await syncLinear(options.linear, store.topicOverrides());
        const baseline: GraphSnapshot = graphSnapshotSchema.parse({ generatedAt: now().toISOString(), syncedAt: sync.syncedAt, stale: false, nodes: sync.issues, edges: sync.edges, teams: sync.teams, zones: DEFAULT_ZONES, recommendations: defaultRecommendations(sync.issues), latestAnalysisRunId: id });
        fallback = baseline;
        const result = options.codex ? await runCodexAnalysis({ issues: sync.issues, edges: sync.edges, focusBrief: store.loadBrief()?.text }, options.codex) : { assignments: [], semanticEdges: [], recommendations: baseline.recommendations };
        const applied = applyCodexAnalysis(sync.issues, sync.edges, result, sync.syncedAt, id);
        const snapshot = { ...applied.snapshot, zones: baseline.zones, recommendations: applied.snapshot.recommendations.length ? applied.snapshot.recommendations : baseline.recommendations };
        store.saveSnapshot(snapshot);
        current = snapshot;
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
        return json({ snapshot, brief: store.loadBrief(), analysis: store.latestAnalysisRun() });
      }
      if (url.pathname === "/api/focus-brief" && request.method === "GET") return json(store.loadBrief() ?? { text: "", updatedAt: now().toISOString() });
      if (url.pathname === "/api/analysis-runs/latest" && request.method === "GET") return json(store.latestAnalysisRun());
      if (request.method === "GET") {
        const relativePath = url.pathname === "/" ? "index.html" : url.pathname.replace(/^\/+/, "");
        const root = resolve(webDistPath);
        const filePath = resolve(root, relativePath);
        if (filePath === root || filePath.startsWith(`${root}${sep}`)) {
          const file = Bun.file(filePath);
          if (await file.exists()) return new Response(file);
        }
      }
      if (!validBearer(request, options.actionToken)) return error("Unauthorized", 401);
      if (url.pathname === "/api/analysis-runs" && request.method === "POST") {
        try { return json(await runAnalysis(), 202); } catch (caught) { return error(caught instanceof Error ? caught.message : "analysis failed", 502); }
      }
      if (url.pathname === "/api/focus-brief" && request.method === "PUT") {
        try { const body = await parseBody(request, updateFocusBriefRequestSchema); const brief = focusBriefSchema.parse({ ...body, updatedAt: now().toISOString() }); store.saveBrief(brief); return json(brief); } catch { return error("Invalid focus brief", 400); }
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
