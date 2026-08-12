import type { GraphEdge, Issue, ZoneId } from "@commonkit/linear-graph-protocol";
import { normalizeLinearEdges, normalizeLinearIssue, summarizeTeams } from "./normalizer.js";

export interface LinearPage {
  nodes: Record<string, unknown>[];
  pageInfo?: { hasNextPage?: boolean; endCursor?: string | null };
}

export interface LinearSource {
  fetchIssues(cursor?: string): Promise<LinearPage>;
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

export function createLinearSource(options: { token: string; endpoint?: string; fetcher?: typeof fetch; repoMap?: LinearSource["repoMap"] }): LinearSource {
  const request = options.fetcher ?? fetch;
  const endpoint = options.endpoint ?? "https://api.linear.app/graphql";
  return {
    repoMap: options.repoMap,
    async fetchIssues(cursor) {
      const response = await request(endpoint, {
        method: "POST",
        headers: { "Authorization": options.token, "Content-Type": "application/json" },
        body: JSON.stringify({ query: QUERY, variables: { after: cursor ?? null } }),
      });
      if (!response.ok) throw new Error(`Linear request failed (${response.status})`);
      const body = await response.json() as { errors?: Array<{ message?: string }>; data?: { issues?: LinearPage } };
      if (body.errors?.length) throw new Error(`Linear GraphQL error: ${body.errors.map((error) => error.message ?? "unknown").join("; ")}`);
      if (!body.data?.issues) throw new Error("Linear response did not contain issues");
      return body.data.issues;
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
