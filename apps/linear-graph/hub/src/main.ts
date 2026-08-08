import { createLinearSource } from "./linear.js";
import { startGraphHub } from "./server.js";

const required = (name: string) => {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
};

function repoMap() {
  const value = process.env.LINEAR_GRAPH_REPO_MAP;
  if (!value) return undefined;
  const parsed = JSON.parse(value) as unknown;
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) throw new Error("LINEAR_GRAPH_REPO_MAP must be a JSON object");
  return parsed as { teams?: Record<string, string>; projects?: Record<string, string> };
}

function executionRepos(): Record<string, string> | undefined {
  const value = process.env.LINEAR_GRAPH_EXECUTION_REPOS;
  if (!value) return undefined;
  const parsed = JSON.parse(value) as unknown;
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) throw new Error("LINEAR_GRAPH_EXECUTION_REPOS must be a JSON object");
  for (const [key, root] of Object.entries(parsed)) {
    if (!key || typeof root !== "string" || !root.startsWith("/")) throw new Error("LINEAR_GRAPH_EXECUTION_REPOS values must be absolute paths");
  }
  return parsed as Record<string, string>;
}

function nextDailyRun(now: Date, timeZone: string, hour: number): Date {
  const formatter = new Intl.DateTimeFormat("en-CA", { timeZone, year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hourCycle: "h23" });
  const parts = (value: Date) => Object.fromEntries(formatter.formatToParts(value).filter((part) => part.type !== "literal").map((part) => [part.type, Number(part.value)]));
  const current = parts(now);
  const dateEpoch = Date.UTC(current.year, current.month - 1, current.day, hour);
  const targetEpoch = dateEpoch + (current.hour >= hour ? 86_400_000 : 0);
  for (let minute = -180; minute <= 180; minute += 1) {
    const candidate = new Date(targetEpoch + minute * 60_000);
    const candidateParts = parts(candidate);
    const target = new Date(targetEpoch);
    if (candidate > now && candidateParts.year === target.getUTCFullYear() && candidateParts.month === target.getUTCMonth() + 1 && candidateParts.day === target.getUTCDate() && candidateParts.hour === hour && candidateParts.minute === 0) return candidate;
  }
  throw new Error(`Could not calculate next ${hour}:00 run in ${timeZone}`);
}

const executionRepoMap = executionRepos();
const hub = startGraphHub({
  actionToken: required("LINEAR_GRAPH_ACTION_TOKEN"),
  dataPath: process.env.LINEAR_GRAPH_DATA_PATH ?? "/var/lib/linear-graph/graph.sqlite",
  hostname: process.env.LINEAR_GRAPH_HOST ?? "127.0.0.1",
  port: Number(process.env.LINEAR_GRAPH_PORT ?? 8790),
  linear: createLinearSource({ token: required("LINEAR_API_TOKEN"), repoMap: repoMap() }),
  codex: { apiKey: process.env.CODEX_API_KEY, model: process.env.LINEAR_GRAPH_CODEX_MODEL },
  execution: executionRepoMap ? { repositoryAllowlist: executionRepoMap, apiKey: process.env.CODEX_API_KEY, model: process.env.LINEAR_GRAPH_CODEX_MODEL } : undefined,
});
console.log(`Linear graph hub listening on ${hub.url}`);

if (process.env.LINEAR_GRAPH_AUTO_ANALYZE !== "0") {
  const latest = hub.store.latestAnalysisRun();
  if (!latest || latest.status !== "completed") {
    void hub.runAnalysis().catch((error) => console.error("Linear graph initial analysis failed", error));
  }
  const scheduleNext = () => {
    const next = nextDailyRun(new Date(), process.env.LINEAR_GRAPH_TIMEZONE ?? "Europe/Madrid", 7);
    setTimeout(() => {
      void hub.runAnalysis().catch((error) => console.error("Linear graph daily analysis failed", error));
      scheduleNext();
    }, Math.max(1_000, next.getTime() - Date.now()));
  };
  scheduleNext();
}
