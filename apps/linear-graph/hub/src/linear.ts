import type { GraphEdge, Issue, ZoneId } from "@commonkit/linear-graph-protocol";
import { normalizeLinearEdges, normalizeLinearIssue, summarizeTeams } from "./normalizer.js";

export interface LinearPage {
  nodes: Record<string, unknown>[];
  pageInfo?: { hasNextPage?: boolean; endCursor?: string | null };
}

export interface EpicChild {
  identifier: string; title: string; url: string; dueDate: string | null; priority: number;
  state: { name: string; type: string };
  labels: { nodes: Array<{ name: string }> };
  comments: { nodes: Array<{ body: string; createdAt: string }> };
}
export interface Epic { identifier: string; title: string; description: string | null; url: string; children: { nodes: EpicChild[] } }

export interface LinearSource {
  fetchIssues(cursor?: string): Promise<LinearPage>;
  /** One issue and its direct children; null when Linear does not know the ref. */
  fetchEpic?(ref: string): Promise<Epic | null>;
  repoMap?: { teams?: Record<string, string>; projects?: Record<string, string> };
}

const QUERY = `query GraphIssues($after: String) {
  issues(first: 100, after: $after, orderBy: updatedAt) {
    nodes { id identifier title description url createdAt updatedAt completedAt dueDate priority
      team { id key name } state { id name type } project { id name url }
      cycle { id name number } parent { id } labels { nodes { name } }
      relations { nodes { type issue { id } } }
      inverseRelations { nodes { type issue { id } } }
    }
    pageInfo { hasNextPage endCursor }
  }
}`;

const EPIC_QUERY = `query FocusEpic($ref: String!) {
  issue(id: $ref) { identifier title description url
    children { nodes { identifier title url dueDate priority state { name type } labels { nodes { name } } comments { nodes { body createdAt } } } }
  }
}`;

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

export function createLinearSource(options: { token: string; endpoint?: string; fetcher?: typeof fetch; repoMap?: LinearSource["repoMap"]; retryDelayMs?: number; maxRetries?: number }): LinearSource {
  const request = options.fetcher ?? fetch;
  const endpoint = options.endpoint ?? "https://api.linear.app/graphql";
  async function post<T>(query: string, variables: Record<string, unknown>): Promise<T | undefined> {
    let response: Response;
    // Linear rate-limits with 429, or 400 + RATELIMITED. Back off instead of failing the
    // whole daily sync on one page (USG-383: 22 days lost to 400s, 7 to 429s).
    for (let attempt = 0; ; attempt += 1) {
      response = await request(endpoint, {
        method: "POST",
        headers: { "Authorization": options.token, "Content-Type": "application/json" },
        body: JSON.stringify({ query, variables }),
      });
      if (response.ok) break;
      const detail = (await response.text().catch(() => "")).slice(0, 300);
      const limited = response.status === 429 || (response.status === 400 && detail.includes("RATELIMITED"));
      if (!limited || attempt >= (options.maxRetries ?? 3)) throw new Error(`Linear request failed (${response.status})${detail ? `: ${detail}` : ""}`);
      const retryAfter = Number(response.headers.get("Retry-After"));
      await sleep(Math.min(300_000, Number.isFinite(retryAfter) && retryAfter > 0 ? retryAfter * 1000 : (options.retryDelayMs ?? 60_000) * 2 ** attempt));
    }
    const body = await response.json() as { errors?: Array<{ message?: string }>; data?: T };
    if (body.errors?.length) throw new Error(`Linear GraphQL error: ${body.errors.map((error) => error.message ?? "unknown").join("; ")}`);
    return body.data;
  }
  return {
    repoMap: options.repoMap,
    async fetchIssues(cursor) {
      const data = await post<{ issues?: LinearPage }>(QUERY, { after: cursor ?? null });
      if (!data?.issues) throw new Error("Linear response did not contain issues");
      return data.issues;
    },
    async fetchEpic(ref) {
      try {
        return (await post<{ issue?: Epic | null }>(EPIC_QUERY, { ref }))?.issue ?? null;
      } catch (caught) {
        if (caught instanceof Error && /not found/i.test(caught.message)) return null;
        throw caught;
      }
    },
  };
}

export interface SyncResult { issues: Issue[]; edges: GraphEdge[]; teams: ReturnType<typeof summarizeTeams>; syncedAt: string; rawCount: number }

export async function syncLinear(source: LinearSource, overrides = new Map<string, ZoneId>()): Promise<SyncResult> {
  const raw: Record<string, unknown>[] = [];
  let cursor: string | undefined;
  for (;;) {
    const page = await source.fetchIssues(cursor);
    raw.push(...page.nodes);
    if (!page.pageInfo?.hasNextPage || !page.pageInfo.endCursor) break;
    cursor = page.pageInfo.endCursor;
  }
  const issues = raw.map((item) => {
    const team = item.team as Record<string, unknown> | undefined;
    const project = item.project as Record<string, unknown> | null | undefined;
    const repo = source.repoMap?.projects?.[typeof project?.id === "string" ? project.id : ""]
      ?? source.repoMap?.teams?.[typeof team?.key === "string" ? team.key : ""];
    return normalizeLinearIssue(repo ? { ...item, repo } : item, overrides);
  });
  return { issues, edges: normalizeLinearEdges(raw, issues), teams: summarizeTeams(issues), syncedAt: new Date().toISOString(), rawCount: raw.length };
}
