import { expect, test } from "bun:test";
import { MAX_PROMPT_BYTES, runCodexAnalysis, type CodexRunnerOptions } from "../src/codex.js";
import { normalizeLinearIssue } from "../src/normalizer.js";
import { startGraphHub } from "../src/server.js";
import { GraphStore } from "../src/store.js";
import type { EnrichmentProgress } from "@commonkit/linear-graph-protocol";

export const issue = (id: string, description = "Full description") => normalizeLinearIssue({
  id, identifier: `USG-${id}`, title: `Issue ${id}`, description, url: `https://linear.app/x/USG-${id}`,
  team: { id: "team", key: "USG", name: "Unsold" }, state: { id: "todo", name: "Todo", type: "unstarted" },
  priority: 2, createdAt: "2026-11-01T00:00:00.000Z", updatedAt: "2026-11-02T00:00:00.000Z",
  completedAt: null, dueDate: null, project: null, cycle: null, labels: { nodes: [] }, parent: null,
});

export function answer(input: { issues?: { id: string }[]; summaries?: unknown[] }) {
  if (input.summaries) return { brief: "Every batch was included in this synthesis." };
  return {
    brief: "This batch has actionable work.",
    assignments: (input.issues ?? []).map(({ id }) => ({ issueId: id, zone: "product", topicTags: [], confidence: 0.9, rationale: "Product work" })),
    semanticEdges: [], recommendations: [],
    triageDecisions: (input.issues ?? []).map(({ id }) => ({ issueId: id, disposition: "ready", rationale: "Concrete task", confidence: 0.9, evidenceIssueIds: [id], nextAction: "Implement" })),
  };
}

export function fakeCodex(respond: (input: any, prompt: string) => unknown = answer): CodexRunnerOptions {
  return { spawn: () => {
    let output: ReadableStreamDefaultController<Uint8Array>;
    const stdout = new ReadableStream<Uint8Array>({ start(controller) { output = controller; } });
    return {
      stdin: { write(prompt: string) {
        try {
          const result = respond(JSON.parse(prompt.split("\n").at(-1)!), prompt);
          output.enqueue(new TextEncoder().encode(JSON.stringify(result))); output.close();
        } catch (error) { output.error(error); }
      }, end() {} }, stdout, stderr: undefined, exited: Promise.resolve(0), kill() {},
    } as never;
  } };
}

test("multi-megabyte input covers every open issue and full description in bounded prompts", async () => {
  const issues = Array.from({ length: 130 }, (_, i) => issue(String(i), `${i}:` + '"\\\n'.repeat(12000)));
  const seen = new Map<string, string>();
  let calls = 0;
  const result = await runCodexAnalysis({ issues, edges: [] }, fakeCodex((input, prompt) => {
    calls++;
    expect(Buffer.byteLength(prompt)).toBeLessThanOrEqual(256_000);
    for (const row of input.issues ?? []) { expect(seen.has(row.id)).toBe(false); seen.set(row.id, row.description); }
    return answer(input);
  }));
  expect(calls).toBeGreaterThan(1);
  expect(seen.size).toBe(issues.length);
  for (const row of issues) expect(seen.get(row.id)).toBe(row.description!);
  expect(result.triageDecisions).toHaveLength(issues.length);
  expect(result.assignments).toHaveLength(issues.length);
});

test("rejects malformed, incomplete, duplicate, and foreign analysis instead of claiming coverage", async () => {
  for (const change of [
    (out: any) => { out.triageDecisions = []; },
    (out: any) => { out.assignments.push(out.assignments[0]); },
    (out: any) => { out.triageDecisions[0].issueId = "invented"; },
    (out: any) => { out.triageDecisions[0].evidenceIssueIds = ["invented"]; },
    (out: any) => { out.triageDecisions[0].confidence = 2; },
    (out: any) => { out.assignments = "wrong"; },
    (out: any) => { out.extra = true; },
  ]) {
    await expect(runCodexAnalysis({ issues: [issue("1")], edges: [] }, fakeCodex((input) => {
      const out = answer(input); change(out); return out;
    }))).rejects.toThrow();
  }
});

test("coverage errors identify both collections and safe missing, duplicate and foreign IDs", async () => {
  const first = "11111111-1111-4111-8111-111111111111";
  const second = "22222222-2222-4222-8222-222222222222";
  const foreign = "33333333-3333-4333-8333-333333333333";
  let message = "";
  try {
    await runCodexAnalysis({ issues: [issue(first), issue(second)], edges: [] }, fakeCodex((input) => {
      const out = answer(input);
      out.assignments = [out.assignments![0]!, out.assignments![0]!, { ...out.assignments![0]!, issueId: foreign }];
      out.triageDecisions = [{ ...out.triageDecisions![0]!, issueId: "private@example.com" }];
      return out;
    }));
  } catch (error) { message = (error as Error).message; }
  expect(message).toContain(`assignments: expected=2 actual=3 missing=1 [${second}] duplicate=1 [${first}] foreign=1 [${foreign}]`);
  expect(message).toContain(`triageDecisions: expected=2 actual=1 missing=2 [${first},${second}] duplicate=0 [] foreign=1 [sha256:`);
  expect(message).not.toContain("private@example.com");
});

test("one correction recovers each coverage mismatch without dropping descriptions or relationship context", async () => {
  for (const collection of ["assignments", "triageDecisions"] as const) {
    for (const kind of ["missing", "duplicate", "foreign"] as const) {
      const issues = Array.from({ length: 26 }, (_, i) => issue(String(i).padStart(2, "0"), "Full context ".repeat(500)));
      const edges = [{ sourceId: "00", targetId: "25", kind: "blocks", source: "linear" }];
      const progress: EnrichmentProgress[] = [];
      let calls = 0;
      let original: unknown;
      const result = await runCodexAnalysis({ issues, edges, focusBrief: "Manual focus" }, fakeCodex((input, prompt) => {
        if (!input.issues) return answer(input);
        calls++;
        const out = answer(input);
        if (calls === 1) {
          original = input;
          if (kind === "missing") out[collection]!.pop();
          if (kind === "duplicate") out[collection]!.push(out[collection]![0]! as never);
          if (kind === "foreign") out[collection]![0]!.issueId = "25";
        }
        if (calls === 2) {
          expect(input).toEqual(original);
          expect(prompt).toContain("Coverage correction 1/1");
          expect(prompt).toContain(collection);
          expect(prompt).toContain(`${kind}=1`);
          expect(Buffer.byteLength(prompt)).toBeLessThanOrEqual(256_000);
          expect(progress.at(-1)).toMatchObject({ status: "running", analyzedIssues: 0, completedBatches: 0 });
        }
        return out;
      }), (value) => progress.push(value));
      expect(calls).toBe(3); // first batch, its correction, second batch
      expect(result.assignments).toHaveLength(26);
      expect(result.triageDecisions).toHaveLength(26);
      expect(progress.at(-1)).toMatchObject({ status: "completed", analyzedIssues: 26, completedBatches: 2, totalBatches: 2 });
    }
  }
});

test("a second coverage failure exhausts correction and retains only previously validated progress", async () => {
  let calls = 0;
  const progress: EnrichmentProgress[] = [];
  await expect(runCodexAnalysis({ issues: Array.from({ length: 26 }, (_, i) => issue(String(i))), edges: [] }, fakeCodex((input) => {
    calls++;
    const out = answer(input);
    if (calls >= 2) out.triageDecisions = [];
    return out;
  }), (value) => progress.push(value))).rejects.toThrow(/Batch 2\/2 failed after 25\/26 issues: Coverage correction 1\/1 failed: Codex coverage incomplete;.*triageDecisions: expected=1 actual=0 missing=1/);
  expect(calls).toBe(3);
  expect(progress.at(-1)).toMatchObject({ status: "failed", analyzedIssues: 25, completedBatches: 1, totalBatches: 2 });
});

test("schema and reference failures do not spend a coverage correction", async () => {
  for (const kind of ["schema", "reference"]) {
    let calls = 0;
    await expect(runCodexAnalysis({ issues: [issue("1")], edges: [] }, fakeCodex((input) => {
      calls++;
      const out = answer(input);
      if (kind === "schema") return { bad: true };
      out.triageDecisions![0]!.evidenceIssueIds = ["unknown"];
      return out;
    }))).rejects.toThrow();
    expect(calls).toBe(1);
  }
});

test("a malformed correction stops without a third request", async () => {
  let calls = 0;
  await expect(runCodexAnalysis({ issues: [issue("1")], edges: [] }, fakeCodex((input) => {
    calls++;
    return calls === 1 ? { ...answer(input), assignments: [] } : { invalid: true };
  }))).rejects.toThrow("Coverage correction 1/1 failed: Codex output failed schema validation");
  expect(calls).toBe(2);
});

test("transport failures are not retried as coverage failures", async () => {
  let calls = 0;
  const base = fakeCodex();
  await expect(runCodexAnalysis({ issues: [issue("1")], edges: [] }, {
    spawn(command, options) {
      calls++;
      return { ...base.spawn!(command, options), exited: Promise.resolve(1) } as never;
    },
  })).rejects.toThrow("Codex exited with 1");
  expect(calls).toBe(1);
});

test("correction overhead cannot exceed the prompt budget or truncate context", async () => {
  let bytes = 0;
  await runCodexAnalysis({ issues: [issue("1", "")], edges: [] }, fakeCodex((input, prompt) => {
    bytes = Buffer.byteLength(prompt); return answer(input);
  }));
  // Empty descriptions serialize as null; replacing null with a quoted ASCII
  // string costs two fewer overhead bytes.
  let calls = 0;
  await expect(runCodexAnalysis({ issues: [issue("1", "x".repeat(MAX_PROMPT_BYTES - bytes + 2))], edges: [] }, fakeCodex((input, prompt) => {
    calls++;
    expect(Buffer.byteLength(prompt)).toBe(MAX_PROMPT_BYTES);
    return { ...answer(input), assignments: [] };
  }))).rejects.toThrow("Coverage correction exceeds prompt budget: Codex coverage incomplete");
  expect(calls).toBe(1);
});

test("oversized single issues fail explicitly before spawning, never silently truncate", async () => {
  let calls = 0;
  await expect(runCodexAnalysis({ issues: [issue("large", "x".repeat(1_100_000))], edges: [] }, fakeCodex((input) => {
    calls++; return answer(input);
  }))).rejects.toThrow(/exceeds.*budget/);
  expect(calls).toBe(0);
});

test("cross-batch relations retain their external endpoint context", async () => {
  const issues = Array.from({ length: 26 }, (_, i) => issue(String(i).padStart(2, "0")));
  const edges = [{ id: "link", sourceId: "00", targetId: "25", kind: "blocks", source: "linear" }];
  const endpointBatches: string[] = [];
  await runCodexAnalysis({ issues, edges }, fakeCodex((input) => {
    if (input.issues?.some((row: any) => row.id === "00")) {
      expect(input.edges).toEqual(edges);
      expect(input.relatedIssues.some((row: any) => row.id === "25")).toBe(true);
      endpointBatches.push("0");
    }
    return answer(input);
  }));
  expect(endpointBatches).toEqual(["0"]);
});

test("empty and closed-only universes need no provider call", async () => {
  const result = await runCodexAnalysis({ issues: [{ ...issue("closed"), status: { id: "done", name: "Done", type: "completed" } }], edges: [] }, {
    spawn() { throw new Error("must not spawn"); },
  });
  expect(result.triageDecisions).toEqual([]);
  expect(result.brief).toContain("no active issues");
});

test("hierarchical synthesis includes all batches and bounds JSON escaping", async () => {
  const issues = Array.from({ length: 1051 }, (_, i) => issue(String(i)));
  let analyzed = 0;
  let synthesisCalls = 0;
  const progress: EnrichmentProgress[] = [];
  await runCodexAnalysis({ issues, edges: [] }, fakeCodex((input, prompt) => {
    expect(Buffer.byteLength(prompt)).toBeLessThanOrEqual(256_000);
    if (input.issues) analyzed += input.issues.length;
    else synthesisCalls++;
    return { ...answer(input), brief: "\u0001".repeat(5000) };
  }), (value) => progress.push(value));
  expect(analyzed).toBe(1051);
  expect(synthesisCalls).toBeGreaterThan(3);
  expect(progress.at(-1)).toMatchObject({ status: "completed", totalIssues: 1051, analyzedIssues: 1051, totalBatches: 43, completedBatches: 43 });
});

test("a later batch failure records partial coverage without publishing partial enrichment", async () => {
  const store = new GraphStore(":memory:");
  const oldAt = "2026-10-01T00:00:00.000Z";
  store.saveBrief({ text: "Universe synced: 1209 active issues are ready for triage and bundle planning.", updatedAt: oldAt, generatedAt: oldAt, snapshotAt: oldAt, status: "ready", source: "fallback" });
  let calls = 0;
  const rows = Array.from({ length: 26 }, (_, i) => {
    const normalized = issue(String(i));
    return { ...normalized, state: normalized.status, labels: { nodes: [] } };
  });
  const hub = startGraphHub({ actionToken: "test", port: 0, store,
    linear: { async fetchIssues() { return { nodes: rows, pageInfo: { hasNextPage: false } }; } },
    codex: fakeCodex((input) => { calls++; return calls >= 2 ? { ...answer(input), triageDecisions: [] } : answer(input); }),
  });
  try {
    const run = await hub.runAnalysis();
    expect(run).toMatchObject({ status: "completed", enrichment: { status: "failed", totalIssues: 26, analyzedIssues: 25, completedBatches: 1 } });
    expect(run.error).toContain("Batch 2/2 failed after 25/26 issues");
    expect(calls).toBe(3);
    expect(run.error).toContain("Coverage correction 1/1 failed");
    expect(store.loadTriageDecisions()).toEqual([]);
    expect(store.loadSnapshot()!.nodes).toHaveLength(26);
    expect(store.loadBrief()).toMatchObject({ source: "fallback", status: "failed", snapshotAt: store.loadSnapshot()!.syncedAt });
    expect(store.loadBrief()!.text).toContain("26 active issues");
    expect(store.loadBrief()!.text).not.toContain("1209");
    const fresh = await fetch(`${hub.url}/api/freshness`).then((response) => response.json());
    expect(fresh).toMatchObject({ stale: false, lastRun: { enrichment: { status: "failed", analyzedIssues: 25 } } });
  } finally { await hub.stop(); }
});

test("failed enrichment preserves old Codex brief provenance and manual ranking", async () => {
  for (const source of ["codex", "manual"] as const) {
    const store = new GraphStore(":memory:");
    const oldAt = "2026-10-01T00:00:00.000Z";
    store.saveBrief({ text: "USG-2 first", source, updatedAt: oldAt, generatedAt: oldAt, snapshotAt: oldAt, status: "ready" });
    const rows = [issue("1"), issue("2")].map((row) => ({ ...row, state: row.status, labels: { nodes: [] } }));
    const hub = startGraphHub({ actionToken: "test", port: 0, store,
      linear: { async fetchIssues() { return { nodes: rows, pageInfo: { hasNextPage: false } }; } },
      codex: fakeCodex(() => ({ invalid: true })),
    });
    try {
      await hub.runAnalysis();
      expect(store.loadBrief()).toMatchObject({ text: "USG-2 first", source, generatedAt: oldAt, snapshotAt: oldAt, status: source === "manual" ? "ready" : "stale" });
      if (source === "manual") expect(store.loadSnapshot()!.recommendations[0]!.issueId).toBe("2");
    } finally { await hub.stop(); }
  }
});

test("summary failure does not mark fully triaged batches as completed enrichment", async () => {
  const progress: EnrichmentProgress[] = [];
  await expect(runCodexAnalysis({ issues: Array.from({ length: 26 }, (_, i) => issue(String(i))), edges: [] }, fakeCodex((input) => input.summaries ? { brief: "" } : answer(input)), (value) => progress.push(value))).rejects.toThrow("summary failed schema validation");
  expect(progress.at(-1)).toMatchObject({ status: "failed", phase: "summarizing", analyzedIssues: 26, completedBatches: 2 });
});

test("successful multi-batch enrichment publishes all triage without changing deterministic ranking", async () => {
  const rows = Array.from({ length: 26 }, (_, i) => issue(String(i))).map((row) => ({ ...row, state: row.status, labels: { nodes: [] } }));
  const store = new GraphStore(":memory:");
  store.saveBrief({ text: "USG-25 first", source: "manual", updatedAt: "2026-10-01T00:00:00.000Z" });
  const hub = startGraphHub({ actionToken: "test", port: 0, store,
    linear: { async fetchIssues() { return { nodes: rows, pageInfo: { hasNextPage: false } }; } },
    codex: fakeCodex(),
  });
  try {
    const run = await hub.runAnalysis();
    expect(run).toMatchObject({ status: "completed", error: null, enrichment: { status: "completed", analyzedIssues: 26, completedBatches: 2 } });
    expect(store.loadTriageDecisions()).toHaveLength(26);
    expect(store.loadSnapshot()!.recommendations[0]!.issueId).toBe("25");
    expect(store.loadBrief()).toMatchObject({ text: "USG-25 first", source: "manual", status: "ready" });
  } finally { await hub.stop(); }
});

test("interrupted coverage retains counts and a running brief becomes stale on restart", async () => {
  const store = new GraphStore(":memory:");
  const timestamp = "2026-11-01T00:00:00.000Z";
  store.saveAnalysisRun({ id: "run", startedAt: timestamp, completedAt: null, status: "running", issueCount: 26, error: null, inputHash: null,
    enrichment: { status: "running", phase: "analyzing", totalIssues: 26, analyzedIssues: 25, totalBatches: 2, completedBatches: 1 },
  });
  store.saveBrief({ text: "Old brief", source: "codex", status: "running", snapshotAt: timestamp, updatedAt: timestamp });
  expect(store.closeInterruptedRuns()).toBe(1);
  expect(store.closeInterruptedRuns()).toBe(0);
  const hub = startGraphHub({ actionToken: "test", port: 0, store });
  try {
    expect(store.latestAnalysisRun()).toMatchObject({ status: "failed", enrichment: { status: "failed", analyzedIssues: 25 } });
    expect(store.loadBrief()).toMatchObject({ status: "stale", snapshotAt: timestamp });
  } finally { await hub.stop(); }
});
