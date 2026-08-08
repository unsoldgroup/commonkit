import type { GraphEdge, Issue, ZoneId } from "@commonkit/linear-graph-protocol";
import { normalizeLinearEdges, normalizeLinearIssue, summarizeTeams } from "./normalizer.js";

export interface LinearPage {
  nodes: Record<string, unknown>[];
  pageInfo?: { hasNextPage?: boolean; endCursor?: string | null };
}

export interface LinearProjectMutation {
  createProject(input: { name: string; description: string; teamIds: string[]; issueIds: string[] }): Promise<{ id: string; url?: string }>;
}

export interface LinearSource {
  fetchIssues(cursor?: string): Promise<LinearPage>;
  repoMap?: { teams?: Record<string, string>; projects?: Record<string, string> };
  mutate?: LinearProjectMutation;
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
  const graphql = async <T>(query: string, variables: Record<string, unknown>): Promise<T> => {
    const response = await request(endpoint, { method: "POST", headers: { "Authorization": options.token, "Content-Type": "application/json" }, body: JSON.stringify({ query, variables }) });
    if (!response.ok) throw new Error(`Linear request failed (${response.status})`);
    const body = await response.json() as { errors?: Array<{ message?: string }>; data?: T };
    if (body.errors?.length) throw new Error(`Linear GraphQL error: ${body.errors.map((error) => error.message ?? "unknown").join("; ")}`);
    if (!body.data) throw new Error("Linear response did not contain data");
    return body.data;
  };
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
    mutate: {
      async createProject(input) {
        const data = await graphql<{ projectCreate?: { success?: boolean; project?: { id?: string; url?: string } | null } }>(
          `mutation GraphBundleProject($input: ProjectCreateInput!) { projectCreate(input: $input) { success project { id url } } }`,
          { input: { name: input.name, description: input.description, teamIds: input.teamIds } },
        );
        const project = data.projectCreate?.project;
        if (!data.projectCreate?.success || !project?.id) throw new Error("Linear project creation was not successful");
        for (const issueId of input.issueIds) {
          const result = await graphql<{ issueUpdate?: { success?: boolean } }>(
            `mutation AttachBundleIssue($id: String!, $projectId: String!) { issueUpdate(id: $id, input: { projectId: $projectId }) { success } }`,
            { id: issueId, projectId: project.id },
          );
          if (!result.issueUpdate?.success) throw new Error(`Linear issue ${issueId} could not be attached to the project`);
        }
        return { id: project.id, ...(project.url ? { url: project.url } : {}) };
      },
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
