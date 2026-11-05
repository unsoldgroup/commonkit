import { expect, test } from "bun:test";
import { classifyTheme, parseTheme, themeInputHash } from "../src/themes.js";
import { GraphStore } from "../src/store.js";
import { startGraphHub } from "../src/server.js";
import { normalizeLinearIssue } from "../src/normalizer.js";
import { DEFAULT_ZONES } from "@commonkit/linear-graph-protocol";

const answer = (choice: string, confidence: number) => ({ success: true, result: { answers: { theme: { choice, confidence } } } });

test("validates the Clef response and leaves uncertain work unclear", () => {
  expect(parseTheme(answer("checkout", 0.8)).theme).toBe("checkout");
  expect(parseTheme(answer("checkout", 0.2))).toEqual({ theme: "unclear", suggested: "checkout", confidence: 0.2 });
  expect(() => parseTheme(answer("toString", 0.9))).toThrow();
  expect(() => parseTheme(answer("checkout", 2))).toThrow();
  expect(themeInputHash("one")).not.toBe(themeInputHash("two"));
});

test("hub persists classifications without changing the snapshot and invalidates changed titles", async () => {
  const store = new GraphStore(":memory:");
  const node = normalizeLinearIssue({ id: "a", identifier: "USG-1", title: "Checkout issue", url: "https://linear.app/x/USG-1", team: { id: "t", key: "USG", name: "Unsold" }, state: { id: "s", name: "Todo", type: "unstarted" }, createdAt: "2026-01-01T00:00:00Z", updatedAt: "2026-01-01T00:00:00Z" });
  const date = "2026-01-01T00:00:00.000Z";
  store.saveSnapshot({ generatedAt: date, syncedAt: date, stale: false, nodes: [node], edges: [], teams: [], zones: [...DEFAULT_ZONES], recommendations: [], latestAnalysisRunId: null });
  let complete!: () => void;
  const called = new Promise<void>((resolve) => { complete = resolve; });
  const hub = startGraphHub({ actionToken: "test", port: 0, store, themes: { gatewayUrl: "https://gateway.ai.cloudflare.com/v1/account/default/workers-ai/@cf/cloudflare/clef", token: "test", fetch: (async () => { complete(); return Response.json(answer("checkout", 0.9)); }) as unknown as typeof fetch } });
  try {
    await called;
    // Stop waits for the job to commit before closing.
    await hub.stop();
    expect(store.getTheme(node.id, themeInputHash(node.title))?.theme).toBe("checkout");
    expect(store.getTheme(node.id, themeInputHash(node.title))?.suggested).toBe("checkout");
    expect(store.getTheme(node.id, themeInputHash("Changed title"))).toBeNull();
    expect(store.loadSnapshot()!.nodes[0]!.theme).toBeUndefined();
  } finally { store.close(); }
});

test("gateway failures leave work pending and expose no provider response", async () => {
  const store = new GraphStore(":memory:");
  const node = normalizeLinearIssue({ id: "a", identifier: "USG-1", title: "Checkout issue", url: "https://linear.app/x/USG-1", team: { id: "t", key: "USG", name: "Unsold" }, state: { id: "s", name: "Todo", type: "unstarted" }, createdAt: "2026-01-01T00:00:00Z", updatedAt: "2026-01-01T00:00:00Z" });
  const date = "2026-01-01T00:00:00.000Z";
  store.saveSnapshot({ generatedAt: date, syncedAt: date, stale: false, nodes: [node], edges: [], teams: [], zones: [...DEFAULT_ZONES], recommendations: [], latestAnalysisRunId: null });
  let called!: () => void;
  const done = new Promise<void>((resolve) => { called = resolve; });
  const hub = startGraphHub({ actionToken: "test", port: 0, store, themes: { gatewayUrl: "https://gateway.ai.cloudflare.com/v1/account/default/workers-ai/@cf/cloudflare/clef", token: "test", fetch: (async () => { called(); return new Response("private provider content", { status: 500 }); }) as unknown as typeof fetch } });
  try {
    await done;
    await new Promise((resolve) => setTimeout(resolve, 0));
    const data = await (await fetch(`${hub.url}/api/graph`)).json() as { snapshot: { nodes: { theme: string }[] }; themes: { error: string } };
    expect(data.snapshot.nodes[0]!.theme).toBe("pending");
    expect(data.themes.error).toBe("Theme grouping failed; retrying later.");
    expect(JSON.stringify(data)).not.toContain("private provider content");
    expect(store.getTheme(node.id, themeInputHash(node.title))).toBeNull();
  } finally { await hub.stop(); store.close(); }
});

test("sends bounded title only through the gateway with logging disabled", async () => {
  let request: RequestInit | undefined;
  const result = await classifyTheme("x".repeat(1000), { gatewayUrl: "https://gateway.ai.cloudflare.com/v1/account/default/workers-ai/@cf/cloudflare/clef", token: "test", fetch: (async (_url, init) => { request = init; return Response.json(answer("data", 0.8)); }) as typeof fetch });
  expect(result.theme).toBe("data");
  expect(JSON.parse(request!.body as string).state.length).toBe(500);
  expect((request!.headers as Record<string, string>)["cf-aig-skip-log"]).toBe("true");
  await expect(classifyTheme("x", { gatewayUrl: "https://evil.example/v1/account/default/workers-ai/@cf/cloudflare/clef", token: "test" })).rejects.toThrow();
});
