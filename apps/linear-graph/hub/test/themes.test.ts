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

test("three failing issues are deferred across restart so later issues are classified", async () => {
  const store = new GraphStore(":memory:");
  const nodes = ["a", "b", "c", "d"].map((id) => normalizeLinearIssue({ id, identifier: `USG-${id}`, title: `Issue ${id}`, url: `https://linear.app/x/USG-${id}`, team: { id: "t", key: "USG", name: "Unsold" }, state: { id: "s", name: "Todo", type: "unstarted" }, createdAt: "2026-01-01T00:00:00Z", updatedAt: "2026-01-01T00:00:00Z" }));
  const date = "2026-01-01T00:00:00.000Z";
  store.saveSnapshot({ generatedAt: date, syncedAt: date, stale: false, nodes, edges: [], teams: [], zones: [...DEFAULT_ZONES], recommendations: [], latestAnalysisRunId: null });
  const config = { gatewayUrl: "https://gateway.ai.cloudflare.com/v1/account/default/workers-ai/@cf/cloudflare/clef", token: "test" };
  let attempts = 0, third!: () => void;
  const firstPass = new Promise<void>((resolve) => { third = resolve; });
  const first = startGraphHub({ actionToken: "test", port: 0, store, now: () => new Date(date), themes: { ...config, fetch: (async () => { if (++attempts === 3) third(); return new Response("bad", { status: 500 }); }) as unknown as typeof fetch } });
  await firstPass;
  await first.stop();
  expect(attempts).toBe(3);
  for (const node of nodes.slice(0, 3)) expect(store.themeRetryAfter(node.id, themeInputHash(node.title))).toBe(Date.parse(date) + 3_600_000);
  let secondAttempts = 0, fourth!: () => void;
  const secondPass = new Promise<void>((resolve) => { fourth = resolve; });
  const second = startGraphHub({ actionToken: "test", port: 0, store, now: () => new Date(date), themes: { ...config, fetch: (async () => { secondAttempts++; fourth(); return Response.json(answer("checkout", 0.9)); }) as unknown as typeof fetch } });
  try {
    await secondPass;
    await second.stop();
    expect(secondAttempts).toBe(1);
    expect(store.getTheme(nodes[3]!.id, themeInputHash(nodes[3]!.title))?.theme).toBe("checkout");
    expect(store.themeRetryAfter(nodes[0]!.id, themeInputHash("Changed title"))).toBeNull();
    let retried = 0, retriedThree!: () => void;
    const retryPass = new Promise<void>((resolve) => { retriedThree = resolve; });
    const retry = startGraphHub({ actionToken: "test", port: 0, store, now: () => new Date(Date.parse(date) + 3_600_000), themes: { ...config, fetch: (async () => { if (++retried === 3) retriedThree(); return new Response("bad", { status: 500 }); }) as unknown as typeof fetch } });
    await retryPass;
    await retry.stop();
    expect(retried).toBe(3);
    expect(store.themeRetryAfter(nodes[0]!.id, themeInputHash(nodes[0]!.title))).toBe(Date.parse(date) + 7_200_000);
  } finally { store.close(); }
});

test("sends bounded title only through the gateway with logging disabled", async () => {
  let request: RequestInit | undefined;
  const result = await classifyTheme("x".repeat(1000), { gatewayUrl: "https://gateway.ai.cloudflare.com/v1/account/default/workers-ai/@cf/cloudflare/clef", token: "test", fetch: (async (_url, init) => { request = init; return Response.json(answer("data", 0.8)); }) as typeof fetch });
  expect(result.theme).toBe("data");
  expect(JSON.parse(request!.body as string).state.length).toBe(500);
  expect((request!.headers as Record<string, string>)["cf-aig-skip-log"]).toBe("true");
  await expect(classifyTheme("x", { gatewayUrl: "https://evil.example/v1/account/default/workers-ai/@cf/cloudflare/clef", token: "test" })).rejects.toThrow();
});
