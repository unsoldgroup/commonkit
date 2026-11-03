import { afterEach, describe, expect, test } from "bun:test";
import { feedStdin } from "../src/codex.js";
import { createLinearSource } from "../src/linear.js";
import { assignRelationZones, frameLabel, freshness, nextUp, rankIssues, shortDescription } from "../src/nextup.js";
import { normalizeLinearEdges, normalizeLinearIssue } from "../src/normalizer.js";
import { startGraphHub } from "../src/server.js";
import { GraphStore } from "../src/store.js";

const raw = (id: string, extra: Record<string, unknown> = {}) => ({
  id, identifier: `USG-${id}`, title: `Issue ${id}`, description: null, url: `https://linear.app/x/USG-${id}`,
  team: { id: "team", key: "USG", name: "Unsold" }, state: { id: "s", name: "Todo", type: "unstarted" },
  priority: 3, createdAt: "2026-10-01T00:00:00.000Z", updatedAt: "2026-10-02T00:00:00.000Z", completedAt: null,
  dueDate: null, project: null, cycle: null, labels: { nodes: [] }, parent: null, relations: { nodes: [] }, inverseRelations: { nodes: [] },
  ...extra,
});
const build = (raws: ReturnType<typeof raw>[]) => {
  const issues = raws.map((item) => normalizeLinearIssue(item));
  return { issues, edges: normalizeLinearEdges(raws, issues) };
};

let hub: ReturnType<typeof startGraphHub> | undefined;
afterEach(async () => { if (hub) await hub.stop(); hub = undefined; });

describe("USG-383 next-up ranking", () => {
  test("ranks pins, then unblocked by Linear priority and unblock count; blocked last", () => {
    const { issues, edges } = build([
      raw("1", { priority: 0 }),
      raw("2", { priority: 1, inverseRelations: { nodes: [{ type: "blocks", issue: { id: "4" } }] } }), // blocked by 4
      raw("3", { priority: 2 }),
      raw("4", { priority: 2, relations: { nodes: [{ type: "blocks", issue: { id: "2" } }] } }),
      raw("5", { priority: 4, state: { id: "d", name: "Done", type: "completed" } }),
    ]);
    const brief = { text: "Today: USG-1 then the rest", updatedAt: "2026-11-03T00:00:00.000Z", source: "manual" as const };
    const order = rankIssues(issues, edges, brief).map((item) => item.issue.identifier);
    expect(order).toEqual(["USG-1", "USG-4", "USG-3", "USG-2"]);
    const blocked = rankIssues(issues, edges).find((item) => item.issue.identifier === "USG-2")!;
    expect(blocked.nextAction).toBe("Clear blocker USG-4 first");
    expect(blocked.whyNow).toContain("blocked by USG-4");
    // A generated brief never pins.
    expect(rankIssues(issues, edges, { ...brief, source: "codex" })[0]!.issue.identifier).toBe("USG-4");
  });

  test("relation-linked issues share one zone; manual overrides win", () => {
    const { issues, edges } = build([
      raw("a", { title: "Deploy the VPS monitoring" }),
      raw("b", { title: "Something vague", parent: { id: "a" } }),
      raw("c", { title: "Rotate OAuth token" }),
    ]);
    const zoned = assignRelationZones(issues, edges);
    expect(zoned.find((i) => i.id === "b")!.zone).toBe(zoned.find((i) => i.id === "a")!.zone);
    expect(zoned.find((i) => i.id === "c")!.zone).toBe("security");
    const overridden = assignRelationZones([{ ...issues[1]!, zone: "documentation" }, issues[0]!], edges);
    expect(overridden[0]!.zone).toBe("documentation");
    expect(overridden[1]!.zone).toBe("documentation");
  });

  test("frame feed strips emails and quote ids and reports staleness past 36h", () => {
    expect(shortDescription("Refund EXP-1UMHD9A41 for jane.doe@example.com per EXP-4663")).toBe("Refund [quote] for [email] per EXP-4663");
    const { issues, edges } = build([raw("1", { parent: { id: "2" } }), raw("2")]);
    const snapshot = { generatedAt: "2026-11-01T00:00:00.000Z", syncedAt: "2026-11-01T00:00:00.000Z", stale: false, nodes: issues, edges, teams: [], zones: [], recommendations: [], latestAnalysisRunId: null };
    const feed = nextUp(snapshot, null, null, new Date("2026-11-02T11:00:00.000Z"), 5);
    expect(feed.stale).toBe(false);
    expect(feed.items[0]).toEqual({ ref: "USG-1", short_description: "Issue 1", why_now: "Medium priority", next_action: "Start work on USG-1", priority: 3, pinned: false, zone: "unsorted", relations: [{ ref: "USG-2", kind: "parent", short_description: "Issue 2" }] });
    expect(freshness(snapshot, null, new Date("2026-11-02T12:00:01.000Z")).stale).toBe(true);
    expect(freshness(null, null, new Date()).stale).toBe(true);
    expect(frameLabel("EXP-9", "Refund Jane Doe jane@x.com EXP-1UMHD9A41")).toBe("EXP-9");
    expect(frameLabel("EXP-9", "Call Jane Doe re EXP-1UMHD9A41")).toBe("EXP-9");
    expect(frameLabel("EXP-9", "Refund Jane Doe [email] [Quote]")).toBe("EXP-9");
    expect(frameLabel("EXP-9", "Fix checkout per EXP-4663")).toBe("Fix checkout per EXP-4663");
    const leaky = build([raw("1", { title: "Refund Jane Doe jane@x.com EXP-1UMHD9A41", parent: { id: "2" } }), raw("2", { title: "Email jane@x.com" })]);
    const leakyFeed = nextUp({ ...snapshot, nodes: leaky.issues, edges: leaky.edges }, null, null, new Date("2026-11-02T11:00:00.000Z"), 5);
    for (const item of leakyFeed.items) {
      expect(item.short_description).toBe(item.ref);
      for (const rel of item.relations) expect(rel.short_description).toBe(rel.ref);
    }
  });
});

describe("USG-383 failure-mode regressions", () => {
  test("a rejected stdin flush is swallowed instead of crashing the process", async () => {
    let caught = false;
    const rejected = Promise.reject(Object.assign(new Error("broken pipe"), { code: "EPIPE" }));
    const original = rejected.catch.bind(rejected);
    rejected.catch = ((handler: (reason: unknown) => unknown) => { caught = true; return original(handler); }) as typeof rejected.catch;
    feedStdin({ write: () => 1, end: () => rejected }, "prompt");
    expect(caught).toBe(true);
    feedStdin({ write: () => { throw new Error("closed"); }, end: () => 0 }, "prompt"); // sync throw tolerated
  });

  test("stuck running analysis runs are closed on open, idempotently", () => {
    const store = new GraphStore(":memory:");
    store.saveAnalysisRun({ id: "done", startedAt: "2026-10-01T00:00:00.000Z", completedAt: "2026-10-01T00:01:00.000Z", status: "completed", issueCount: 1, error: null, inputHash: null });
    store.saveAnalysisRun({ id: "stuck", startedAt: "2026-10-02T00:00:00.000Z", completedAt: null, status: "running", issueCount: 0, error: null, inputHash: null });
    expect(store.closeInterruptedRuns()).toBe(1);
    expect(store.closeInterruptedRuns()).toBe(0);
    const latest = store.latestAnalysisRun()!;
    expect(latest).toMatchObject({ id: "stuck", status: "failed", completedAt: "2026-10-02T00:00:00.000Z" });
    store.close();
  });

  test("Linear rate limits are retried, other errors are not", async () => {
    let calls = 0;
    const ok = { data: { issues: { nodes: [], pageInfo: { hasNextPage: false } } } };
    const source = createLinearSource({ token: "t", retryDelayMs: 1, fetcher: (async () => {
      calls += 1;
      if (calls === 1) return new Response("", { status: 429 });
      if (calls === 2) return new Response(JSON.stringify({ errors: [{ extensions: { code: "RATELIMITED" } }] }), { status: 400 });
      return new Response(JSON.stringify(ok));
    }) as unknown as typeof fetch });
    await source.fetchIssues();
    expect(calls).toBe(3);
    const bad = createLinearSource({ token: "t", retryDelayMs: 1, fetcher: (async () => new Response("Query too complex", { status: 400 })) as unknown as typeof fetch });
    await expect(bad.fetchIssues()).rejects.toThrow("Linear request failed (400): Query too complex");
  });

  test("a failing Codex no longer blocks the Linear snapshot; next-up is served", async () => {
    const raws = [raw("1", { priority: 1 }), raw("2", { parent: { id: "1" } })];
    hub = startGraphHub({
      actionToken: "secret", port: 0,
      linear: { async fetchIssues() { return { nodes: raws, pageInfo: { hasNextPage: false } }; } },
      codex: { spawn: () => ({ stdin: { write() {}, end() {} }, stdout: undefined, stderr: undefined, exited: Promise.resolve(1), kill() {} }) as never },
    });
    const run = await hub.runAnalysis();
    expect(run.status).toBe("completed");
    expect(run.error).toContain("Codex enrichment skipped");
    const feed = await fetch(`${hub.url}/api/next-up?limit=2`).then((response) => response.json());
    expect(feed.stale).toBe(false);
    expect(feed.items.map((item: { ref: string }) => item.ref)).toEqual(["USG-1", "USG-2"]);
    expect(feed.items[0].relations).toEqual([{ ref: "USG-2", kind: "child", short_description: "Issue 2" }]);
    const fresh = await fetch(`${hub.url}/api/freshness`).then((response) => response.json());
    expect(fresh).toMatchObject({ stale: false, staleAfterHours: 36, lastRun: { status: "completed" } });
  });
});
